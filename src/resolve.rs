//! Look up and cache a reference. One EMPIAR entry names the EMDB entries it
//! was deposited with, so a key with only an EMPIAR id still yields the
//! microscope settings.

use crate::context::{ReferenceContext, ReferenceKey};
use crate::emdb::EmdbClient;
use crate::empiar::EmpiarClient;
use crate::error::{Error, Result};
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Resolver {
    emdb: EmdbClient,
    empiar: EmpiarClient,
    cache: Cache<String, Arc<ReferenceContext>>,
}

impl Resolver {
    /// `ttl` bounds how long a looked-up entry is reused; `capacity` how
    /// many distinct keys are kept. Public EBI entries change rarely, so an
    /// hour and a few thousand keys is the usual setting.
    pub fn new(emdb: EmdbClient, empiar: EmpiarClient, ttl: Duration, capacity: u64) -> Self {
        Resolver {
            emdb,
            empiar,
            cache: Cache::builder()
                .max_capacity(capacity)
                .time_to_live(ttl)
                .build(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(
            EmdbClient::new(),
            EmpiarClient::new(),
            Duration::from_secs(3600),
            4096,
        )
    }

    /// Number of cached references. Runs the cache's pending housekeeping
    /// first so the count is current.
    pub async fn cache_len(&self) -> u64 {
        self.cache.run_pending_tasks().await;
        self.cache.entry_count()
    }

    /// Resolve `key`, fetching at most once per key per TTL. Errors are not
    /// cached, so a transient EBI failure is retried on the next event.
    pub async fn resolve(&self, key: &ReferenceKey) -> Result<Arc<ReferenceContext>> {
        let key = key.normalized()?;
        let ck = key.cache_key();
        let this = self.clone();
        self.cache
            .try_get_with(ck, async move { this.fetch(&key).await.map(Arc::new) })
            .await
            .map_err(|e: Arc<Error>| (*e).clone())
    }

    async fn fetch(&self, key: &ReferenceKey) -> Result<ReferenceContext> {
        let empiar = match &key.empiar {
            Some(id) => Some(self.empiar.entry(id).await?),
            None => None,
        };
        let emdb_id = key
            .emdb
            .clone()
            .or_else(|| empiar.as_ref().and_then(|p| p.emdb_ids.first().cloned()));
        let emdb = match emdb_id {
            Some(id) => match self.emdb.entry(&id).await {
                Ok(e) => Some(e),
                // An EMPIAR cross-reference to an EMDB entry that is not
                // released yet is common; keep what EMPIAR gave us.
                Err(Error::NotFound { .. }) if key.emdb.is_none() => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        Ok(ReferenceContext::merge(empiar, emdb))
    }
}

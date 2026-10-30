use std::sync::Arc;
use thiserror::Error;

/// Errors are `Clone` so a lookup shared between concurrent events for the
/// same reference can hand every waiter the same failure.
#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("{archive} entry {id} not found")]
    NotFound { archive: &'static str, id: String },

    #[error("{archive} id {id:?} is not a valid identifier")]
    BadId { archive: &'static str, id: String },

    #[error("{archive} returned HTTP {status} for {id}")]
    Http {
        archive: &'static str,
        id: String,
        status: u16,
    },

    #[error("request to {archive} failed: {source}")]
    Transport {
        archive: &'static str,
        #[source]
        source: Arc<reqwest::Error>,
    },

    #[error("{archive} response for {id} did not have the expected shape: {detail}")]
    Shape {
        archive: &'static str,
        id: String,
        detail: String,
    },

    #[error("event carries no EMPIAR or EMDB reference and no default was configured")]
    NoReference,

    #[error("event is not valid JSON: {0}")]
    Json(Arc<serde_json::Error>),

    #[error("kafka: {0}")]
    Kafka(#[from] rdkafka::error::KafkaError),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(Arc::new(e))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

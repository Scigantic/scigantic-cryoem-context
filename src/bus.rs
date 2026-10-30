//! The Kafka loop: consume an ingestion event, resolve its reference, check
//! it, produce the enriched event. Offsets are stored only after the
//! enriched message is acknowledged, so a crash replays rather than drops.

use crate::context::ReferenceKey;
use crate::error::{Error, Result};
use crate::message::{headers, EnrichedEvent, IngestEvent};
use crate::qc::{self, Status, Tolerances, Verdict};
use crate::resolve::Resolver;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::{Header, Headers, Message, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use rdkafka::util::Timeout;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct BusConfig {
    pub brokers: String,
    pub group_id: String,
    pub in_topic: String,
    pub out_topic: String,
    /// Where events that are not parseable JSON go. `None` logs and skips.
    pub dead_letter_topic: Option<String>,
    /// Applied when an event names no reference in its body or headers.
    pub default_reference: ReferenceKey,
    pub tolerances: Tolerances,
    /// Extra `librdkafka` properties, for example `security.protocol`.
    pub extra: Vec<(String, String)>,
    pub produce_timeout: Duration,
}

impl BusConfig {
    fn apply_extra(&self, c: &mut ClientConfig) {
        for (k, v) in &self.extra {
            c.set(k, v);
        }
    }

    pub fn consumer(&self) -> Result<StreamConsumer> {
        let mut c = ClientConfig::new();
        c.set("bootstrap.servers", &self.brokers)
            .set("group.id", &self.group_id)
            .set("enable.auto.commit", "true")
            .set("enable.auto.offset.store", "false")
            .set("auto.offset.reset", "earliest")
            .set("session.timeout.ms", "30000");
        self.apply_extra(&mut c);
        Ok(c.create()?)
    }

    pub fn producer(&self) -> Result<FutureProducer> {
        let mut c = ClientConfig::new();
        c.set("bootstrap.servers", &self.brokers)
            .set("enable.idempotence", "true")
            .set("compression.type", "lz4")
            .set("message.timeout.ms", "30000");
        self.apply_extra(&mut c);
        Ok(c.create()?)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub consumed: u64,
    pub enriched: u64,
    pub pass: u64,
    pub warn: u64,
    pub fail: u64,
    pub unknown: u64,
    pub dead_lettered: u64,
}

/// What to do with one consumed payload.
#[derive(Debug)]
pub enum Outcome {
    Enriched(Box<EnrichedEvent>),
    /// The payload was not an event. Carries the reason.
    Rejected(String),
}

/// Reference ids carried in Kafka headers, if any.
pub fn reference_from_headers<H: Headers>(h: Option<&H>) -> ReferenceKey {
    let mut key = ReferenceKey::default();
    let Some(h) = h else { return key };
    for i in 0..h.count() {
        let Header { key: k, value } = h.get(i);
        let Some(v) = value.and_then(|v| std::str::from_utf8(v).ok()) else {
            continue;
        };
        match k {
            headers::EMPIAR => key.empiar = Some(v.to_string()),
            headers::EMDB => key.emdb = Some(v.to_string()),
            _ => {}
        }
    }
    key
}

/// Enrich one payload. Reference resolution failures are not rejections:
/// the event is passed through with an `unknown` verdict that says why, so
/// downstream still sees every file.
pub async fn process(
    resolver: &Resolver,
    payload: &[u8],
    header_ref: &ReferenceKey,
    default_ref: &ReferenceKey,
    tol: &Tolerances,
) -> Outcome {
    let ev = match IngestEvent::parse(payload, header_ref) {
        Ok(ev) => ev,
        Err(e) => return Outcome::Rejected(format!("not an ingestion event: {e}")),
    };
    let ev = IngestEvent {
        reference: ev.reference.or(default_ref),
        ..ev
    };

    if ev.reference.is_empty() {
        let v = Verdict::unknown(&Error::NoReference.to_string());
        return Outcome::Enriched(Box::new(EnrichedEvent::new(ev, None, v)));
    }

    match resolver.resolve(&ev.reference).await {
        Ok(ctx) => {
            let verdict = if ev.observed.is_empty() {
                Verdict::unknown("header reports no setup values to compare")
            } else {
                qc::check(&ev.observed, &ctx.expected, tol)
            };
            Outcome::Enriched(Box::new(EnrichedEvent::new(
                ev,
                Some((*ctx).clone()),
                verdict,
            )))
        }
        Err(e) => {
            warn!(error = %e, "reference unresolved");
            let v = Verdict::unknown(&format!("reference unresolved: {e}"));
            Outcome::Enriched(Box::new(EnrichedEvent::new(ev, None, v)))
        }
    }
}

pub struct Bridge {
    cfg: BusConfig,
    resolver: Arc<Resolver>,
    consumer: StreamConsumer,
    producer: FutureProducer,
    stats: Arc<Mutex<Stats>>,
}

impl Bridge {
    pub fn new(cfg: BusConfig, resolver: Arc<Resolver>) -> Result<Bridge> {
        let consumer = cfg.consumer()?;
        let producer = cfg.producer()?;
        consumer.subscribe(&[cfg.in_topic.as_str()])?;
        Ok(Bridge {
            cfg,
            resolver,
            consumer,
            producer,
            stats: Arc::new(Mutex::new(Stats::default())),
        })
    }

    /// Live counters, shared with the health endpoint.
    pub fn stats(&self) -> Arc<Mutex<Stats>> {
        self.stats.clone()
    }

    /// Run until `shutdown` resolves. Returns counts.
    pub async fn run<F: Future<Output = ()>>(&self, shutdown: F) -> Result<Stats> {
        let mut stats = Stats::default();
        let shutdown = std::pin::pin!(shutdown);
        let mut shutdown = shutdown.fuse_once();
        info!(
            brokers = %self.cfg.brokers,
            in_topic = %self.cfg.in_topic,
            out_topic = %self.cfg.out_topic,
            "bridge running"
        );
        loop {
            let msg = tokio::select! {
                _ = &mut shutdown => break,
                m = self.consumer.recv() => m?,
            };
            stats.consumed += 1;
            let Some(payload) = msg.payload() else {
                self.consumer.store_offset_from_message(&msg)?;
                continue;
            };
            let header_ref = reference_from_headers(msg.headers());
            let key = msg.key().map(|k| k.to_vec());

            match process(
                &self.resolver,
                payload,
                &header_ref,
                &self.cfg.default_reference,
                &self.cfg.tolerances,
            )
            .await
            {
                Outcome::Enriched(ev) => {
                    match ev.qc.status {
                        Status::Pass => stats.pass += 1,
                        Status::Warn => stats.warn += 1,
                        Status::Fail => stats.fail += 1,
                        Status::Unknown => stats.unknown += 1,
                    }
                    let out_key = key
                        .or_else(|| ev.source.as_ref().map(|s| s.as_bytes().to_vec()))
                        .unwrap_or_default();
                    let body = serde_json::to_vec(&ev)?;
                    let status = serde_json::to_value(ev.qc.status)?
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string();
                    let refh = ev.reference_header();
                    let hdrs = OwnedHeaders::new()
                        .insert(Header {
                            key: headers::VERDICT,
                            value: Some(status.as_bytes()),
                        })
                        .insert(Header {
                            key: headers::REFERENCE,
                            value: Some(refh.as_bytes()),
                        });
                    self.send(&self.cfg.out_topic, &out_key, &body, hdrs)
                        .await?;
                    stats.enriched += 1;
                    debug!(source = ?ev.source, status = %status, "enriched");
                }
                Outcome::Rejected(reason) => {
                    warn!(%reason, "rejected");
                    if let Some(dlq) = &self.cfg.dead_letter_topic {
                        let hdrs = OwnedHeaders::new().insert(Header {
                            key: headers::ERROR,
                            value: Some(reason.as_bytes()),
                        });
                        self.send(dlq, &key.unwrap_or_default(), payload, hdrs)
                            .await?;
                        stats.dead_lettered += 1;
                    }
                }
            }
            self.consumer.store_offset_from_message(&msg)?;
            if let Ok(mut shared) = self.stats.lock() {
                *shared = stats;
            }
        }
        if let Err(e) = self.consumer.commit_consumer_state(CommitMode::Sync) {
            // Nothing stored yet is a normal way to stop.
            if !matches!(
                e,
                rdkafka::error::KafkaError::ConsumerCommit(
                    rdkafka::types::RDKafkaErrorCode::NoOffset
                )
            ) {
                return Err(e.into());
            }
        }
        self.producer
            .flush(Timeout::After(self.cfg.produce_timeout))?;
        info!(?stats, "bridge stopped");
        Ok(stats)
    }

    async fn send(
        &self,
        topic: &str,
        key: &[u8],
        payload: &[u8],
        hdrs: OwnedHeaders,
    ) -> Result<()> {
        let rec = FutureRecord::to(topic)
            .key(key)
            .payload(payload)
            .headers(hdrs);
        self.producer
            .send(rec, Timeout::After(self.cfg.produce_timeout))
            .await
            .map(|_| ())
            .map_err(|(e, _)| Error::Kafka(e))
    }
}

/// A future that, once complete, stays complete when polled again. `select!`
/// polls the shutdown branch every iteration, so a bare `Pin<&mut F>` would
/// be polled after completion.
trait FuseOnce: Future<Output = ()> + Sized {
    fn fuse_once(self) -> Fused<Self> {
        Fused {
            inner: self,
            done: false,
        }
    }
}
impl<F: Future<Output = ()>> FuseOnce for F {}

struct Fused<F> {
    inner: F,
    done: bool,
}

impl<F: Future<Output = ()> + Unpin> Future for Fused<F> {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.done {
            return std::task::Poll::Pending;
        }
        match std::pin::Pin::new(&mut self.inner).poll(cx) {
            std::task::Poll::Ready(()) => {
                self.done = true;
                std::task::Poll::Ready(())
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

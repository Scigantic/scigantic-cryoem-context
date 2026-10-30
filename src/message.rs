//! Wire shapes. Input is a `scigantic-edge` per-file record, optionally
//! carrying a `reference` object; output is that record wrapped with the
//! resolved reference and the verdict.
//!
//! Input, as `scigantic-edge` writes it plus the optional reference:
//!
//! ```json
//! {
//!   "reference": {"empiar": "10973", "emdb": "EMD-14173"},
//!   "source": "/mnt/krios/session1/FoilHole_001.eer",
//!   "sizeBytes": 147199534,
//!   "mtime": 1761700000,
//!   "decoded": {"format": "eer", "summary": "...", "fields": {"pixelSizeA": 0.43, "...": "..."}}
//! }
//! ```
//!
//! A flat `{"setup": {"pixelSizeA": 0.43, "voltageKv": 200}}` is also
//! accepted for producers that are not scigantic-edge.

use crate::context::{ReferenceContext, ReferenceKey};
use crate::observed::Observed;
use crate::qc::Verdict;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// Kafka header names read on input and written on output.
pub mod headers {
    pub const EMPIAR: &str = "scigantic-empiar";
    pub const EMDB: &str = "scigantic-emdb";
    pub const VERDICT: &str = "scigantic-qc-verdict";
    pub const REFERENCE: &str = "scigantic-reference";
    pub const ERROR: &str = "scigantic-bridge-error";
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestEvent {
    pub reference: ReferenceKey,
    pub source: Option<String>,
    pub observed: Observed,
    pub raw: Value,
}

impl IngestEvent {
    /// Parse a payload. `header_ref` is what the Kafka headers said; the
    /// body wins over headers when both name an archive.
    pub fn parse(
        payload: &[u8],
        header_ref: &ReferenceKey,
    ) -> Result<IngestEvent, serde_json::Error> {
        let raw: Value = serde_json::from_slice(payload)?;
        let body_ref: ReferenceKey = raw
            .get("reference")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let reference = body_ref.or(header_ref);
        let source = raw
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string);

        let observed = match raw.get("decoded") {
            Some(d) => {
                let format = d.get("format").and_then(Value::as_str).unwrap_or("");
                match d.get("fields").and_then(Value::as_object) {
                    Some(f) => Observed::from_decoded(format, f),
                    None => Observed {
                        format: (!format.is_empty()).then(|| format.to_string()),
                        ..Default::default()
                    },
                }
            }
            None => raw
                .get("setup")
                .or(Some(&raw))
                .and_then(Value::as_object)
                .map(Observed::from_flat)
                .unwrap_or_default(),
        };

        Ok(IngestEvent {
            reference,
            source,
            observed,
            raw,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStamp {
    pub version: String,
    pub processed_at_unix: u64,
    pub reference_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub event: Value,
    pub observed: Observed,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<ReferenceContext>,
    pub qc: Verdict,
    pub bridge: BridgeStamp,
}

impl EnrichedEvent {
    pub fn new(ev: IngestEvent, reference: Option<ReferenceContext>, qc: Verdict) -> EnrichedEvent {
        // The resolved key names both archives even when the event named one.
        let reference_key = match &reference {
            Some(r) => r.key.cache_key(),
            None => ev
                .reference
                .normalized()
                .map(|k| k.cache_key())
                .unwrap_or_default(),
        };
        EnrichedEvent {
            source: ev.source,
            event: ev.raw,
            observed: ev.observed,
            reference,
            qc,
            bridge: BridgeStamp {
                version: crate::VERSION.to_string(),
                processed_at_unix: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                reference_key,
            },
        }
    }

    /// Value for the `scigantic-reference` header: the archive ids used,
    /// comma separated.
    pub fn reference_header(&self) -> String {
        match &self.reference {
            Some(r) => [r.key.empiar.as_deref(), r.key.emdb.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(","),
            None => String::new(),
        }
    }
}

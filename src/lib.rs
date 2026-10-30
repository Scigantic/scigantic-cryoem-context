//! Reference context for cryo-EM ingestion on a Kafka bus.
//!
//! A facility's ingestion pipeline publishes one event per file it has seen
//! (for example the per-file record `scigantic-edge` writes). This crate
//! looks up the EMDB and EMPIAR entries the collection is meant to resemble,
//! attaches what they say about the instrument setup (voltage, pixel size,
//! detector, dose, defocus range, resolution), compares the observed header
//! against it, and republishes the event with that context and a QC verdict.
//!
//! The pieces are usable on their own:
//!
//! - [`emdb::EmdbClient`] and [`empiar::EmpiarClient`] fetch and flatten one
//!   entry each from the EBI REST APIs.
//! - [`context::ReferenceContext`] merges the two into one set of expected
//!   acquisition values.
//! - [`observed::Observed`] pulls the same values out of a decoded header
//!   record, using the field names `scigantic-edge` emits.
//! - [`qc::check`] compares observed against expected and returns findings.
//! - [`resolve::Resolver`] caches lookups; [`bus::Bridge`] runs the Kafka
//!   consume, enrich, produce loop; [`health::serve`] answers probes.

pub mod bus;
pub mod context;
pub mod emdb;
pub mod empiar;
pub mod error;
pub mod health;
pub mod message;
pub mod observed;
pub mod qc;
pub mod resolve;
pub(crate) mod value;

pub use context::{Expected, ReferenceContext, ReferenceKey};
pub use error::Error;
pub use observed::Observed;
pub use qc::{Finding, Severity, Status, Tolerances, Verdict};
pub use resolve::Resolver;

/// Crate version, stamped into every enriched message.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

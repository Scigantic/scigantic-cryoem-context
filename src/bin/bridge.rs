//! `cryoem-context-bridge`: run the Kafka loop, or look up and check
//! without a broker.

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use scigantic_cryoem_context::bus::{self, Bridge, BusConfig, Outcome};
use scigantic_cryoem_context::context::ReferenceKey;
use scigantic_cryoem_context::emdb::EmdbClient;
use scigantic_cryoem_context::empiar::EmpiarClient;
use scigantic_cryoem_context::qc::{Status, Tolerances};
use scigantic_cryoem_context::Resolver;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "cryoem-context-bridge", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Lookup {
    /// EMDB REST base URL
    #[arg(long, env = "CRYOEM_BRIDGE_EMDB_API", default_value = scigantic_cryoem_context::emdb::DEFAULT_BASE_URL)]
    emdb_api: String,
    /// EMPIAR REST base URL
    #[arg(long, env = "CRYOEM_BRIDGE_EMPIAR_API", default_value = scigantic_cryoem_context::empiar::DEFAULT_BASE_URL)]
    empiar_api: String,
    /// Seconds a resolved reference is reused before it is fetched again
    #[arg(long, env = "CRYOEM_BRIDGE_CACHE_TTL", default_value_t = 3600)]
    cache_ttl: u64,
    /// EMPIAR accession applied when an event names none (e.g. 10973)
    #[arg(long, env = "CRYOEM_BRIDGE_EMPIAR")]
    empiar: Option<String>,
    /// EMDB id applied when an event names none (e.g. EMD-14173)
    #[arg(long, env = "CRYOEM_BRIDGE_EMDB")]
    emdb: Option<String>,
    /// Relative pixel-size tolerance
    #[arg(long, default_value_t = 0.02)]
    tol_pixel: f64,
    /// Relative voltage tolerance
    #[arg(long, default_value_t = 0.01)]
    tol_voltage: f64,
    /// Relative dose tolerance
    #[arg(long, default_value_t = 0.25)]
    tol_dose: f64,
}

impl Lookup {
    fn resolver(&self) -> Resolver {
        Resolver::new(
            EmdbClient::with_base_url(&self.emdb_api),
            EmpiarClient::with_base_url(&self.empiar_api),
            Duration::from_secs(self.cache_ttl),
            4096,
        )
    }
    fn default_reference(&self) -> ReferenceKey {
        ReferenceKey {
            empiar: self.empiar.clone(),
            emdb: self.emdb.clone(),
        }
    }
    fn tolerances(&self) -> Tolerances {
        Tolerances {
            pixel_size: self.tol_pixel,
            voltage: self.tol_voltage,
            dose: self.tol_dose,
            ..Default::default()
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Consume ingestion events, enrich them, produce them
    Run {
        #[command(flatten)]
        lookup: Lookup,
        /// Comma-separated bootstrap servers
        #[arg(long, env = "CRYOEM_BRIDGE_BROKERS")]
        brokers: String,
        /// Consumer group id
        #[arg(
            long,
            env = "CRYOEM_BRIDGE_GROUP",
            default_value = "cryoem-context-bridge"
        )]
        group: String,
        /// Topic of ingestion events
        #[arg(long = "in", env = "CRYOEM_BRIDGE_IN", default_value = "cryoem.ingest")]
        in_topic: String,
        /// Topic the enriched events are written to
        #[arg(
            long = "out",
            env = "CRYOEM_BRIDGE_OUT",
            default_value = "cryoem.ingest.enriched"
        )]
        out_topic: String,
        /// Topic for payloads that are not events; omitted means skip
        #[arg(long, env = "CRYOEM_BRIDGE_DLQ")]
        dlq: Option<String>,
        /// Extra librdkafka property, key=value, repeatable
        /// (e.g. security.protocol=SASL_SSL)
        #[arg(long = "kafka", value_name = "KEY=VALUE")]
        kafka: Vec<String>,
    },
    /// Print the merged reference for an EMPIAR and/or EMDB id
    Lookup {
        #[command(flatten)]
        lookup: Lookup,
        /// Omit the full EMDB and EMPIAR entries, print only the expected values
        #[arg(long)]
        brief: bool,
    },
    /// Check one event file (an edge record JSON) against its reference.
    /// Exits 1 on a fail verdict, 2 on unknown.
    Check {
        #[command(flatten)]
        lookup: Lookup,
        /// Path to the event JSON, or - for stdin
        file: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rdkafka=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().cmd {
        Cmd::Lookup { lookup, brief } => {
            let key = lookup.default_reference();
            if key.is_empty() {
                bail!("give --empiar and/or --emdb");
            }
            let ctx = lookup.resolver().resolve(&key).await?;
            if brief {
                let v = serde_json::json!({"key": ctx.key, "title": ctx.title, "expected": ctx.expected});
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&*ctx)?);
            }
        }
        Cmd::Check { lookup, file } => {
            let payload = if file == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s.into_bytes()
            } else {
                std::fs::read(&file).with_context(|| format!("read {file}"))?
            };
            let resolver = lookup.resolver();
            let out = bus::process(
                &resolver,
                &payload,
                &ReferenceKey::default(),
                &lookup.default_reference(),
                &lookup.tolerances(),
            )
            .await;
            match out {
                Outcome::Enriched(ev) => {
                    println!("{}", serde_json::to_string_pretty(&ev)?);
                    match ev.qc.status {
                        Status::Pass | Status::Warn => {}
                        Status::Fail => std::process::exit(1),
                        Status::Unknown => std::process::exit(2),
                    }
                }
                Outcome::Rejected(reason) => bail!("{reason}"),
            }
        }
        Cmd::Run {
            lookup,
            brokers,
            group,
            in_topic,
            out_topic,
            dlq,
            kafka,
        } => {
            let mut extra = Vec::new();
            for kv in kafka {
                let (k, v) = kv
                    .split_once('=')
                    .with_context(|| format!("--kafka {kv:?} is not key=value"))?;
                extra.push((k.to_string(), v.to_string()));
            }
            let cfg = BusConfig {
                brokers,
                group_id: group,
                in_topic,
                out_topic,
                dead_letter_topic: dlq,
                default_reference: lookup.default_reference(),
                tolerances: lookup.tolerances(),
                extra,
                produce_timeout: Duration::from_secs(30),
            };
            let bridge = Bridge::new(cfg, Arc::new(lookup.resolver()))?;
            let stats = bridge
                .run(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
            eprintln!("{stats:?}");
        }
    }
    Ok(())
}

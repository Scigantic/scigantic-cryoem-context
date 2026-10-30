//! A minimal HTTP listener for Kubernetes probes. Any GET answers 200 with
//! the bridge's counters as JSON once the bridge is running. No framework:
//! one read, one write, per connection.

use crate::bus::Stats;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::info;

#[derive(Serialize)]
struct Body<'a> {
    status: &'a str,
    version: &'a str,
    #[serde(flatten)]
    stats: StatsJson,
}

#[derive(Serialize)]
struct StatsJson {
    consumed: u64,
    enriched: u64,
    pass: u64,
    warn: u64,
    fail: u64,
    unknown: u64,
    dead_lettered: u64,
}

impl From<Stats> for StatsJson {
    fn from(s: Stats) -> Self {
        StatsJson {
            consumed: s.consumed,
            enriched: s.enriched,
            pass: s.pass,
            warn: s.warn,
            fail: s.fail,
            unknown: s.unknown,
            dead_lettered: s.dead_lettered,
        }
    }
}

/// Bind `addr` and serve until the returned task is dropped or aborted.
pub async fn serve(
    addr: SocketAddr,
    stats: Arc<Mutex<Stats>>,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "health endpoint listening");
    Ok(tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                continue;
            };
            let stats = stats.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), sock.read(&mut buf))
                        .await;
                let snapshot = stats.lock().map(|s| *s).unwrap_or_default();
                let body = serde_json::to_vec(&Body {
                    status: "ok",
                    version: crate::VERSION,
                    stats: snapshot.into(),
                })
                .unwrap_or_default();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    }))
}

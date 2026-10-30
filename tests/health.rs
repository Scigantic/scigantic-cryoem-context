use scigantic_cryoem_context::bus::Stats;
use scigantic_cryoem_context::health;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn health_answers_200_with_counters() {
    let stats = Arc::new(Mutex::new(Stats {
        consumed: 7,
        enriched: 6,
        fail: 1,
        ..Default::default()
    }));
    // serve() does not report an ephemeral port back, so find a free one.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let task = health::serve(addr, stats).await.unwrap();

    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
    let body = out.split("\r\n\r\n").nth(1).unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["consumed"], 7);
    assert_eq!(v["fail"], 1);
    task.abort();
}

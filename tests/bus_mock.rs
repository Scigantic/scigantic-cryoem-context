//! End to end on librdkafka's in-process mock cluster: produce three
//! ingestion events, run the bridge, read the enriched topic back.
mod common;
use common::{eer_record, mdoc_record, mock_ebi, resolver_for};
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::{Header, Headers, Message, OwnedHeaders};
use rdkafka::mocking::MockCluster;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use rdkafka::ClientConfig;
use scigantic_cryoem_context::bus::{Bridge, BusConfig};
use scigantic_cryoem_context::message::{headers, EnrichedEvent};
use scigantic_cryoem_context::qc::{Status, Tolerances};
use scigantic_cryoem_context::ReferenceKey;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bridge_enriches_and_dead_letters() {
    let cluster = MockCluster::new(1).expect("mock cluster");
    let brokers = cluster.bootstrap_servers();
    cluster.create_topic("cryoem.ingest", 1, 1).unwrap();
    cluster
        .create_topic("cryoem.ingest.enriched", 1, 1)
        .unwrap();
    cluster.create_topic("cryoem.ingest.dlq", 1, 1).unwrap();

    let ebi = mock_ebi().await;
    let resolver = Arc::new(resolver_for(&ebi, Duration::from_secs(60)));

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("message.timeout.ms", "10000")
        .create()
        .unwrap();

    // 1. matching EER movie, reference in the body
    let good = serde_json::to_vec(&eer_record(
        0.43,
        Some(serde_json::json!({"empiar": "10973"})),
    ))
    .unwrap();
    // 2. 200 kV mdoc, reference via Kafka headers naming the 300 kV Krios entry
    let bad = serde_json::to_vec(&mdoc_record(200.0)).unwrap();
    // 3. not JSON
    let junk = b"not json".to_vec();
    // 4. valid event, no reference anywhere
    let orphan = serde_json::to_vec(&mdoc_record(300.0)).unwrap();

    let send = |key: &'static str, payload: Vec<u8>, hdrs: Option<OwnedHeaders>| {
        let producer = producer.clone();
        async move {
            let mut rec = FutureRecord::to("cryoem.ingest").key(key).payload(&payload);
            if let Some(h) = hdrs {
                rec = rec.headers(h);
            }
            producer
                .send(rec, Timeout::After(Duration::from_secs(10)))
                .await
                .unwrap();
        }
    };
    send("f1", good, None).await;
    send(
        "f2",
        bad,
        Some(OwnedHeaders::new().insert(Header {
            key: headers::EMDB,
            value: Some("EMD-11638"),
        })),
    )
    .await;
    send("f3", junk, None).await;
    send("f4", orphan, None).await;

    let cfg = BusConfig {
        brokers: brokers.clone(),
        group_id: "test-bridge".into(),
        in_topic: "cryoem.ingest".into(),
        out_topic: "cryoem.ingest.enriched".into(),
        dead_letter_topic: Some("cryoem.ingest.dlq".into()),
        default_reference: ReferenceKey::default(),
        tolerances: Tolerances::default(),
        extra: vec![],
        produce_timeout: Duration::from_secs(10),
    };
    let bridge = Bridge::new(cfg, resolver).unwrap();

    // Stop once four messages have been consumed, with a ceiling.
    let stats = bridge
        .run(tokio::time::sleep(Duration::from_secs(8)))
        .await
        .unwrap();
    assert_eq!(stats.consumed, 4, "{stats:?}");
    assert_eq!(stats.enriched, 3);
    assert_eq!(stats.dead_lettered, 1);
    assert_eq!(
        (stats.pass, stats.fail, stats.unknown, stats.warn),
        (1, 1, 1, 0)
    );

    let out: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("group.id", "test-reader")
        .set("auto.offset.reset", "earliest")
        .create()
        .unwrap();
    out.subscribe(&["cryoem.ingest.enriched"]).unwrap();
    let mut got = Vec::new();
    while got.len() < 3 {
        let m = tokio::time::timeout(Duration::from_secs(10), out.recv())
            .await
            .expect("enriched message within 10 s")
            .unwrap();
        let verdict = m
            .headers()
            .and_then(|h| {
                (0..h.count())
                    .map(|i| h.get(i))
                    .find(|x| x.key == headers::VERDICT)
                    .and_then(|x| x.value.map(|v| String::from_utf8_lossy(v).into_owned()))
            })
            .unwrap();
        let ev: EnrichedEvent = serde_json::from_slice(m.payload().unwrap()).unwrap();
        got.push((
            String::from_utf8_lossy(m.key().unwrap()).into_owned(),
            verdict,
            ev,
        ));
    }
    got.sort_by(|a, b| a.0.cmp(&b.0));

    let (k, h, ev) = &got[0];
    assert_eq!(k, "f1");
    assert_eq!(h, "pass");
    assert_eq!(ev.qc.status, Status::Pass);
    let r = ev.reference.as_ref().unwrap();
    assert_eq!(r.key.empiar.as_deref(), Some("EMPIAR-10973"));
    assert_eq!(r.key.emdb.as_deref(), Some("EMD-14173"));
    assert_eq!(ev.reference_header(), "EMPIAR-10973,EMD-14173");
    assert_eq!(ev.event["sizeBytes"], 147199534);

    let (k, h, ev) = &got[1];
    assert_eq!(k, "f2");
    assert_eq!(h, "fail");
    assert_eq!(ev.qc.findings[0].key, "voltageKv");
    assert_eq!(
        ev.reference.as_ref().unwrap().key.emdb.as_deref(),
        Some("EMD-11638")
    );

    let (k, h, ev) = &got[2];
    assert_eq!(k, "f4");
    assert_eq!(h, "unknown");
    assert!(ev.reference.is_none());
    assert!(ev.qc.findings[0]
        .message
        .contains("no EMPIAR or EMDB reference"));

    let dlq: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("group.id", "test-dlq")
        .set("auto.offset.reset", "earliest")
        .create()
        .unwrap();
    dlq.subscribe(&["cryoem.ingest.dlq"]).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(10), dlq.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.payload().unwrap(), b"not json");
    let reason = m
        .headers()
        .map(|h| h.get(0))
        .and_then(|x| x.value.map(|v| String::from_utf8_lossy(v).into_owned()))
        .unwrap();
    assert!(reason.starts_with("not an ingestion event"), "{reason}");
}

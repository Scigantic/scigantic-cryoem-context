mod common;
use common::{mock_ebi, resolver_for};
use scigantic_cryoem_context::{Error, ReferenceKey};
use std::time::Duration;

#[tokio::test]
async fn empiar_only_key_pulls_the_cross_referenced_emdb_entry() {
    let server = mock_ebi().await;
    let r = resolver_for(&server, Duration::from_secs(60));
    let ctx = r
        .resolve(&ReferenceKey {
            empiar: Some("10973".into()),
            emdb: None,
        })
        .await
        .unwrap();
    assert_eq!(ctx.key.emdb.as_deref(), Some("EMD-14173"));
    assert_eq!(ctx.expected.voltage_kv, Some(200.0));
    assert_eq!(ctx.expected.pixel_size_a, Some(0.43));
    assert_eq!(
        ctx.title.as_deref(),
        Some("1.6Å Apoferritin from 200kV Glacios with Selectris Falcon 4")
    );
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 2, "one EMPIAR call and one EMDB call");
}

#[tokio::test]
async fn second_resolve_is_served_from_cache() {
    let server = mock_ebi().await;
    let r = resolver_for(&server, Duration::from_secs(60));
    let k = ReferenceKey {
        empiar: Some("EMPIAR-10973".into()),
        emdb: Some("14173".into()),
    };
    let a = r.resolve(&k).await.unwrap();
    // Different spelling, same canonical key.
    let b = r
        .resolve(&ReferenceKey {
            empiar: Some("10973".into()),
            emdb: Some("EMD-14173".into()),
        })
        .await
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert_eq!(r.cache_len().await, 1);
}

#[tokio::test]
async fn ttl_expiry_refetches() {
    let server = mock_ebi().await;
    let r = resolver_for(&server, Duration::from_millis(50));
    let k = ReferenceKey {
        empiar: None,
        emdb: Some("EMD-11638".into()),
    };
    r.resolve(&k).await.unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    r.resolve(&k).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn not_found_surfaces_and_is_not_cached() {
    let server = mock_ebi().await;
    let r = resolver_for(&server, Duration::from_secs(60));
    let k = ReferenceKey {
        empiar: Some("999999".into()),
        emdb: None,
    };
    for _ in 0..2 {
        match r.resolve(&k).await {
            Err(Error::NotFound { archive, id }) => {
                assert_eq!(archive, "EMPIAR");
                assert_eq!(id, "EMPIAR-999999");
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "errors are retried"
    );

    // EMDB's way of saying not found is 200 with an empty object.
    match r
        .resolve(&ReferenceKey {
            empiar: None,
            emdb: Some("EMD-999999".into()),
        })
        .await
    {
        Err(Error::NotFound {
            archive: "EMDB",
            id,
        }) => assert_eq!(id, "EMD-999999"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn explicit_emdb_overrides_the_cross_reference() {
    let server = mock_ebi().await;
    let r = resolver_for(&server, Duration::from_secs(60));
    let ctx = r
        .resolve(&ReferenceKey {
            empiar: Some("10973".into()),
            emdb: Some("EMD-11638".into()),
        })
        .await
        .unwrap();
    assert_eq!(ctx.expected.voltage_kv, Some(300.0));
    assert_eq!(ctx.expected.pixel_size_a, Some(0.43));
}

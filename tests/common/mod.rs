#![allow(dead_code)]
use scigantic_cryoem_context::emdb::EmdbClient;
use scigantic_cryoem_context::empiar::EmpiarClient;
use scigantic_cryoem_context::Resolver;
use serde_json::Value;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub fn fixture(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&std::fs::read(&p).expect(&p)).expect("fixture json")
}

/// A mock EBI serving the three fixtures: EMPIAR-10973 and its paired
/// EMD-14173 (200 kV Glacios, Falcon 4), plus EMD-11638 (300 kV Krios).
pub async fn mock_ebi() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/empiar/api/entry/10973/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("empiar-10973.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/emdb/api/entry/EMD-14173"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("emd-14173.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/emdb/api/entry/EMD-11638"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("emd-11638.json")))
        .mount(&server)
        .await;
    // EMDB answers unknown ids with 200 and {}.
    Mock::given(method("GET"))
        .and(path("/emdb/api/entry/EMD-999999"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/empiar/api/entry/999999/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    server
}

pub fn resolver_for(server: &MockServer, ttl: Duration) -> Resolver {
    Resolver::new(
        EmdbClient::with_base_url(&format!("{}/emdb/api", server.uri())),
        EmpiarClient::with_base_url(&format!("{}/empiar/api", server.uri())),
        ttl,
        64,
    )
}

/// An edge record for a Falcon 4 EER movie from the EMPIAR-10973 session.
pub fn eer_record(pixel: f64, reference: Option<Value>) -> Value {
    let mut v = serde_json::json!({
        "source": "/mnt/glacios/session1/Images-Disc1/GridSquare_1/Data/FoilHole_0001.eer",
        "sizeBytes": 147199534,
        "mtime": 1761700000,
        "decoded": {
            "format": "eer",
            "summary": "EER 4096x4096, 371 frames",
            "fields": {
                "pixelSizeA": pixel,
                "commercialName": "Falcon 4i",
                "imageWidth": 4096,
                "imageLength": 4096,
                "numberOfFrames": 371,
                "exposureTime": 5.1
            }
        }
    });
    if let Some(r) = reference {
        v["reference"] = r;
    }
    v
}

/// A SerialEM mdoc record from a 200 kV collection.
pub fn mdoc_record(voltage: f64) -> Value {
    serde_json::json!({
        "source": "/mnt/glacios/session1/FoilHole_0001.mrc.mdoc",
        "decoded": {
            "format": "serialem-mdoc",
            "fields": {
                "pixelSpacing": 0.43,
                "voltage": voltage,
                "imageSize": [4096, 4096],
                "firstSection": {"exposureTime": 5.0, "exposureDose": 40.0, "pixelSpacing": 0.43}
            }
        }
    })
}

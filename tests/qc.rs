mod common;
use common::{eer_record, fixture, mdoc_record};
use scigantic_cryoem_context::context::ReferenceContext;
use scigantic_cryoem_context::message::IngestEvent;
use scigantic_cryoem_context::qc::{check, Severity, Status, Tolerances};
use scigantic_cryoem_context::{emdb, empiar, Observed, ReferenceKey};

fn glacios() -> ReferenceContext {
    ReferenceContext::merge(
        Some(empiar::parse(10973, &fixture("empiar-10973.json")).unwrap()),
        Some(emdb::parse("EMD-14173", &fixture("emd-14173.json")).unwrap()),
    )
}

fn krios() -> ReferenceContext {
    ReferenceContext::merge(
        None,
        Some(emdb::parse("EMD-11638", &fixture("emd-11638.json")).unwrap()),
    )
}

#[test]
fn matching_eer_passes() {
    let ev = IngestEvent::parse(
        serde_json::to_vec(&eer_record(0.43, None))
            .unwrap()
            .as_slice(),
        &ReferenceKey::default(),
    )
    .unwrap();
    assert_eq!(ev.observed.format.as_deref(), Some("eer"));
    assert_eq!(ev.observed.detector.as_deref(), Some("Falcon 4i"));
    assert_eq!(ev.observed.frames, Some(371));
    let v = check(&ev.observed, &glacios().expected, &Tolerances::default());
    assert_eq!(v.status, Status::Pass, "{v:#?}");
    assert!(v.checked.contains(&"pixelSizeA".to_string()));
    assert!(v.checked.contains(&"detector".to_string()));
    assert!(v.checked.contains(&"dataFormat".to_string()));
    // EER headers carry no voltage; the reference does.
    assert!(v.unobserved.contains(&"voltageKv".to_string()));
    // EMPIAR-10973 lists no image dimensions and EMD-14173 has no
    // digitization block, so dimensions are unreferenced.
    assert!(v.unreferenced.contains(&"imageWidth".to_string()));
}

#[test]
fn pixel_size_within_two_percent_passes_beyond_fails() {
    let x = glacios().expected;
    let mut o = Observed::from_flat(&serde_json::Map::new());
    o.pixel_size_a = Some(0.43 * 1.015);
    assert_eq!(check(&o, &x, &Tolerances::default()).status, Status::Pass);
    o.pixel_size_a = Some(0.43 * 1.03);
    let v = check(&o, &x, &Tolerances::default());
    assert_eq!(v.status, Status::Fail);
    let f = &v.findings[0];
    assert_eq!(f.key, "pixelSizeA");
    assert_eq!(f.severity, Severity::Fail);
    assert_eq!(f.reference.as_deref(), Some("EMPIAR-10973"));
    assert!(
        f.message.contains("EMPIAR-10973 reports 0.43 A"),
        "{}",
        f.message
    );
}

#[test]
fn wrong_magnification_fails() {
    // Falcon 4 at a different nominal magnification: 0.6 A instead of 0.43.
    let ev = IngestEvent::parse(
        serde_json::to_vec(&eer_record(0.6, None))
            .unwrap()
            .as_slice(),
        &ReferenceKey::default(),
    )
    .unwrap();
    let v = check(&ev.observed, &glacios().expected, &Tolerances::default());
    assert_eq!(v.status, Status::Fail);
    assert_eq!(v.findings.len(), 1);
    assert_eq!(v.findings[0].key, "pixelSizeA");
}

#[test]
fn two_hundred_kv_session_against_three_hundred_kv_prior_fails_on_voltage() {
    let ev = IngestEvent::parse(
        serde_json::to_vec(&mdoc_record(200.0)).unwrap().as_slice(),
        &ReferenceKey::default(),
    )
    .unwrap();
    assert_eq!(ev.observed.voltage_kv, Some(200.0));
    assert_eq!(ev.observed.dose_per_image_e_a2, Some(40.0));
    let v = check(&ev.observed, &krios().expected, &Tolerances::default());
    assert_eq!(v.status, Status::Fail, "{v:#?}");
    let keys: Vec<&str> = v.findings.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, vec!["voltageKv"]);
    assert_eq!(v.findings[0].reference.as_deref(), Some("EMD-11638"));
    // Dose 40 vs 40, dimensions 4096 vs 4096 agree; pixel size is not in
    // an EMDB-only reference.
    assert!(v.checked.contains(&"dosePerImageEA2".to_string()));
    assert!(v.checked.contains(&"imageWidth".to_string()));
    assert!(v.unreferenced.contains(&"pixelSizeA".to_string()));
}

#[test]
fn same_session_against_krios_passes_voltage_only_when_it_matches() {
    let ev = IngestEvent::parse(
        serde_json::to_vec(&mdoc_record(300.0)).unwrap().as_slice(),
        &ReferenceKey::default(),
    )
    .unwrap();
    let v = check(&ev.observed, &krios().expected, &Tolerances::default());
    assert_eq!(v.status, Status::Pass, "{v:#?}");
}

#[test]
fn dimension_and_detector_mismatches_warn() {
    let x = krios().expected; // 4096 x 4096, Falcon IV, 40 e/A^2
    let o = Observed {
        image_width: Some(5760),
        image_height: Some(4092),
        detector: Some("Gatan K3".into()),
        dose_per_image_e_a2: Some(60.0),
        ..Default::default()
    };
    let v = check(&o, &x, &Tolerances::default());
    assert_eq!(v.status, Status::Warn);
    let mut keys: Vec<&str> = v.findings.iter().map(|f| f.key.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["detector", "dosePerImageEA2", "imageHeight", "imageWidth"]
    );
    assert!(v.findings.iter().all(|f| f.severity == Severity::Warn));
}

#[test]
fn unrecognised_detector_is_info_not_warn() {
    let x = krios().expected;
    let o = Observed {
        detector: Some("Prototype camera".into()),
        voltage_kv: Some(300.0),
        ..Default::default()
    };
    let v = check(&o, &x, &Tolerances::default());
    assert_eq!(v.status, Status::Pass);
    assert_eq!(v.findings.len(), 1);
    assert_eq!(v.findings[0].severity, Severity::Info);
}

#[test]
fn nothing_comparable_is_unknown() {
    let v = check(
        &Observed::default(),
        &krios().expected,
        &Tolerances::default(),
    );
    assert_eq!(v.status, Status::Unknown);
    assert!(v.checked.is_empty());
}

#[test]
fn body_reference_wins_over_headers_and_flat_setup_is_accepted() {
    let body = serde_json::json!({
        "reference": {"empiar": "10973"},
        "setup": {"pixelSizeA": 0.43, "voltageKv": 200, "detector": "Falcon 4i"}
    });
    let hdr = ReferenceKey {
        empiar: Some("99999".into()),
        emdb: Some("EMD-14173".into()),
    };
    let ev = IngestEvent::parse(serde_json::to_vec(&body).unwrap().as_slice(), &hdr).unwrap();
    assert_eq!(ev.reference.empiar.as_deref(), Some("10973"));
    assert_eq!(ev.reference.emdb.as_deref(), Some("EMD-14173"));
    assert_eq!(ev.observed.voltage_kv, Some(200.0));
    assert_eq!(ev.observed.pixel_size_a, Some(0.43));
    let v = check(&ev.observed, &glacios().expected, &Tolerances::default());
    assert_eq!(v.status, Status::Pass, "{v:#?}");
}

#[test]
fn mrc_record_uses_edge_field_names() {
    let body = serde_json::json!({
        "decoded": {"format": "mrc", "fields": {"pixelSizeA": 1.06, "nx": 4096, "ny": 4096, "nz": 1, "dtype": "float32"}}
    });
    let ev = IngestEvent::parse(
        serde_json::to_vec(&body).unwrap().as_slice(),
        &ReferenceKey::default(),
    )
    .unwrap();
    assert_eq!(ev.observed.pixel_size_a, Some(1.06));
    assert_eq!(ev.observed.image_width, Some(4096));
    assert_eq!(ev.observed.dtype.as_deref(), Some("float32"));
    assert!(ev.reference.is_empty());
}

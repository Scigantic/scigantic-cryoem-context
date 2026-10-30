mod common;
use common::fixture;
use scigantic_cryoem_context::context::{ReferenceContext, ReferenceKey};
use scigantic_cryoem_context::{emdb, empiar, Error};

#[test]
fn emdb_11638_flattens() {
    let e = emdb::parse("EMD-11638", &fixture("emd-11638.json")).unwrap();
    assert_eq!(e.id, "EMD-11638");
    assert_eq!(e.status.as_deref(), Some("REL"));
    assert_eq!(e.method.as_deref(), Some("singleParticle"));
    assert_eq!(e.resolution_a, Some(1.22));
    assert_eq!(e.resolution_method.as_deref(), Some("FSC 0.143 CUT-OFF"));
    assert_eq!(e.number_images_used, Some(363126));
    assert_eq!(e.point_group.as_deref(), Some("O"));
    assert_eq!(e.map_pixel_spacing_a, Some(0.5332));
    assert_eq!(e.map_dimensions, Some([256, 256, 256]));
    assert_eq!(e.microscope.as_deref(), Some("FEI TITAN KRIOS"));
    assert_eq!(e.voltage_kv, Some(300.0));
    assert_eq!(e.nominal_cs_mm, Some(2.7));
    assert_eq!(e.nominal_defocus_min_um, Some(0.3));
    assert_eq!(e.nominal_defocus_max_um, Some(0.9));
    assert_eq!(e.nominal_magnification, Some(270000.0));
    assert_eq!(e.detector.as_deref(), Some("FEI FALCON IV (4k x 4k)"));
    assert_eq!(e.dose_per_image_e_a2, Some(40.0));
    assert_eq!(e.detector_width_px, Some(4096));
    assert_eq!(e.detector_height_px, Some(4096));
    assert_eq!(
        e.sample_name.as_deref(),
        Some("Mouse heavy-chain apoferritin")
    );
    assert_eq!(e.organisms, vec!["Mus musculus"]);
    assert_eq!(e.grid_model.as_deref(), Some("UltrAuFoil R1.2/1.3"));
    assert_eq!(
        e.vitrification_instrument.as_deref(),
        Some("FEI VITROBOT MARK IV")
    );
    assert_eq!(e.software, vec!["RELION"]);
    assert_eq!(e.pdb_ids, vec!["7a4m"]);
}

#[test]
fn emdb_14173_without_digitization_block() {
    let e = emdb::parse("EMD-14173", &fixture("emd-14173.json")).unwrap();
    assert_eq!(e.voltage_kv, Some(200.0));
    assert_eq!(e.microscope.as_deref(), Some("TFS GLACIOS"));
    assert_eq!(e.detector_width_px, None);
    assert_eq!(e.resolution_a, Some(1.635));
    assert_eq!(e.nominal_defocus_min_um, Some(0.5));
    // The deposited map is binned relative to acquisition (0.43 A).
    assert_eq!(e.map_pixel_spacing_a, Some(0.605));
}

#[test]
fn empiar_10973_flattens() {
    let p = empiar::parse(10973, &fixture("empiar-10973.json")).unwrap();
    assert_eq!(p.id, "EMPIAR-10973");
    assert_eq!(p.emdb_ids, vec!["EMD-14173"]);
    assert_eq!(p.status.as_deref(), Some("REL"));
    assert_eq!(p.dataset_size.as_deref(), Some("754.4 GB"));
    assert_eq!(p.doi.as_deref(), Some("10.6019/EMPIAR-10973"));
    let s = p.primary_imageset().unwrap();
    assert!(s.is_micrographs());
    assert_eq!(s.data_format.as_deref(), Some("EER"));
    assert_eq!(s.pixel_width_a, Some(0.43));
    assert_eq!(s.num_images, Some(4984));
    assert_eq!(s.frames_per_image, Some(371));
    assert_eq!(s.image_width, None);
}

#[test]
fn merge_prefers_empiar_for_images_and_emdb_for_microscope() {
    let p = empiar::parse(10973, &fixture("empiar-10973.json")).unwrap();
    let e = emdb::parse("EMD-14173", &fixture("emd-14173.json")).unwrap();
    let ctx = ReferenceContext::merge(Some(p), Some(e));
    let x = &ctx.expected;
    assert_eq!(x.pixel_size_a, Some(0.43));
    assert_eq!(x.voltage_kv, Some(200.0));
    assert_eq!(x.detector.as_deref(), Some("FEI FALCON IV (4k x 4k)"));
    assert_eq!(x.data_format.as_deref(), Some("EER"));
    assert_eq!(x.frames_per_image, Some(371));
    assert_eq!(x.dose_per_image_e_a2, Some(40.0));
    assert_eq!(x.resolution_a, Some(1.635));
    assert_eq!(x.provenance["pixelSizeA"], "EMPIAR-10973");
    assert_eq!(x.provenance["voltageKv"], "EMD-14173");
    assert_eq!(ctx.key.empiar.as_deref(), Some("EMPIAR-10973"));
    assert_eq!(ctx.key.emdb.as_deref(), Some("EMD-14173"));
    // Round trip through JSON.
    let s = serde_json::to_string(&ctx).unwrap();
    let back: ReferenceContext = serde_json::from_str(&s).unwrap();
    assert_eq!(back, ctx);
}

#[test]
fn ids_normalize() {
    for s in ["EMD-11638", "emd-11638", "EMD_11638", "11638", " EMD11638 "] {
        assert_eq!(emdb::normalize_id(s).unwrap(), "EMD-11638", "{s:?}");
    }
    for s in ["", "EMD-", "EMD-abc", "1234567", "7a4m"] {
        assert!(
            matches!(emdb::normalize_id(s), Err(Error::BadId { .. })),
            "{s:?}"
        );
    }
    for s in ["EMPIAR-10973", "empiar-10973", "10973", "EMPIAR_10973"] {
        assert_eq!(empiar::normalize_id(s).unwrap(), 10973, "{s:?}");
    }
    assert_eq!(empiar::canonical_id(42), "EMPIAR-00042");
    assert!(matches!(
        empiar::normalize_id("0"),
        Err(Error::BadId { .. })
    ));
    assert!(matches!(
        empiar::normalize_id("EMD-11638"),
        Err(Error::BadId { .. })
    ));

    let k = ReferenceKey {
        empiar: Some("10973".into()),
        emdb: None,
    }
    .normalized()
    .unwrap();
    assert_eq!(k.cache_key(), "EMPIAR-10973|");
    assert!(matches!(
        ReferenceKey::default().normalized(),
        Err(Error::NoReference)
    ));
}

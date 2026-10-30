//! EMDB entry lookup. One call to `https://www.ebi.ac.uk/emdb/api/entry/EMD-NNNNN`
//! flattened into the fields that describe how the map was collected.

use crate::error::{Error, Result};
use crate::value::{at, num_at, text, text_at, u64_at};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://www.ebi.ac.uk/emdb/api";

/// The acquisition and sample facts of one EMDB entry. Every field is
/// optional because the deposition schema makes nearly everything optional;
/// absent means the entry did not say, never a guess.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmdbEntry {
    pub id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub method: Option<String>,
    pub aggregation_state: Option<String>,

    pub resolution_a: Option<f64>,
    pub resolution_method: Option<String>,
    pub number_images_used: Option<u64>,
    pub point_group: Option<String>,

    /// Pixel spacing of the deposited map. This is after any binning and
    /// resampling, so it is not the acquisition pixel size.
    pub map_pixel_spacing_a: Option<f64>,
    pub map_dimensions: Option<[u64; 3]>,

    pub microscope: Option<String>,
    pub voltage_kv: Option<f64>,
    pub electron_source: Option<String>,
    pub illumination_mode: Option<String>,
    pub imaging_mode: Option<String>,
    pub nominal_cs_mm: Option<f64>,
    pub nominal_defocus_min_um: Option<f64>,
    pub nominal_defocus_max_um: Option<f64>,
    pub nominal_magnification: Option<f64>,

    pub detector: Option<String>,
    pub dose_per_image_e_a2: Option<f64>,
    pub detector_width_px: Option<u64>,
    pub detector_height_px: Option<u64>,

    pub sample_name: Option<String>,
    pub organisms: Vec<String>,
    pub molecular_weight_mda: Option<f64>,
    pub grid_model: Option<String>,
    pub vitrification_instrument: Option<String>,

    pub software: Vec<String>,
    pub pdb_ids: Vec<String>,
}

/// Accepts `EMD-11638`, `emd-11638`, `EMD_11638` or `11638` and returns
/// the canonical `EMD-11638`.
pub fn normalize_id(raw: &str) -> Result<String> {
    let s = raw.trim();
    let digits = s
        .strip_prefix("EMD-")
        .or_else(|| s.strip_prefix("emd-"))
        .or_else(|| s.strip_prefix("EMD_"))
        .or_else(|| s.strip_prefix("emd_"))
        .or_else(|| s.strip_prefix("EMD"))
        .or_else(|| s.strip_prefix("emd"))
        .unwrap_or(s);
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::BadId {
            archive: "EMDB",
            id: raw.to_string(),
        });
    }
    Ok(format!("EMD-{digits}"))
}

/// Flatten an EMDB API response.
pub fn parse(id: &str, v: &Value) -> Result<EmdbEntry> {
    let sd = at(v, "structure_determination_list.structure_determination.[]").ok_or_else(|| {
        Error::Shape {
            archive: "EMDB",
            id: id.to_string(),
            detail: "no structure_determination".into(),
        }
    })?;
    let micro = at(sd, "microscopy_list.microscopy.[]");
    let rec = micro.and_then(|m| at(m, "image_recording_list.image_recording.[]"));
    let proc_ = at(sd, "image_processing.[]");
    let final_rec = proc_.and_then(|p| at(p, "final_reconstruction"));
    let prep = at(sd, "specimen_preparation_list.specimen_preparation.[]");

    let map_dimensions = (|| {
        Some([
            u64_at(v, "map.dimensions.col")?,
            u64_at(v, "map.dimensions.row")?,
            u64_at(v, "map.dimensions.sec")?,
        ])
    })();

    let mut organisms = Vec::new();
    let mut mw = 0.0;
    let mut saw_mw = false;
    if let Some(macs) = at(v, "sample.macromolecule_list.macromolecule").and_then(Value::as_array) {
        for m in macs {
            if let Some(o) = text_at(m, "natural_source.organism") {
                if !organisms.contains(&o) {
                    organisms.push(o);
                }
            }
            if let Some(x) = num_at(m, "molecular_weight.theoretical") {
                mw += x;
                saw_mw = true;
            }
        }
    }

    let mut software = BTreeSet::new();
    if let Some(p) = proc_ {
        if let Some(obj) = p.as_object() {
            for step in obj.values() {
                if let Some(list) = at(step, "software_list.software").and_then(Value::as_array) {
                    for s in list {
                        if let Some(name) = text_at(s, "name") {
                            software.insert(name);
                        }
                    }
                }
            }
        }
    }

    let mut pdb_ids = Vec::new();
    if let Some(list) = at(v, "crossreferences.pdb_list.pdb_reference").and_then(Value::as_array) {
        for p in list {
            if let Some(x) = text_at(p, "pdb_id") {
                pdb_ids.push(x.to_lowercase());
            }
        }
    }

    Ok(EmdbEntry {
        id: text_at(v, "emdb_id").unwrap_or_else(|| id.to_string()),
        title: text_at(v, "admin.title"),
        status: text_at(v, "admin.current_status.code"),
        method: text_at(sd, "method"),
        aggregation_state: text_at(sd, "aggregation_state"),
        resolution_a: final_rec.and_then(|f| num_at(f, "resolution")),
        resolution_method: final_rec.and_then(|f| text_at(f, "resolution_method")),
        number_images_used: final_rec.and_then(|f| u64_at(f, "number_images_used")),
        point_group: final_rec.and_then(|f| text_at(f, "applied_symmetry.point_group")),
        map_pixel_spacing_a: num_at(v, "map.pixel_spacing.x"),
        map_dimensions,
        microscope: micro.and_then(|m| text_at(m, "microscope")),
        voltage_kv: micro.and_then(|m| num_at(m, "acceleration_voltage")),
        electron_source: micro.and_then(|m| text_at(m, "electron_source")),
        illumination_mode: micro.and_then(|m| text_at(m, "illumination_mode")),
        imaging_mode: micro.and_then(|m| text_at(m, "imaging_mode")),
        nominal_cs_mm: micro.and_then(|m| num_at(m, "nominal_cs")),
        nominal_defocus_min_um: micro.and_then(|m| num_at(m, "nominal_defocus_min")),
        nominal_defocus_max_um: micro.and_then(|m| num_at(m, "nominal_defocus_max")),
        nominal_magnification: micro.and_then(|m| num_at(m, "nominal_magnification")),
        detector: rec.and_then(|r| text_at(r, "film_or_detector_model")),
        dose_per_image_e_a2: rec.and_then(|r| num_at(r, "average_electron_dose_per_image")),
        detector_width_px: rec.and_then(|r| u64_at(r, "digitization_details.dimensions.width")),
        detector_height_px: rec.and_then(|r| u64_at(r, "digitization_details.dimensions.height")),
        sample_name: at(v, "sample.name").and_then(text),
        organisms,
        molecular_weight_mda: saw_mw.then_some(mw),
        grid_model: prep.and_then(|p| text_at(p, "grid.model")),
        vitrification_instrument: prep.and_then(|p| text_at(p, "vitrification.instrument")),
        software: software.into_iter().collect(),
        pdb_ids,
    })
}

#[derive(Debug, Clone)]
pub struct EmdbClient {
    http: reqwest::Client,
    base_url: String,
}

impl EmdbClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            http: default_http(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    pub fn entry_url(&self, id: &str) -> Result<String> {
        Ok(format!("{}/entry/{}", self.base_url, normalize_id(id)?))
    }

    pub async fn entry(&self, id: &str) -> Result<EmdbEntry> {
        let canonical = normalize_id(id)?;
        let url = format!("{}/entry/{}", self.base_url, canonical);
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| Error::Transport {
                archive: "EMDB",
                source: std::sync::Arc::new(e),
            })?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(Error::NotFound {
                archive: "EMDB",
                id: canonical,
            });
        }
        if !status.is_success() {
            return Err(Error::Http {
                archive: "EMDB",
                id: canonical,
                status: status.as_u16(),
            });
        }
        let body: Value = resp.json().await.map_err(|e| Error::Transport {
            archive: "EMDB",
            source: std::sync::Arc::new(e),
        })?;
        // EMDB answers an unknown id with 200 and an empty body or `{}`.
        if body.is_null() || body.as_object().map(|o| o.is_empty()).unwrap_or(false) {
            return Err(Error::NotFound {
                archive: "EMDB",
                id: canonical,
            });
        }
        parse(&canonical, &body)
    }
}

impl Default for EmdbClient {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!(
            "scigantic-cryoem-context/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client with static configuration")
}

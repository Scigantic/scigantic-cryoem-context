//! EMPIAR entry lookup. One call to `https://www.ebi.ac.uk/empiar/api/entry/NNNNN/`
//! flattened into the image sets and their acquisition pixel size.

use crate::emdb;
use crate::error::{Error, Result};
use crate::value::{at, num_at, text_at, u64_at};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://www.ebi.ac.uk/empiar/api";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSet {
    pub name: Option<String>,
    pub directory: Option<String>,
    pub category: Option<String>,
    pub data_format: Option<String>,
    pub header_format: Option<String>,
    pub num_images: Option<u64>,
    pub frames_per_image: Option<u64>,
    pub voxel_type: Option<String>,
    /// Acquisition pixel size in angstrom, as deposited for this image set.
    pub pixel_width_a: Option<f64>,
    pub pixel_height_a: Option<f64>,
    pub image_width: Option<u64>,
    pub image_height: Option<u64>,
    pub file_pattern: Option<String>,
}

impl ImageSet {
    /// Raw micrographs and movies, as opposed to particle stacks, masks or
    /// reconstructions. These are the image sets an ingestion pipeline
    /// should be compared against.
    pub fn is_micrographs(&self) -> bool {
        self.category
            .as_deref()
            .map(|c| c.to_ascii_lowercase().contains("micrograph"))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmpiarEntry {
    pub id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub release_date: Option<String>,
    pub experiment_type: Option<String>,
    pub scale: Option<String>,
    pub dataset_size: Option<String>,
    pub doi: Option<String>,
    /// EMDB ids this entry is deposited alongside, canonicalised to `EMD-NNNNN`.
    pub emdb_ids: Vec<String>,
    pub imagesets: Vec<ImageSet>,
}

impl EmpiarEntry {
    /// The image set to take acquisition values from: the first micrograph
    /// set with a pixel size, else the first set with a pixel size, else
    /// the first set.
    pub fn primary_imageset(&self) -> Option<&ImageSet> {
        self.imagesets
            .iter()
            .find(|s| s.is_micrographs() && s.pixel_width_a.is_some())
            .or_else(|| self.imagesets.iter().find(|s| s.pixel_width_a.is_some()))
            .or_else(|| self.imagesets.first())
    }
}

/// Accepts `EMPIAR-10973`, `empiar-10973`, `EMPIAR_10973` or `10973` and
/// returns the numeric accession.
pub fn normalize_id(raw: &str) -> Result<u32> {
    let s = raw.trim();
    let digits = s
        .strip_prefix("EMPIAR-")
        .or_else(|| s.strip_prefix("empiar-"))
        .or_else(|| s.strip_prefix("EMPIAR_"))
        .or_else(|| s.strip_prefix("empiar_"))
        .or_else(|| s.strip_prefix("EMPIAR"))
        .or_else(|| s.strip_prefix("empiar"))
        .unwrap_or(s);
    digits
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0 && digits.len() <= 6)
        .ok_or_else(|| Error::BadId {
            archive: "EMPIAR",
            id: raw.to_string(),
        })
}

pub fn canonical_id(n: u32) -> String {
    format!("EMPIAR-{n:05}")
}

/// Flatten an EMPIAR API response. The body is keyed by the accession:
/// `{"EMPIAR-10973": {...}}`.
pub fn parse(n: u32, v: &Value) -> Result<EmpiarEntry> {
    let id = canonical_id(n);
    let e = v
        .get(&id)
        .or_else(|| v.as_object().and_then(|o| o.values().next()))
        .filter(|e| e.is_object())
        .ok_or_else(|| Error::NotFound {
            archive: "EMPIAR",
            id: id.clone(),
        })?;

    let mut emdb_ids = Vec::new();
    if let Some(list) = at(e, "cross_references").and_then(Value::as_array) {
        for x in list {
            let s = match x {
                Value::String(s) => s.clone(),
                Value::Object(_) => match text_at(x, "name") {
                    Some(s) => s,
                    None => continue,
                },
                _ => continue,
            };
            if let Ok(c) = emdb::normalize_id(&s) {
                if !emdb_ids.contains(&c) {
                    emdb_ids.push(c);
                }
            }
        }
    }

    let mut imagesets = Vec::new();
    if let Some(list) = at(e, "imagesets").and_then(Value::as_array) {
        for s in list {
            imagesets.push(ImageSet {
                name: text_at(s, "name"),
                directory: text_at(s, "directory"),
                category: text_at(s, "category"),
                data_format: text_at(s, "data_format"),
                header_format: text_at(s, "header_format"),
                num_images: u64_at(s, "num_images_or_tilt_series"),
                frames_per_image: u64_at(s, "frames_per_image"),
                voxel_type: text_at(s, "voxel_type"),
                pixel_width_a: num_at(s, "pixel_width").filter(|x| *x > 0.0),
                pixel_height_a: num_at(s, "pixel_height").filter(|x| *x > 0.0),
                image_width: u64_at(s, "image_width").filter(|x| *x > 0),
                image_height: u64_at(s, "image_height").filter(|x| *x > 0),
                file_pattern: text_at(s, "micrographs_file_pattern"),
            });
        }
    }

    Ok(EmpiarEntry {
        id,
        title: text_at(e, "title"),
        status: text_at(e, "status"),
        release_date: text_at(e, "release_date"),
        experiment_type: text_at(e, "experiment_type"),
        scale: text_at(e, "scale"),
        dataset_size: text_at(e, "dataset_size"),
        doi: text_at(e, "entry_doi"),
        emdb_ids,
        imagesets,
    })
}

#[derive(Debug, Clone)]
pub struct EmpiarClient {
    http: reqwest::Client,
    base_url: String,
}

impl EmpiarClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            http: emdb::default_http(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    pub fn entry_url(&self, id: &str) -> Result<String> {
        Ok(format!("{}/entry/{}/", self.base_url, normalize_id(id)?))
    }

    pub async fn entry(&self, id: &str) -> Result<EmpiarEntry> {
        let n = normalize_id(id)?;
        let canonical = canonical_id(n);
        let url = format!("{}/entry/{}/", self.base_url, n);
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| Error::Transport {
                archive: "EMPIAR",
                source: std::sync::Arc::new(e),
            })?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(Error::NotFound {
                archive: "EMPIAR",
                id: canonical,
            });
        }
        if !status.is_success() {
            return Err(Error::Http {
                archive: "EMPIAR",
                id: canonical,
                status: status.as_u16(),
            });
        }
        let body: Value = resp.json().await.map_err(|e| Error::Transport {
            archive: "EMPIAR",
            source: std::sync::Arc::new(e),
        })?;
        parse(n, &body)
    }
}

impl Default for EmpiarClient {
    fn default() -> Self {
        Self::new()
    }
}

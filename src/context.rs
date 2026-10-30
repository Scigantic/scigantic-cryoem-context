//! The merged reference: what EMDB and EMPIAR together say a collection
//! like this one should look like at the microscope.

use crate::emdb::EmdbEntry;
use crate::empiar::EmpiarEntry;
use crate::error::{Error, Result};
use crate::{emdb, empiar};
use serde::{Deserialize, Serialize};

/// Which archive entries to look up. Either may be absent; at least one
/// must be present to resolve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReferenceKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empiar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emdb: Option<String>,
}

/// A blank id (an unset environment variable read as `""`) is no id.
fn present(o: &Option<String>) -> Option<&str> {
    o.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl ReferenceKey {
    pub fn is_empty(&self) -> bool {
        present(&self.empiar).is_none() && present(&self.emdb).is_none()
    }

    /// Canonicalise both ids, rejecting anything that is not an accession.
    /// Blank ids are treated as absent.
    pub fn normalized(&self) -> Result<ReferenceKey> {
        if self.is_empty() {
            return Err(Error::NoReference);
        }
        Ok(ReferenceKey {
            empiar: present(&self.empiar)
                .map(|s| empiar::normalize_id(s).map(empiar::canonical_id))
                .transpose()?,
            emdb: present(&self.emdb).map(emdb::normalize_id).transpose()?,
        })
    }

    /// Stable cache key, for example `EMPIAR-10973|EMD-14173`.
    pub fn cache_key(&self) -> String {
        format!(
            "{}|{}",
            self.empiar.as_deref().unwrap_or(""),
            self.emdb.as_deref().unwrap_or("")
        )
    }

    /// Fill in whatever `other` has that `self` lacks. Blank ids on either
    /// side count as lacking.
    pub fn or(self, other: &ReferenceKey) -> ReferenceKey {
        ReferenceKey {
            empiar: present(&self.empiar)
                .or(present(&other.empiar))
                .map(str::to_string),
            emdb: present(&self.emdb)
                .or(present(&other.emdb))
                .map(str::to_string),
        }
    }
}

/// Expected acquisition values in the canonical keys `scigantic-edge` uses
/// for its setup check, plus the collection-level facts that have no
/// per-file counterpart. Each carries where it came from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expected {
    pub pixel_size_a: Option<f64>,
    pub voltage_kv: Option<f64>,
    pub detector: Option<String>,
    pub image_width: Option<u64>,
    pub image_height: Option<u64>,
    pub data_format: Option<String>,
    pub frames_per_image: Option<u64>,
    pub dose_per_image_e_a2: Option<f64>,
    pub defocus_min_um: Option<f64>,
    pub defocus_max_um: Option<f64>,
    pub nominal_magnification: Option<f64>,
    pub microscope: Option<String>,
    pub resolution_a: Option<f64>,
    /// Field name to archive id, for every field above that is set.
    pub provenance: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceContext {
    pub key: ReferenceKey,
    pub title: Option<String>,
    pub expected: Expected,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empiar: Option<EmpiarEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emdb: Option<EmdbEntry>,
}

impl ReferenceContext {
    /// Merge the two entries. EMPIAR supplies what is only known about the
    /// raw images (acquisition pixel size, dimensions, format, frame count);
    /// EMDB supplies the microscope settings and the outcome. When both have
    /// a value, EMPIAR wins for image-set facts and EMDB for microscope facts.
    pub fn merge(empiar: Option<EmpiarEntry>, emdb: Option<EmdbEntry>) -> ReferenceContext {
        let mut x = Expected::default();
        let mut prov = std::collections::BTreeMap::new();

        if let Some(e) = &emdb {
            let id = e.id.clone();
            let mut put = |name: &str, present: bool| {
                if present {
                    prov.insert(name.to_string(), id.clone());
                }
            };
            x.voltage_kv = e.voltage_kv;
            put("voltageKv", e.voltage_kv.is_some());
            x.detector = e.detector.clone();
            put("detector", e.detector.is_some());
            x.image_width = e.detector_width_px;
            put("imageWidth", e.detector_width_px.is_some());
            x.image_height = e.detector_height_px;
            put("imageHeight", e.detector_height_px.is_some());
            x.dose_per_image_e_a2 = e.dose_per_image_e_a2;
            put("dosePerImageEA2", e.dose_per_image_e_a2.is_some());
            x.defocus_min_um = e.nominal_defocus_min_um;
            put("defocusMinUm", e.nominal_defocus_min_um.is_some());
            x.defocus_max_um = e.nominal_defocus_max_um;
            put("defocusMaxUm", e.nominal_defocus_max_um.is_some());
            x.nominal_magnification = e.nominal_magnification;
            put("nominalMagnification", e.nominal_magnification.is_some());
            x.microscope = e.microscope.clone();
            put("microscope", e.microscope.is_some());
            x.resolution_a = e.resolution_a;
            put("resolutionA", e.resolution_a.is_some());
        }

        if let Some(p) = &empiar {
            if let Some(s) = p.primary_imageset() {
                let id = p.id.clone();
                if let Some(v) = s.pixel_width_a {
                    x.pixel_size_a = Some(v);
                    prov.insert("pixelSizeA".into(), id.clone());
                }
                if let Some(v) = s.image_width {
                    x.image_width = Some(v);
                    prov.insert("imageWidth".into(), id.clone());
                }
                if let Some(v) = s.image_height {
                    x.image_height = Some(v);
                    prov.insert("imageHeight".into(), id.clone());
                }
                if let Some(v) = &s.data_format {
                    x.data_format = Some(v.clone());
                    prov.insert("dataFormat".into(), id.clone());
                }
                if let Some(v) = s.frames_per_image {
                    x.frames_per_image = Some(v);
                    prov.insert("framesPerImage".into(), id.clone());
                }
            }
        }

        x.provenance = prov;
        let title = empiar
            .as_ref()
            .and_then(|p| p.title.clone())
            .or_else(|| emdb.as_ref().and_then(|e| e.title.clone()));
        let key = ReferenceKey {
            empiar: empiar.as_ref().map(|p| p.id.clone()),
            emdb: emdb.as_ref().map(|e| e.id.clone()),
        };
        ReferenceContext {
            key,
            title,
            expected: x,
            empiar,
            emdb,
        }
    }
}

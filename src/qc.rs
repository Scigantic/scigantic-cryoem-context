//! Compare an observed header against the reference and say where they
//! disagree. Tolerances follow `scigantic-edge` (`signal/setup.go`): pixel
//! size 2 percent, voltage 1 percent, dimensions exact, exposure 10 percent.
//! Dose has no edge counterpart; 25 percent is used.

use crate::context::Expected;
use crate::observed::Observed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Every comparable key agreed.
    Pass,
    /// Something disagreed that is worth a look but does not stop ingestion.
    Warn,
    /// A setup parameter is wrong: ingestion of this file should be held.
    Fail,
    /// No reference could be resolved or nothing was comparable.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub key: String,
    pub severity: Severity,
    pub observed: Value,
    pub expected: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub status: Status,
    pub findings: Vec<Finding>,
    /// Keys present on both sides and compared.
    pub checked: Vec<String>,
    /// Keys the reference has that the header did not report.
    pub unobserved: Vec<String>,
    /// Keys the header reports that the reference does not have.
    pub unreferenced: Vec<String>,
}

impl Verdict {
    pub fn unknown(reason: &str) -> Verdict {
        Verdict {
            status: Status::Unknown,
            findings: vec![Finding {
                key: "reference".into(),
                severity: Severity::Warn,
                observed: Value::Null,
                expected: Value::Null,
                tolerance: None,
                reference: None,
                message: reason.to_string(),
            }],
            checked: vec![],
            unobserved: vec![],
            unreferenced: vec![],
        }
    }
}

/// Relative tolerances per key. `0.0` means exact.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tolerances {
    pub pixel_size: f64,
    pub voltage: f64,
    pub image_dims: f64,
    pub dose: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Tolerances {
            pixel_size: 0.02,
            voltage: 0.01,
            image_dims: 0.0,
            dose: 0.25,
        }
    }
}

fn rel_diff(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return if a == 0.0 { 0.0 } else { f64::INFINITY };
    }
    ((a - b) / b).abs()
}

/// Detector family tokens, so "FEI FALCON IV (4k x 4k)" and the EER
/// header's "Falcon 4i" compare equal. Lower case, no spaces.
fn detector_family(s: &str) -> Option<&'static str> {
    let t: String = s
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    const FAMILIES: &[(&str, &[&str])] = &[
        ("falcon4", &["falcon4", "falconiv", "falcon4i"]),
        ("falcon3", &["falcon3", "falconiii"]),
        ("falcon2", &["falcon2", "falconii"]),
        ("k3", &["gatank3", "k3"]),
        ("k2", &["gatank2", "k2summit", "k2"]),
        ("de64", &["de64"]),
        ("de20", &["de20"]),
        ("ceta", &["ceta"]),
        ("apollo", &["apollo"]),
        ("rio", &["gatanrio", "rio"]),
    ];
    for (family, aliases) in FAMILIES {
        if aliases.iter().any(|a| t.contains(a)) {
            return Some(family);
        }
    }
    None
}

/// Compare `obs` against `exp`.
pub fn check(obs: &Observed, exp: &Expected, tol: &Tolerances) -> Verdict {
    let mut findings = Vec::new();
    let mut checked = Vec::new();
    let mut unobserved = Vec::new();
    let mut unreferenced = Vec::new();
    let prov = |k: &str| exp.provenance.get(k).cloned();

    let mut numeric = |key: &str,
                       o: Option<f64>,
                       e: Option<f64>,
                       t: f64,
                       sev: Severity,
                       what: &str,
                       unit: &str| {
        match (o, e) {
            (Some(o), Some(e)) => {
                checked.push(key.to_string());
                let d = rel_diff(o, e);
                if d > t {
                    findings.push(Finding {
                        key: key.to_string(),
                        severity: sev,
                        observed: json!(o),
                        expected: json!(e),
                        tolerance: Some(t),
                        reference: prov(key),
                        message: format!(
                            "{what} is {o} {unit}; {} reports {e} {unit} ({:.1}% off, tolerance {:.0}%)",
                            prov(key).unwrap_or_else(|| "the reference".into()),
                            d * 100.0,
                            t * 100.0
                        ),
                    });
                }
            }
            (None, Some(_)) => unobserved.push(key.to_string()),
            (Some(_), None) => unreferenced.push(key.to_string()),
            (None, None) => {}
        }
    };

    numeric(
        "pixelSizeA",
        obs.pixel_size_a,
        exp.pixel_size_a,
        tol.pixel_size,
        Severity::Fail,
        "pixel size",
        "A",
    );
    numeric(
        "voltageKv",
        obs.voltage_kv,
        exp.voltage_kv,
        tol.voltage,
        Severity::Fail,
        "accelerating voltage",
        "kV",
    );
    numeric(
        "imageWidth",
        obs.image_width.map(|x| x as f64),
        exp.image_width.map(|x| x as f64),
        tol.image_dims,
        Severity::Warn,
        "image width",
        "px",
    );
    numeric(
        "imageHeight",
        obs.image_height.map(|x| x as f64),
        exp.image_height.map(|x| x as f64),
        tol.image_dims,
        Severity::Warn,
        "image height",
        "px",
    );
    numeric(
        "dosePerImageEA2",
        obs.dose_per_image_e_a2,
        exp.dose_per_image_e_a2,
        tol.dose,
        Severity::Warn,
        "dose per image",
        "e/A^2",
    );

    match (&obs.detector, &exp.detector) {
        (Some(o), Some(e)) => {
            checked.push("detector".into());
            match (detector_family(o), detector_family(e)) {
                (Some(fo), Some(fe)) if fo != fe => findings.push(Finding {
                    key: "detector".into(),
                    severity: Severity::Warn,
                    observed: json!(o),
                    expected: json!(e),
                    tolerance: None,
                    reference: prov("detector"),
                    message: format!("detector is {o:?} ({fo}); the reference used {e:?} ({fe})"),
                }),
                (None, _) | (_, None) => findings.push(Finding {
                    key: "detector".into(),
                    severity: Severity::Info,
                    observed: json!(o),
                    expected: json!(e),
                    tolerance: None,
                    reference: prov("detector"),
                    message: format!(
                        "detector {o:?} could not be matched to the reference's {e:?}"
                    ),
                }),
                _ => {}
            }
        }
        (None, Some(_)) => unobserved.push("detector".into()),
        (Some(_), None) => unreferenced.push("detector".into()),
        (None, None) => {}
    }

    if let (Some(o), Some(e)) = (&obs.format, &exp.data_format) {
        checked.push("dataFormat".into());
        let oo = o.to_ascii_lowercase();
        let ee = e.to_ascii_lowercase();
        let same = oo == ee
            || (oo == "mrc" && (ee == "mrcs" || ee == "mrc"))
            || (oo == "serialem-mdoc")
            || (oo == "epu-image");
        if !same {
            findings.push(Finding {
                key: "dataFormat".into(),
                severity: Severity::Info,
                observed: json!(o),
                expected: json!(e),
                tolerance: None,
                reference: prov("dataFormat"),
                message: format!("file format is {o}; the reference image set is {e}"),
            });
        }
    }

    let status = if checked.is_empty() {
        Status::Unknown
    } else {
        match findings.iter().map(|f| f.severity).max() {
            Some(Severity::Fail) => Status::Fail,
            Some(Severity::Warn) => Status::Warn,
            _ => Status::Pass,
        }
    };

    Verdict {
        status,
        findings,
        checked,
        unobserved,
        unreferenced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        assert_eq!(detector_family("FEI FALCON IV (4k x 4k)"), Some("falcon4"));
        assert_eq!(detector_family("Falcon 4i"), Some("falcon4"));
        assert_eq!(detector_family("GATAN K3 (6k x 4k)"), Some("k3"));
        assert_eq!(detector_family("K2 SUMMIT"), Some("k2"));
        assert_eq!(detector_family("BM-Ceta"), Some("ceta"));
        assert_eq!(detector_family("mystery"), None);
    }

    #[test]
    fn rel() {
        assert_eq!(rel_diff(300.0, 300.0), 0.0);
        assert!((rel_diff(200.0, 300.0) - 1.0 / 3.0).abs() < 1e-12);
        assert!(rel_diff(1.0, 0.0).is_infinite());
    }
}

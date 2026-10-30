//! The setup values a decoded header reports, in the canonical keys
//! `scigantic-edge` uses (`signal/setup.go`): pixelSizeA, voltageKv,
//! detector, imageWidth, imageHeight, dtype, exposureTime. The per-format
//! field names are copied from there so the two agree on what a header
//! says.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    pub format: Option<String>,
    pub pixel_size_a: Option<f64>,
    pub voltage_kv: Option<f64>,
    pub detector: Option<String>,
    pub image_width: Option<u64>,
    pub image_height: Option<u64>,
    pub dtype: Option<String>,
    pub exposure_time_s: Option<f64>,
    pub dose_per_image_e_a2: Option<f64>,
    pub frames: Option<u64>,
}

fn pos(v: Option<&Value>) -> Option<f64> {
    let n = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (n.is_finite() && n > 0.0).then_some(n)
}

fn count(v: Option<&Value>) -> Option<u64> {
    let n = pos(v)?;
    (n.fract() == 0.0).then_some(n as u64)
}

fn txt(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

impl Observed {
    /// From a `scigantic-edge` decoded header: its `format` and `fields`.
    pub fn from_decoded(format: &str, f: &Map<String, Value>) -> Observed {
        let mut o = Observed {
            format: Some(format.to_string()),
            ..Default::default()
        };
        match format {
            "mrc" => {
                o.pixel_size_a = pos(f.get("pixelSizeA"));
                o.image_width = count(f.get("nx"));
                o.image_height = count(f.get("ny"));
                o.dtype = txt(f.get("dtype"));
                o.voltage_kv = pos(f.get("voltageKv")).or_else(|| pos(f.get("ht")));
                o.frames = count(f.get("nz"));
            }
            "epu-image" => {
                o.pixel_size_a = pos(f.get("pixelSizeA"));
                o.voltage_kv = pos(f.get("voltageKv"));
                o.detector = txt(f.get("cameraName"));
                o.image_width = count(f.get("readoutWidth"));
                o.image_height = count(f.get("readoutHeight"));
                o.exposure_time_s = pos(f.get("exposureTime"));
                o.dose_per_image_e_a2 = pos(f.get("dose"));
            }
            "eer" => {
                o.pixel_size_a = pos(f.get("pixelSizeA"));
                o.detector = txt(f.get("commercialName"));
                o.image_width = count(f.get("imageWidth"));
                o.image_height = count(f.get("imageLength"));
                o.exposure_time_s = pos(f.get("exposureTime"));
                o.frames = count(f.get("numberOfFrames")).or_else(|| count(f.get("frames")));
                o.voltage_kv = pos(f.get("voltageKv"));
            }
            "serialem-mdoc" => {
                o.pixel_size_a = pos(f.get("pixelSpacing"));
                o.voltage_kv = pos(f.get("voltage"));
                if let Some(size) = f.get("imageSize").and_then(Value::as_array) {
                    if size.len() == 2 {
                        o.image_width = count(size.first());
                        o.image_height = count(size.get(1));
                    }
                }
                if let Some(first) = f.get("firstSection").and_then(Value::as_object) {
                    o.exposure_time_s = pos(first.get("exposureTime"));
                    if o.pixel_size_a.is_none() {
                        o.pixel_size_a = pos(first.get("pixelSpacing"));
                    }
                    o.dose_per_image_e_a2 = pos(first.get("exposureDose"));
                }
                o.frames = count(f.get("sections"));
            }
            _ => {
                // Unknown format: accept the canonical keys if the producer
                // already flattened them.
                o.merge_flat(f);
            }
        }
        o
    }

    /// From an object that already uses the canonical keys.
    pub fn from_flat(f: &Map<String, Value>) -> Observed {
        let mut o = Observed::default();
        o.merge_flat(f);
        o
    }

    fn merge_flat(&mut self, f: &Map<String, Value>) {
        self.pixel_size_a = self.pixel_size_a.or_else(|| pos(f.get("pixelSizeA")));
        self.voltage_kv = self.voltage_kv.or_else(|| pos(f.get("voltageKv")));
        self.detector = self.detector.take().or_else(|| txt(f.get("detector")));
        self.image_width = self.image_width.or_else(|| count(f.get("imageWidth")));
        self.image_height = self.image_height.or_else(|| count(f.get("imageHeight")));
        self.dtype = self.dtype.take().or_else(|| txt(f.get("dtype")));
        self.exposure_time_s = self.exposure_time_s.or_else(|| pos(f.get("exposureTime")));
        self.dose_per_image_e_a2 = self
            .dose_per_image_e_a2
            .or_else(|| pos(f.get("dosePerImageEA2")));
        self.frames = self.frames.or_else(|| count(f.get("frames")));
        if self.format.is_none() {
            self.format = txt(f.get("format"));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pixel_size_a.is_none()
            && self.voltage_kv.is_none()
            && self.detector.is_none()
            && self.image_width.is_none()
            && self.image_height.is_none()
            && self.dose_per_image_e_a2.is_none()
    }
}

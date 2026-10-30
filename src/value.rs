//! Helpers for walking the EBI JSON, which wraps most scalars as
//! `{"units": "kV", "valueOf_": "300"}` and nests lists under `*_list`.

use serde_json::Value;

/// Follow a dotted path. A numeric segment indexes a list; `[]` takes the
/// first element of a list, which is how the EBI schema exposes the only
/// microscopy or image_recording block most entries have.
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = match seg {
            "[]" => cur.as_array()?.first()?,
            s => match s.parse::<usize>() {
                Ok(i) => cur.as_array()?.get(i)?,
                Err(_) => cur.as_object()?.get(s)?,
            },
        };
    }
    Some(cur)
}

/// A number, whether written as a JSON number, a numeric string, or the EBI
/// `{"valueOf_": "300"}` wrapper. NaN and infinities are rejected.
pub fn num(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Object(o) => return o.get("valueOf_").and_then(num),
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// Text, whether a plain string or the `{"valueOf_": "..."}` wrapper. Empty
/// after trimming is absent.
pub fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Object(o) => return o.get("valueOf_").and_then(text),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

pub fn num_at(v: &Value, path: &str) -> Option<f64> {
    at(v, path).and_then(num)
}

pub fn text_at(v: &Value, path: &str) -> Option<String> {
    at(v, path).and_then(text)
}

pub fn u64_at(v: &Value, path: &str) -> Option<u64> {
    let n = num_at(v, path)?;
    (n >= 0.0 && n.fract() == 0.0).then_some(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrapped_and_plain_scalars() {
        let v = json!({"a": {"units": "kV", "valueOf_": "300"}, "b": 2.5, "c": "x", "d": {"valueOf_": " "}});
        assert_eq!(num_at(&v, "a"), Some(300.0));
        assert_eq!(num_at(&v, "b"), Some(2.5));
        assert_eq!(num_at(&v, "c"), None);
        assert_eq!(text_at(&v, "c"), Some("x".into()));
        assert_eq!(text_at(&v, "d"), None);
    }

    #[test]
    fn path_indexes_lists() {
        let v = json!({"l": {"m": [{"x": 1}, {"x": 2}]}});
        assert_eq!(num_at(&v, "l.m.[].x"), Some(1.0));
        assert_eq!(num_at(&v, "l.m.1.x"), Some(2.0));
        assert_eq!(num_at(&v, "l.m.2.x"), None);
    }
}

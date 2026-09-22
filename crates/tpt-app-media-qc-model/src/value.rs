//! Measured/expected value type used in findings ([spec § 6.3]).
//!
//! Every finding must be able to explain *what was measured* and *what was
//! expected* so the failure is auditable.

use crate::time::Rational;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A typed measurement value with a stable, human-readable and JSON form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bool(bool),
    /// A rational (`num/den`) value, e.g. a frame rate.
    Rational(Rational),
    /// A byte count.
    Bytes(u64),
    /// A duration, serialized as milliseconds.
    DurationMs(u64),
    /// A plain signed ratio like loudness offset in LU.
    Ratio(f64),
}

impl Value {
    pub fn duration(d: Duration) -> Self {
        Value::DurationMs(d.as_millis() as u64)
    }

    pub fn bytes(n: u64) -> Self {
        Value::Bytes(n)
    }

    /// JSON representation used when embedding findings into reports.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Int(v) => serde_json::json!(v),
            Value::UInt(v) => serde_json::json!(v),
            Value::Float(v) => {
                if v.is_finite() {
                    serde_json::json!(v)
                } else {
                    serde_json::json!(null)
                }
            }
            Value::Text(v) => serde_json::json!(v),
            Value::Bool(v) => serde_json::json!(v),
            Value::Rational(r) => serde_json::json!(r.to_string()),
            Value::Bytes(v) => serde_json::json!({ "bytes": v }),
            Value::DurationMs(v) => serde_json::json!({ "duration_ms": v }),
            Value::Ratio(v) => serde_json::json!({ "ratio": v }),
        }
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::UInt(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Float(v) => {
                if v.fract() == 0.0 && v.abs() < 1e15 {
                    write!(f, "{v:.0}")
                } else {
                    write!(f, "{v:.3}")
                }
            }
            Value::Text(v) => f.write_str(v),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Rational(r) => write!(f, "{r}"),
            Value::Bytes(v) => write!(f, "{v} bytes"),
            Value::DurationMs(v) => {
                let secs = v / 1000;
                let ms = v % 1000;
                write!(f, "{}.{:03}s", secs, ms)
            }
            Value::Ratio(v) => write!(f, "{v:.2}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_forms() {
        assert_eq!(Value::UInt(1920).to_string(), "1920");
        assert_eq!(Value::Bytes(8_000_000).to_string(), "8000000 bytes");
        assert_eq!(Value::DurationMs(4_240).to_string(), "4.240s");
        assert_eq!(Value::Float(25.0).to_string(), "25");
    }

    #[test]
    fn to_json_forms() {
        assert_eq!(Value::DurationMs(500).to_json(), serde_json::json!({"duration_ms": 500}));
        assert_eq!(Value::from("h264").to_json(), serde_json::json!("h264"));
        assert_eq!(Value::from(1920u64).to_json(), serde_json::json!(1920));
    }
}
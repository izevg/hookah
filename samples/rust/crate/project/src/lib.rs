//! Small numeric helpers.

mod legacy;

pub use legacy::{describe, shorten};

/// Arithmetic mean, or `None` for an empty slice.
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// Clamps `value` into `lo..=hi`.
pub fn clamp(value: i64, lo: i64, hi: i64) -> i64 {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

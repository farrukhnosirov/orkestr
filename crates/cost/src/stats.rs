//! Statistics helpers shared by the analytics queries.
//!
//! Model benchmarking and agent performance (§22) live in
//! [`crate::benchmarks`]; this module keeps the small numeric helpers.

/// Nearest-rank percentile of sorted values.
#[must_use]
pub fn percentile(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rank = ((p.clamp(0.0, 1.0) * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted.get(rank - 1).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[10], 0.95), Some(10));
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 0.5), Some(50));
        assert_eq!(percentile(&v, 0.95), Some(95));
    }
}

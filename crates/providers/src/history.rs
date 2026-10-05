//! Historical model statistics for auto-routing (§7.2, §22 "feed router
//! auto-scoring").
//!
//! The cost engine collects execution benchmarks (first-pass success, call
//! success, latency, cost per execution); the core wires a
//! [`ModelStatsSource`] into the registry so [`crate::router::score`] can
//! blend history into the static capability / price / health score. With too
//! few samples history is ignored, and its weight grows with confidence.

use orkestr_domain::ModelRef;
use serde::{Deserialize, Serialize};

/// Minimum executions before history influences routing.
pub const MIN_SAMPLES: u32 = 3;
/// Executions at which history reaches full weight.
pub const FULL_CONFIDENCE_SAMPLES: u32 = 20;
/// Max absolute score adjustment from history.
pub const MAX_HISTORY_ADJUSTMENT: f64 = 0.4;

/// What history says about one model (optionally for one purpose / role).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelHistory {
    /// Executions behind the numbers.
    pub samples: u32,
    /// Successful calls / calls (0..1).
    pub call_success_rate: f64,
    /// First-pass task success (0..1) of finished executions, when known.
    pub first_pass_rate: Option<f64>,
    /// Average call latency (ms).
    pub avg_latency_ms: Option<f64>,
    /// Average cost per execution (USD).
    pub avg_cost_usd: Option<f64>,
}

impl ModelHistory {
    /// Quality in 0..1: first-pass success when known (blended with call
    /// success), else call success.
    #[must_use]
    pub fn quality(&self) -> f64 {
        match self.first_pass_rate {
            Some(fp) => 0.65 * fp.clamp(0.0, 1.0) + 0.35 * self.call_success_rate.clamp(0.0, 1.0),
            None => self.call_success_rate.clamp(0.0, 1.0),
        }
    }

    /// Historical score in 0..1 (quality 60 %, latency 25 %, cost 15 %).
    #[must_use]
    pub fn score(&self) -> f64 {
        let latency = self.avg_latency_ms.map_or(0.5, |ms| 1.0 / (1.0 + ms.max(0.0) / 2000.0));
        let cost = self.avg_cost_usd.map_or(0.5, |c| 1.0 / (1.0 + c.max(0.0) / 0.25));
        0.6 * self.quality() + 0.25 * latency + 0.15 * cost
    }

    /// Weight 0..1 by sample count (0 below [`MIN_SAMPLES`]).
    #[must_use]
    pub fn confidence(&self) -> f64 {
        if self.samples < MIN_SAMPLES {
            0.0
        } else {
            (f64::from(self.samples) / f64::from(FULL_CONFIDENCE_SAMPLES)).min(1.0)
        }
    }

    /// Additive router score adjustment in `[-MAX, +MAX]`.
    #[must_use]
    pub fn adjustment(&self) -> f64 {
        self.confidence() * MAX_HISTORY_ADJUSTMENT * 2.0 * (self.score() - 0.5)
    }
}

/// Source of historical statistics (implemented over the cost engine's
/// benchmark snapshot). Must be cheap: it is called while routing.
pub trait ModelStatsSource: Send + Sync {
    /// History of `model`, preferring the purpose-specific numbers when the
    /// purpose (role) has enough samples.
    fn history(&self, model: &ModelRef, purpose: Option<&str>) -> Option<ModelHistory>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(samples: u32, fp: Option<f64>, call: f64) -> ModelHistory {
        ModelHistory { samples, call_success_rate: call, first_pass_rate: fp, avg_latency_ms: Some(1000.0), avg_cost_usd: Some(0.05) }
    }

    #[test]
    fn confidence_and_adjustment() {
        assert_eq!(h(2, Some(1.0), 1.0).adjustment(), 0.0, "below minimum samples");
        let good = h(20, Some(0.95), 1.0);
        let bad = h(20, Some(0.1), 0.6);
        assert!(good.adjustment() > 0.1);
        assert!(bad.adjustment() < 0.0 && bad.adjustment() < good.adjustment() - 0.3);
        assert!(good.adjustment() <= MAX_HISTORY_ADJUSTMENT);
        assert!(h(5, Some(0.95), 1.0).adjustment() < good.adjustment(), "fewer samples weigh less");
        assert!((h(3, None, 0.8).quality() - 0.8).abs() < 1e-9);
    }
}

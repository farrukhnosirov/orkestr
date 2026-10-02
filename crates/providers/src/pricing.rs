//! Static pricing table — **ESTIMATED** list prices in USD per 1M tokens.
//!
//! These values are a convenience snapshot for budget estimates and the cost
//! center (§22); they are not authoritative and may be outdated. Every cost
//! computed from them must be labelled "estimated" in the UI. The cost area
//! may override them with user-maintained prices.

use orkestr_domain::ModelPricing;

use crate::types::Usage;

/// Date the table was last reviewed (shown next to estimates).
pub const PRICING_TABLE_DATE: &str = "2026-10-01";

/// `(model id prefix, input, output, cached input)` — longest prefix wins.
const TABLE: &[(&str, f64, f64, Option<f64>)] = &[
    // OpenAI
    ("gpt-5-nano", 0.05, 0.40, Some(0.005)),
    ("gpt-5-mini", 0.25, 2.00, Some(0.025)),
    ("gpt-5", 1.25, 10.00, Some(0.125)),
    ("gpt-4.1-nano", 0.10, 0.40, Some(0.025)),
    ("gpt-4.1-mini", 0.40, 1.60, Some(0.10)),
    ("gpt-4.1", 2.00, 8.00, Some(0.50)),
    ("gpt-4o-mini", 0.15, 0.60, Some(0.075)),
    ("gpt-4o", 2.50, 10.00, Some(1.25)),
    ("o4-mini", 1.10, 4.40, Some(0.275)),
    ("o3-mini", 1.10, 4.40, Some(0.55)),
    ("o3", 2.00, 8.00, Some(0.50)),
    ("o1", 15.00, 60.00, Some(7.50)),
    // Anthropic
    ("claude-opus-4-5", 5.00, 25.00, Some(0.50)),
    ("claude-opus-4", 15.00, 75.00, Some(1.50)),
    ("claude-sonnet-4", 3.00, 15.00, Some(0.30)),
    ("claude-haiku-4", 1.00, 5.00, Some(0.10)),
    ("claude-3-7-sonnet", 3.00, 15.00, Some(0.30)),
    ("claude-3-5-sonnet", 3.00, 15.00, Some(0.30)),
    ("claude-3-5-haiku", 0.80, 4.00, Some(0.08)),
    ("claude-3-haiku", 0.25, 1.25, Some(0.03)),
    // Google
    ("gemini-2.5-pro", 1.25, 10.00, Some(0.31)),
    ("gemini-2.5-flash-lite", 0.10, 0.40, Some(0.025)),
    ("gemini-2.5-flash", 0.30, 2.50, Some(0.075)),
    ("gemini-2.0-flash-lite", 0.075, 0.30, None),
    ("gemini-2.0-flash", 0.10, 0.40, Some(0.025)),
    ("gemini-1.5-pro", 1.25, 5.00, None),
    ("gemini-1.5-flash", 0.075, 0.30, None),
    // DeepSeek
    ("deepseek-reasoner", 0.55, 2.19, Some(0.14)),
    ("deepseek-chat", 0.27, 1.10, Some(0.07)),
];

/// Estimated list price of a model (`None` = unknown / local / subscription).
#[must_use]
pub fn pricing_for(model: &str) -> Option<ModelPricing> {
    let id = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    TABLE.iter().filter(|(prefix, ..)| id.starts_with(prefix)).max_by_key(|(prefix, ..)| prefix.len()).map(
        |(_, input, output, cached)| ModelPricing {
            input_per_mtok_usd: *input,
            output_per_mtok_usd: *output,
            cached_input_per_mtok_usd: *cached,
        },
    )
}

/// Estimated USD cost of a usage report with the given pricing.
#[must_use]
pub fn cost_with(pricing: &ModelPricing, usage: &Usage) -> f64 {
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let uncached = usage.input_tokens - cached;
    let cached_price = pricing.cached_input_per_mtok_usd.unwrap_or(pricing.input_per_mtok_usd);
    (uncached as f64 * pricing.input_per_mtok_usd
        + cached as f64 * cached_price
        + usage.output_tokens as f64 * pricing.output_per_mtok_usd)
        / 1_000_000.0
}

/// Estimated USD cost of a call (`None` when the model has no known price).
/// A cost reported by the provider itself takes precedence.
#[must_use]
pub fn estimate_cost(model: &str, usage: &Usage) -> Option<f64> {
    if let Some(c) = usage.reported_cost_usd {
        return Some(c);
    }
    pricing_for(model).map(|p| cost_with(&p, usage))
}

/// Pre-flight estimate for the router's budget filter:
/// `input_estimate × input_price + max_output × output_price`.
#[must_use]
pub fn preflight_estimate(pricing: &ModelPricing, input_tokens: u64, max_output_tokens: u64) -> f64 {
    (input_tokens as f64 * pricing.input_per_mtok_usd + max_output_tokens as f64 * pricing.output_per_mtok_usd)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        assert_eq!(pricing_for("gpt-4o-mini-2024-07-18").unwrap().input_per_mtok_usd, 0.15);
        assert_eq!(pricing_for("gpt-4o-2024-08-06").unwrap().input_per_mtok_usd, 2.50);
        assert_eq!(pricing_for("models/gemini-2.5-flash").unwrap().output_per_mtok_usd, 2.50);
        assert_eq!(pricing_for("gemini-2.5-flash-lite").unwrap().output_per_mtok_usd, 0.40);
        assert!(pricing_for("llama3.2:3b").is_none());
        assert!(pricing_for("mock-smart").is_none());
    }

    #[test]
    fn cost_accounts_for_cache_and_reported_cost() {
        let usage =
            Usage { input_tokens: 1_000_000, output_tokens: 100_000, cached_input_tokens: 500_000, ..Usage::default() };
        let c = estimate_cost("claude-sonnet-4-5", &usage).unwrap();
        // 0.5M * 3 + 0.5M * 0.3 + 0.1M * 15 = 1.5 + 0.15 + 1.5
        assert!((c - 3.15).abs() < 1e-9, "{c}");
        let reported = Usage { reported_cost_usd: Some(0.42), ..usage };
        assert_eq!(estimate_cost("unknown-model", &reported), Some(0.42));
        assert_eq!(estimate_cost("unknown-model", &usage), None);
    }

    #[test]
    fn preflight() {
        let p = pricing_for("gpt-4o").unwrap();
        assert!((preflight_estimate(&p, 1_000_000, 1_000_000) - 12.5).abs() < 1e-9);
    }
}

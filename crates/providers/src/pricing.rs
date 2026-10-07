//! Static pricing table — **ESTIMATED** list prices in USD per 1M tokens.
//!
//! These values are a convenience snapshot for budget estimates and the cost
//! center (§22); they are not authoritative and may be outdated. Every cost
//! computed from them must be labelled "estimated" in the UI. The cost area
//! may override them with user-maintained prices.

use orkestr_domain::ModelPricing;

use crate::types::{CostModelKind, Usage};

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
    // Explicit rows per release: longest prefix wins, so every version that
    // differs from its family's generic row must be listed (e.g. Opus 4.6+
    // costs a third of Opus 4.0/4.1).
    ("claude-fable-5-1", 10.00, 50.00, Some(0.25)),
    ("claude-fable-5", 10.00, 50.00, Some(1.00)),
    ("claude-mythos-5-1", 10.00, 50.00, Some(0.25)),
    ("claude-opus-5-5", 4.00, 20.00, Some(0.20)),
    ("claude-opus-5", 5.00, 25.00, Some(0.50)),
    ("claude-opus-4-8", 5.00, 25.00, Some(0.50)),
    ("claude-opus-4-7", 5.00, 25.00, Some(0.50)),
    ("claude-opus-4-6", 5.00, 25.00, Some(0.50)),
    ("claude-opus-4-5", 5.00, 25.00, Some(0.50)),
    ("claude-opus-4-1", 15.00, 75.00, Some(1.50)),
    ("claude-opus-4-0", 15.00, 75.00, Some(1.50)),
    // Opus 4.0 snapshot ids (`claude-opus-4-20250514`).
    ("claude-opus-4", 15.00, 75.00, Some(1.50)),
    ("claude-sonnet-5-5", 2.00, 10.00, Some(0.20)),
    ("claude-sonnet-5", 2.00, 10.00, Some(0.20)),
    ("claude-sonnet-4-6", 3.00, 15.00, Some(0.30)),
    ("claude-sonnet-4", 3.00, 15.00, Some(0.30)),
    ("claude-haiku-4", 1.00, 5.00, Some(0.10)),
    ("claude-3-7-sonnet", 3.00, 15.00, Some(0.30)),
    ("claude-3-5-sonnet", 3.00, 15.00, Some(0.30)),
    ("claude-3-5-haiku", 0.80, 4.00, Some(0.08)),
    ("claude-3-haiku", 0.25, 1.25, Some(0.03)),
    // Family fallbacks for Claude releases newer than this table, so a new
    // model is estimated at its tier's price instead of silently costing $0.
    ("claude-fable", 10.00, 50.00, Some(1.00)),
    ("claude-mythos", 10.00, 50.00, Some(1.00)),
    ("claude-opus", 5.00, 25.00, Some(0.50)),
    ("claude-sonnet", 3.00, 15.00, Some(0.30)),
    ("claude-haiku", 1.00, 5.00, Some(0.10)),
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

/// Cost of a call split into real spend and API-equivalent value
/// (provider-adapter-spec §8): on a `Subscription` or `Local` instance the
/// marginal spend is $0 — a reported or list-price cost (e.g. Claude Code's
/// `total_cost_usd` on a Pro/Max login) is only what the call *would* cost
/// at API prices, kept for benchmarks and excluded from spend totals and
/// budgets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallCost {
    /// Spend in USD (budgets, caps, totals).
    pub spend_usd: f64,
    /// API-equivalent value of a call that is not metered in $.
    pub api_equivalent_usd: Option<f64>,
}

/// [`CallCost`] of a call on an instance with `cost_model` (`None` = the
/// adapter is unknown: treated as metered).
#[must_use]
pub fn call_cost(cost_model: Option<&CostModelKind>, model: &str, usage: &Usage) -> CallCost {
    let cost = estimate_cost(model, usage).filter(|c| c.is_finite() && *c >= 0.0).unwrap_or(0.0);
    match cost_model {
        Some(CostModelKind::Subscription | CostModelKind::Local) => {
            CallCost { spend_usd: 0.0, api_equivalent_usd: (cost > 0.0).then_some(cost) }
        }
        _ => CallCost { spend_usd: cost, api_equivalent_usd: None },
    }
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
    fn subscription_and_local_calls_are_not_spend() {
        // Claude Code on a Pro/Max login reports `total_cost_usd`: API-equivalent only.
        let usage = Usage { input_tokens: 10, output_tokens: 10, reported_cost_usd: Some(1.25), ..Usage::default() };
        let sub = call_cost(Some(&CostModelKind::Subscription), "claude-sonnet-4-5", &usage);
        assert_eq!(sub, CallCost { spend_usd: 0.0, api_equivalent_usd: Some(1.25) });
        let local = call_cost(Some(&CostModelKind::Local), "llama3.2:3b", &usage);
        assert_eq!(local.spend_usd, 0.0);
        let metered = call_cost(Some(&CostModelKind::PerToken), "claude-sonnet-4-5", &usage);
        assert_eq!(metered, CallCost { spend_usd: 1.25, api_equivalent_usd: None });
        assert_eq!(call_cost(None, "x", &usage).spend_usd, 1.25, "an unknown adapter is metered");
        let free = Usage { reported_cost_usd: Some(f64::NAN), ..usage };
        assert_eq!(call_cost(Some(&CostModelKind::PerToken), "x", &free).spend_usd, 0.0);
    }

    #[test]
    fn current_claude_models_are_priced_per_release() {
        let p = |m: &str| {
            let p = pricing_for(m).unwrap_or_else(|| panic!("{m} must be priced"));
            (p.input_per_mtok_usd, p.output_per_mtok_usd, p.cached_input_per_mtok_usd)
        };
        // Opus 4.6-4.8 must not fall back to the Opus 4.0 row (3x overbilling).
        for m in ["claude-opus-4-6", "claude-opus-4-7", "claude-opus-4-8", "claude-opus-5"] {
            assert_eq!(p(m), (5.00, 25.00, Some(0.50)), "{m}");
        }
        assert_eq!(p("claude-opus-5-5"), (4.00, 20.00, Some(0.20)));
        assert_eq!(p("claude-sonnet-5"), (2.00, 10.00, Some(0.20)));
        assert_eq!(p("claude-sonnet-5-5"), (2.00, 10.00, Some(0.20)));
        assert_eq!(p("claude-sonnet-4-6"), (3.00, 15.00, Some(0.30)));
        assert_eq!(p("claude-fable-5").0, 10.00);
        assert_eq!(p("claude-fable-5-1"), (10.00, 50.00, Some(0.25)));
        assert_eq!(p("claude-haiku-4-5"), (1.00, 5.00, Some(0.10)));
        // Opus 4.0/4.1 keep their old price.
        assert_eq!(p("claude-opus-4-1").0, 15.00);
        assert_eq!(p("claude-opus-4-0").0, 15.00);
        assert_eq!(p("claude-opus-4-20250514").0, 15.00);
        // A future release of a known family is estimated, never $0.
        assert_eq!(p("claude-opus-6").0, 5.00);
        assert_eq!(p("claude-sonnet-6-1").0, 3.00);
    }

    #[test]
    fn preflight() {
        let p = pricing_for("gpt-4o").unwrap();
        assert!((preflight_estimate(&p, 1_000_000, 1_000_000) - 12.5).abs() < 1e-9);
    }
}

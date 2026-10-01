//! # orkestr-cost — **SKELETON** (Phase 1–2 fill it in)
//!
//! Cost & Limit Center (MASTER_SPEC §22): usage records per
//! provider/model/agent/project/task, pricing, daily/monthly budgets
//! (Smart Budgeting), `BudgetThreshold` events, model benchmarking and agent
//! performance analytics.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{Budget, ProjectId, UsageRecord, UsageSummary};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum CostError {
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Result of a budget check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BudgetCheck {
    /// Within budget.
    Ok,
    /// Above the warning threshold (fraction spent).
    Warn(f64),
    /// Exceeded.
    Exceeded,
}

/// Usage ledger.
#[async_trait]
pub trait CostLedger: Send + Sync {
    /// Records one metered call.
    async fn record(&self, usage: &UsageRecord) -> Result<(), CostError>;
    /// Summary for a project in the current budget period.
    async fn project_summary(&self, project: ProjectId) -> Result<UsageSummary, CostError>;
}

/// Pure budget evaluation (USD only; tokens/time are checked by their owners).
#[must_use]
pub fn check_budget(budget: &Budget, spent: &UsageSummary) -> BudgetCheck {
    let Some(limit) = budget.max_cost_usd else { return BudgetCheck::Ok };
    if limit <= 0.0 || spent.cost_usd >= limit {
        return BudgetCheck::Exceeded;
    }
    let fraction = spent.cost_usd / limit;
    if fraction >= budget.warn_at {
        BudgetCheck::Warn(fraction)
    } else {
        BudgetCheck::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::BudgetPeriod;

    #[test]
    fn budget_evaluation() {
        let mut b = Budget::unrestricted(BudgetPeriod::Monthly);
        let spent = UsageSummary { input_tokens: 0, output_tokens: 0, cost_usd: 90.0 };
        assert_eq!(check_budget(&b, &spent), BudgetCheck::Ok);
        b.max_cost_usd = Some(100.0);
        assert!(matches!(check_budget(&b, &spent), BudgetCheck::Warn(f) if (f - 0.9).abs() < 1e-9));
        b.max_cost_usd = Some(50.0);
        assert_eq!(check_budget(&b, &spent), BudgetCheck::Exceeded);
    }
}

//! Small value objects shared by many entities.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{AgentId, ProviderId, UserId};

/// UTC timestamp, serialized as RFC 3339.
pub type Timestamp = DateTime<Utc>;

/// Returns the current UTC time. Engines that need deterministic time inject
/// their own clock; this helper exists for constructors and tests.
#[must_use]
pub fn now() -> Timestamp {
    Utc::now()
}

/// Who performed an action. Used for audit, approvals, snapshots, decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "id")]
#[ts(export)]
pub enum Actor {
    /// A human user.
    User(UserId),
    /// An agent instance.
    Agent(AgentId),
    /// The AI Manager / orchestrator.
    Manager,
    /// The system itself (scheduler, recovery, automation).
    System,
}

/// Reference to a concrete model of a provider, e.g. `(anthropic-1, "claude-opus")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelRef {
    /// Provider instance.
    pub provider_id: ProviderId,
    /// Provider-specific model identifier.
    pub model: String,
}

/// Period a budget applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum BudgetPeriod {
    /// Per individual task.
    PerTask,
    /// Calendar day (local time of the core).
    Daily,
    /// Calendar month.
    Monthly,
    /// Lifetime of the owner (project/agent).
    Total,
}

/// Spend/usage limits (§22 Smart Budgeting). `None` means unrestricted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Budget {
    /// Maximum spend in USD.
    pub max_cost_usd: Option<f64>,
    /// Maximum total tokens.
    pub max_tokens: Option<u64>,
    /// Maximum wall-clock duration in seconds.
    pub max_duration_secs: Option<u64>,
    /// Period the limits apply to.
    pub period: BudgetPeriod,
    /// Fraction (0..1) at which a `BudgetThreshold` warning is emitted.
    pub warn_at: f64,
}

impl Budget {
    /// An explicit "no limits" budget.
    #[must_use]
    pub fn unrestricted(period: BudgetPeriod) -> Self {
        Self { max_cost_usd: None, max_tokens: None, max_duration_secs: None, period, warn_at: 0.8 }
    }

    /// True when no limit is configured.
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        self.max_cost_usd.is_none() && self.max_tokens.is_none() && self.max_duration_secs.is_none()
    }
}

/// Accumulated token usage and cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UsageSummary {
    /// Prompt/input tokens.
    pub input_tokens: u64,
    /// Completion/output tokens.
    pub output_tokens: u64,
    /// Cost in USD (may be estimated).
    pub cost_usd: f64,
}

impl UsageSummary {
    /// Adds another summary to this one.
    pub fn add(&mut self, other: &UsageSummary) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd += other.cost_usd;
    }

    /// Total tokens.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// A git branch + worktree assigned to an agent or task (§16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorktreeRef {
    /// Branch name, e.g. `agent/backend`.
    pub branch: String,
    /// Absolute path of the worktree on the owning node.
    pub path: String,
}

/// Reference to an artifact stored in the content-addressed object store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArtifactRef {
    /// Display name, e.g. `coverage.html`.
    pub name: String,
    /// blake3 hex digest of the stored object.
    pub object_id: String,
    /// MIME type.
    pub mime: String,
    /// Size in bytes.
    pub size_bytes: u64,
}

/// Risk classification for actions and approvals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RiskLevel {
    /// Read-only or trivially reversible.
    Low,
    /// Writes inside a sandboxed worktree.
    Medium,
    /// Destructive or externally visible.
    High,
    /// Production, secrets, irreversible.
    Critical,
}

/// Operating system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum OsKind {
    /// Microsoft Windows.
    Windows,
    /// Apple macOS.
    MacOs,
    /// Linux.
    Linux,
    /// Anything else.
    Other,
}

/// CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Arch {
    /// x86-64.
    X64,
    /// ARM64 / AArch64.
    Arm64,
    /// Anything else.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_serializes_tagged() {
        let id = UserId::new();
        let v = serde_json::to_value(Actor::User(id)).unwrap();
        assert_eq!(v["type"], "User");
        assert_eq!(v["id"], id.to_string());
        let v = serde_json::to_value(Actor::Manager).unwrap();
        assert_eq!(v, serde_json::json!({"type": "Manager"}));
    }

    #[test]
    fn usage_adds() {
        let mut a = UsageSummary { input_tokens: 1, output_tokens: 2, cost_usd: 0.5 };
        a.add(&UsageSummary { input_tokens: 10, output_tokens: 20, cost_usd: 1.0 });
        assert_eq!(a.total_tokens(), 33);
        assert!((a.cost_usd - 1.5).abs() < f64::EPSILON);
    }
}

//! Usage records (§22 Cost & Limit Center).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::Timestamp;
use crate::ids::{AgentId, ExecutionId, ProjectId, ProviderId, TaskId, UsageRecordId};

/// One metered model call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UsageRecord {
    /// Id.
    pub id: UsageRecordId,
    /// Time of the call.
    pub ts: Timestamp,
    /// Provider.
    pub provider_id: ProviderId,
    /// Model id.
    pub model: String,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Agent.
    pub agent_id: Option<AgentId>,
    /// Execution.
    pub execution_id: Option<ExecutionId>,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Reasoning tokens (if reported).
    pub reasoning_tokens: u64,
    /// Cost in USD.
    pub cost_usd: f64,
    /// True when cost is estimated from list prices (subscription/CLI usage).
    pub estimated: bool,
    /// Latency to first token in ms.
    pub latency_ms: Option<u64>,
    /// Whether the call succeeded.
    pub success: bool,
}

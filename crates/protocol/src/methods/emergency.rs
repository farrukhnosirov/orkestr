//! `emergency.*` — emergency controls (§52). Handled locally in core,
//! independent of providers and network.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId};

use super::method;
use super::secrets::VaultStatusResult;

/// Emergency flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmergencyFlagsView {
    /// Stop All AI active (until `emergency.reset`).
    pub all_stopped: bool,
    /// FullAuto capped at SemiAuto.
    pub full_auto_disabled: bool,
    /// Network/Browser tools denied to agents.
    pub network_tools_disabled: bool,
    /// Vault locked.
    pub vault_locked: bool,
}

/// Status result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmergencyStatusResult {
    /// Flags.
    pub flags: EmergencyFlagsView,
    /// Live registered agent tokens.
    pub active_agents: u32,
}

/// Result of a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmergencyStopResult {
    /// Agents stopped.
    pub agents_stopped: u32,
    /// Other work (project/global registrations) stopped.
    pub other_stopped: u32,
    /// Pending approvals cancelled.
    pub approvals_cancelled: u32,
    /// Flags after the stop.
    pub flags: EmergencyFlagsView,
}

/// Params of `emergency.stopProject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StopProjectParams {
    /// Project.
    pub project_id: ProjectId,
}

/// Params of `emergency.stopAgent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StopAgentParams {
    /// Agent.
    pub agent_id: AgentId,
}

/// Params of the toggle controls (default `disabled: true`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToggleParams {
    /// `true` disables (default), `false` re-enables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub disabled: Option<bool>,
}

method!(
    /// `emergency.stopAll` — emits `EmergencyStop{All}`.
    StopAll, "emergency.stopAll", super::Empty, EmergencyStopResult
);
method!(
    /// `emergency.stopProject` — emits `EmergencyStop{Project}`.
    StopProject, "emergency.stopProject", StopProjectParams, EmergencyStopResult
);
method!(
    /// `emergency.stopAgent` — emits `EmergencyStop{Agent}`.
    StopAgent, "emergency.stopAgent", StopAgentParams, EmergencyStopResult
);
method!(
    /// `emergency.disableFullAuto`.
    DisableFullAuto, "emergency.disableFullAuto", ToggleParams, EmergencyStatusResult
);
method!(
    /// `emergency.disableNetworkTools`.
    DisableNetworkTools, "emergency.disableNetworkTools", ToggleParams, EmergencyStatusResult
);
method!(
    /// `emergency.lockVault` — alias of `secrets.lock` (name reserved in Phase 0).
    LockVault, "emergency.lockVault", super::Empty, VaultStatusResult
);
method!(
    /// `emergency.status`.
    Status, "emergency.status", super::Empty, EmergencyStatusResult
);
method!(
    /// `emergency.reset` — re-arms (clears Stop All / FullAuto / network flags; vault stays locked).
    Reset, "emergency.reset", super::Empty, EmergencyStatusResult
);

//! `claudeEngine.*` — Claude Code (the user's subscription, through the
//! official `claude` CLI) as Orkestr's engine: the auto-registered provider,
//! the one-click "use Claude Code for everything" routing, and Claude Code's
//! own sub-agents (built in + `.claude/agents/*.md`) as Orkestr agents.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentTemplateId, ManagerConfig, ProjectId, ProviderId, RoutingConfig};

use super::method;

/// Where a Claude Code agent is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClaudeAgentScope {
    /// Built into the CLI (reported by its `initialize` answer).
    BuiltIn,
    /// `~/.claude/agents/<name>.md`.
    User,
    /// `<project>/.claude/agents/<name>.md`.
    Project,
}

/// One Claude Code sub-agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeAgentDef {
    /// Name (`code-reviewer`), unique per scope.
    pub name: String,
    /// When Claude should use it.
    pub description: String,
    /// Allowed tools (empty = all tools of the session).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Model alias (`sonnet`, `opus`, `haiku`, `inherit`).
    #[serde(default)]
    pub model: Option<String>,
    /// System prompt (the markdown body; empty for built-ins).
    #[serde(default)]
    pub prompt: String,
    /// Scope.
    pub scope: ClaudeAgentScope,
    /// Definition file (user / project scope).
    #[serde(default)]
    pub path: Option<String>,
    /// Orkestr template that spawns it (after `claudeEngine.agents`).
    #[serde(default)]
    pub template_id: Option<AgentTemplateId>,
}

/// State of the Claude Code engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeEngineStatus {
    /// The binary was found.
    pub installed: bool,
    /// The CLI reports a signed-in account.
    pub signed_in: bool,
    /// Subscription type (`max`, `pro`) when known.
    #[serde(default)]
    pub subscription: Option<String>,
    /// The registered provider (auto-registered once installed + signed in).
    #[serde(default)]
    pub provider_id: Option<ProviderId>,
    /// The AI Manager runs on Claude Code.
    pub manager_on_claude: bool,
    /// Agent roles routed to Claude Code (routing rules).
    pub roles_on_claude: u32,
    /// Why the provider could not be registered, if so.
    #[serde(default)]
    pub reason: Option<String>,
    /// The saved Claude Code model alias (`sonnet`, `opus`, `haiku`): the
    /// Manager's model when it runs on Claude Code, else the provider's
    /// default model. Survives restarts; the card's «Model» shows it.
    #[serde(default)]
    pub model: String,
}

/// Params of `claudeEngine.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeEngineStatusParams {
    /// Re-probe the binary / login and register the provider when possible.
    #[serde(default)]
    pub refresh: bool,
}

/// Params of `claudeEngine.useForEverything`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UseClaudeForEverythingParams {
    /// Model alias for the Manager and the agents (default `sonnet`).
    #[serde(default)]
    pub model: Option<String>,
}

/// Params of `claudeEngine.setModel`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetClaudeModelParams {
    /// Model alias (`sonnet`, `opus`, `haiku`, or a full model id).
    pub model: String,
}

/// Result of `claudeEngine.useForEverything`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UseClaudeForEverythingResult {
    /// Engine state afterwards.
    pub status: ClaudeEngineStatus,
    /// The new AI Manager settings (previous model kept as first fallback).
    pub manager: ManagerConfig,
    /// The new routing (every role → Claude Code, previous model as fallback).
    pub routing: RoutingConfig,
}

/// Params of `claudeEngine.agents`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeAgentsParams {
    /// Project whose `.claude/agents` to include.
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// Re-ask the CLI for its built-in agents.
    #[serde(default)]
    pub refresh: bool,
}

/// Result of `claudeEngine.agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeAgentsResult {
    /// Agents (project first, then user, then built in).
    pub agents: Vec<ClaudeAgentDef>,
    /// Why the built-in list is missing, if so.
    #[serde(default)]
    pub builtin_error: Option<String>,
}

/// Params of `claudeEngine.saveAgent` (creates or overwrites the `.md` file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveClaudeAgentParams {
    /// Project (required for the project scope).
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// `user` or `project`.
    pub scope: ClaudeAgentScope,
    /// Name (`[a-z0-9-]`, file name `<name>.md`).
    pub name: String,
    /// Description.
    pub description: String,
    /// Tools (empty = all).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Model alias.
    #[serde(default)]
    pub model: Option<String>,
    /// System prompt.
    pub prompt: String,
    /// The user confirmed writing the file (required).
    pub confirmed: bool,
}

/// Params of `claudeEngine.deleteAgent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteClaudeAgentParams {
    /// Project (required for the project scope).
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// `user` or `project`.
    pub scope: ClaudeAgentScope,
    /// Name.
    pub name: String,
    /// The user confirmed deleting the file (required).
    pub confirmed: bool,
}

/// Result of `claudeEngine.saveAgent` / `claudeEngine.deleteAgent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeAgentFileResult {
    /// The file written / removed.
    pub path: String,
    /// The agent (save only).
    #[serde(default)]
    pub agent: Option<ClaudeAgentDef>,
}

method!(
    /// `claudeEngine.status` — binary / login / provider / routing state.
    Status, "claudeEngine.status", ClaudeEngineStatusParams, ClaudeEngineStatus
);
method!(
    /// `claudeEngine.useForEverything` — Manager, every agent role, review,
    /// consensus and PM on Claude Code (previous models kept as fallbacks).
    UseForEverything, "claudeEngine.useForEverything", UseClaudeForEverythingParams, UseClaudeForEverythingResult
);
method!(
    /// `claudeEngine.setModel` — saves the Claude Code model: the provider's
    /// default, every Manager / routing rule already on Claude Code and the
    /// parallel engines' Claude model.
    SetModel, "claudeEngine.setModel", SetClaudeModelParams, ClaudeEngineStatus
);
method!(
    /// `claudeEngine.agents` — Claude Code's sub-agents, synced as templates.
    Agents, "claudeEngine.agents", ClaudeAgentsParams, ClaudeAgentsResult
);
method!(
    /// `claudeEngine.saveAgent` — writes `.claude/agents/<name>.md`.
    SaveAgent, "claudeEngine.saveAgent", SaveClaudeAgentParams, ClaudeAgentFileResult
);
method!(
    /// `claudeEngine.deleteAgent` — removes `.claude/agents/<name>.md`.
    DeleteAgent, "claudeEngine.deleteAgent", DeleteClaudeAgentParams, ClaudeAgentFileResult
);

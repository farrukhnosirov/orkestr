//! # orkestr-agent-runtime — **SKELETON** (Phase 1 fills it in)
//!
//! Responsibility (MASTER_SPEC §6, §30, §57; contract §5–§7):
//!
//! * Agent template → instance lifecycle (`spawn`, `start`, `stop`), state
//!   transitions (`Idle … Stopped`) published as `AgentStateChanged` events
//!   (floating robots §30 bind to `AgentId`).
//! * The **tool loop**: model output → tool calls → `PermissionEngine::check`
//!   → execute or request approval → results back to the model, bounded by
//!   `max_turns`, budget and time (§17 "always max attempts/time/budget").
//! * [`ToolRegistry`] of [`Tool`]s (fs, terminal, git, browser, mcp, …).
//! * Structured [`Handoff`] between agents (§57) — never the full chat log.
//! * Cancellation via `CancellationToken` for emergency stop (§52).
//!
//! Agent runtime is provider-agnostic: it talks to
//! `orkestr-providers::ProviderAdapter` only (§60.5).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{AgentId, AgentInstance, AgentTemplateId, Capability, ProjectId, TaskId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Errors of the agent runtime.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Unknown agent.
    #[error("agent {0} not found")]
    NotFound(AgentId),
    /// Tool failure.
    #[error("tool `{tool}` failed: {message}")]
    Tool {
        /// Tool name.
        tool: String,
        /// Message.
        message: String,
    },
    /// Permission denied.
    #[error("denied: {0}")]
    Denied(String),
    /// Limit reached (turns/budget/time).
    #[error("limit reached: {0}")]
    LimitReached(String),
    /// Cancelled (user or emergency stop).
    #[error("cancelled")]
    Cancelled,
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Invocation context passed to tools.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Calling agent.
    pub agent_id: AgentId,
    /// Project.
    pub project_id: ProjectId,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Confinement root (agent worktree).
    pub root: std::path::PathBuf,
    /// Cancellation.
    pub cancel: CancellationToken,
}

/// A tool usable by agents.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name (`fs.read`, `terminal.run`).
    fn name(&self) -> &'static str;
    /// Human description for the model.
    fn description(&self) -> &'static str;
    /// Capability checked before every call.
    fn capability(&self) -> Capability;
    /// JSON schema of the input.
    fn input_schema(&self) -> serde_json::Value;
    /// Executes the tool.
    async fn call(&self, ctx: &ToolContext, input: serde_json::Value) -> Result<serde_json::Value, AgentError>;
}

/// Registry of tools.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<std::sync::Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.tools.iter().map(|t| t.name())).finish()
    }
}

impl ToolRegistry {
    /// Registers a tool (replacing one with the same name).
    pub fn register(&mut self, tool: std::sync::Arc<dyn Tool>) {
        self.tools.retain(|t| t.name() != tool.name());
        self.tools.push(tool);
    }

    /// Looks up a tool.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<std::sync::Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Tool names.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}

/// Structured handoff between agents (§57).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    /// Task.
    pub task_id: TaskId,
    /// Concise context.
    pub summary: String,
    /// Relevant files.
    pub relevant_files: Vec<String>,
    /// Decisions taken.
    pub decisions: Vec<String>,
    /// Open issues.
    pub unresolved: Vec<String>,
    /// Test state summary.
    pub test_state: Option<String>,
    /// Artifact object ids.
    pub artifacts: Vec<String>,
}

/// Agent lifecycle management.
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    /// Spawns an instance of a template for a project/task.
    async fn spawn(
        &self,
        template: AgentTemplateId,
        project: ProjectId,
        task: Option<TaskId>,
    ) -> Result<AgentInstance, AgentError>;
    /// Stops an agent (cooperative cancellation, then hard stop).
    async fn stop(&self, agent: AgentId) -> Result<(), AgentError>;
    /// Stops every agent (emergency stop §52). Returns the number stopped.
    async fn stop_all(&self) -> Result<u32, AgentError>;
    /// Lists live agents.
    async fn list(&self) -> Result<Vec<AgentInstance>, AgentError>;
}

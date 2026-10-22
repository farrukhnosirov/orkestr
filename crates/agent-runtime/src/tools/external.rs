//! External tools: tools contributed at runtime by code plugins (Phase 7,
//! ADR-0034). The core installs one [`ExternalTools`] implementation
//! (`core/src/plugin_tools.rs`, WP1) with
//! [`crate::AgentRuntime::set_external_tools`]; the run loop consults it when
//! building the tool list and dispatches calls of unknown built-in names to it
//! (wiring in `run.rs`: WP1).
//!
//! External tool names are prefixed `plugin_` so they never collide with the
//! built-in [`super::TOOL_NAMES`]; every call is permission-checked by the
//! plugin host (grants of principal `plugin:<id>`) **and** by the agent's own
//! `PermissionEngine` check for the declared capability.

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{AgentId, Capability, ProjectId, TaskId};
use parking_lot::RwLock;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Name prefix of every external tool.
pub const EXTERNAL_TOOL_PREFIX: &str = "plugin_";

/// One external tool as shown to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalToolDecl {
    /// Tool name (`plugin_<name>`, `^[a-z][a-z0-9_]{0,63}$`).
    pub name: String,
    /// Description shown to the model.
    pub description: String,
    /// JSON schema of the arguments.
    pub input_schema: Value,
    /// Capability the agent needs to call it (checked by the run loop).
    pub capability: Option<Capability>,
}

/// Context of one external tool call.
#[derive(Debug, Clone)]
pub struct ToolCallCtx {
    /// Project.
    pub project_id: ProjectId,
    /// Calling agent.
    pub agent_id: AgentId,
    /// Task, if any.
    pub task_id: Option<TaskId>,
    /// Cancelled when the agent stops (emergency stop included).
    pub cancel: CancellationToken,
}

/// Provider of external tools.
#[async_trait]
pub trait ExternalTools: Send + Sync {
    /// Tools available to agents of `project`.
    fn list(&self, project: &ProjectId) -> Vec<ExternalToolDecl>;
    /// Calls `tool` (full `plugin_*` name). Errors are shown to the model.
    async fn call(&self, tool: &str, input: Value, ctx: ToolCallCtx) -> Result<Value, String>;
}

/// Shared, late-bound slot for the [`ExternalTools`] provider (part of
/// [`crate::RuntimeConfig`], so tests and older call sites need no change).
#[derive(Clone, Default)]
pub struct ExternalToolsSlot(Arc<RwLock<Option<Arc<dyn ExternalTools>>>>);

impl std::fmt::Debug for ExternalToolsSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalToolsSlot").field("installed", &self.0.read().is_some()).finish()
    }
}

impl ExternalToolsSlot {
    /// Installs (or replaces) the provider.
    pub fn set(&self, tools: Arc<dyn ExternalTools>) {
        *self.0.write() = Some(tools);
    }

    /// The provider, if installed.
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn ExternalTools>> {
        self.0.read().clone()
    }
}

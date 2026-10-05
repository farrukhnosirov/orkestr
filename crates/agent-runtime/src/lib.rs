//! # orkestr-agent-runtime
//!
//! Agent Runtime (MASTER_SPEC §6, §30, §57, §64; `docs/agents/agent-runtime-spec.md`):
//!
//! * [`templates`] — agent templates (all §6 fields), 11 built-in official
//!   templates shipped as JSON resources, CRUD, import/export envelope.
//! * [`state`] — the agent instance state machine (`Idle … Stopped`).
//! * [`runtime::AgentRuntime`] — spawn / stop / pause / resume / list /
//!   transcript / handoffs / stats. Each running agent is its **own tokio
//!   task** with its own `CancellationToken` registered in the
//!   `EmergencyController`, its own worktree (`agent/<role>-<shortid>` via
//!   `orkestr-vcs`, or shared / read-only / none), its own conversation and
//!   budget / turn / wall-clock limits.
//! * [`run`] — the tool loop over the provider router (context bundle →
//!   chat with tools → permission-checked tool execution → results → … until
//!   `submit_result`, limits or cancellation), automatic checkpoint before the
//!   first change, usage → cost engine, provider fallback with structured
//!   handoff, task transitions through the task engine.
//! * [`cli`] — supervision of official CLI coding agents (Claude Code, Codex).
//! * [`tools`] — the Phase 1 tool set (`read_file`, `write_file`,
//!   `edit_file`, `list_dir`, `search_files`, `run_command`, `git_status`,
//!   `git_diff`, `git_commit`, `create_snapshot`, `ask_user`,
//!   `record_memory`, `record_decision`, `submit_result`).
//! * [`handoff`] — structured handoffs (§57).
//!
//! Hidden chain-of-thought is never stored: `ChatEvent` has no reasoning
//! variant and only visible text, tool calls (redacted) and results are
//! persisted (§1.14, §21).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
pub mod deps;
pub mod error;
pub mod gateway;
pub mod handoff;
pub mod language;
pub mod prompt;
pub mod run;
pub mod runtime;
pub mod schema;
pub mod state;
pub mod store;
pub mod templates;
pub mod tools;

pub use deps::{RuntimeConfig, RuntimeDeps};
pub use error::{AgentError, AgentResult};
pub use gateway::{ModelGateway, ProviderInfo, RegistryGateway};
pub use language::{reply_language_instruction, UiLanguage};
pub use runtime::{AgentRuntime, SpawnExtras};
pub use schema::agent_migrations;
pub use templates::TemplateService;

#[cfg(test)]
mod tests;

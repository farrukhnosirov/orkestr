//! # orkestr-orchestrator — the AI Manager
//!
//! MASTER_SPEC §1, §5, §9, §11, §53, §57:
//!
//! * [`chat`] — one conversation per project, persisted (user / Manager /
//!   system-event messages + extractive summaries). A reply is a bounded tool
//!   loop over the model chosen in the AI Manager settings; text streams as
//!   `ManagerMessage` events. Only visible output is kept — never hidden
//!   chain-of-thought.
//! * [`tools`] — the Manager sees the real state through read-only,
//!   permission-checked state tools and acts through action tools
//!   (`propose_plan`, `create_tasks`, `spawn_agent`, `stop_agent`,
//!   `request_approval`, `record_decision`, `answer_user`).
//! * [`planner`] + [`plan`] — study the project (memory, file index, git,
//!   open tasks) → structured plan (JSON schema with provider structured
//!   output, or robust extraction + one repair round) → normalise, validate,
//!   estimate cost and parallelism → plan card; approval creates the tasks
//!   with dependencies in the task engine.
//! * [`run`] — a supervised, bounded, restart-safe run per plan: scheduler
//!   admission (parallel caps, provider slots, CPU/RAM, file overlap), one
//!   isolated agent per ready task (own worktree based on the prerequisite's
//!   branch), result acceptance by approval policy, bounded retries, pause /
//!   resume / cancel, emergency stop, final report + merge proposals.
//! * [`resume`] — "Last session" summary (§57).
//!
//! The orchestrator composes engines through their public APIs; it is not a
//! god service (§64).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod chat;
pub mod deps;
pub mod error;
pub mod plan;
pub mod planner;
pub mod pm;
pub mod resume;
pub mod run;
pub mod schema;
pub mod service;
pub mod store;
pub mod structured;
pub mod team_lead;
pub mod teams;
pub mod tools;
pub mod util;

pub use deps::{
    ManagerHub, ManagerTunables, OrchestratorDeps, ProviderBrief, RegistryHub, ResourceProbe, Unconstrained,
};
pub use error::{OrchResult, OrchestratorError};
pub use schema::orchestrator_migrations;
pub use service::Manager;

#[cfg(test)]
mod tests;

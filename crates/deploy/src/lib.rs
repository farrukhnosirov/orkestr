//! Orkestr deployments (Phase 7, ADR-0041). **Owner: WP4.**

#![forbid(unsafe_code)]

pub mod adapter;
pub mod adapters;
pub mod environments;
pub mod error;
pub mod logs;
pub mod service;
pub mod store;

pub use adapter::{Credentials, DeployAdapter, DeployOutcome, DeployRequest, LogSink, RollbackTarget};
pub use error::DeployError;
pub use orkestr_protocol::methods::deploy::{AdapterAvailability, DeployPlan};
pub use service::{DeployDeps, DeployService};
pub use store::{migrations as deploy_migrations, RunKind, StoredDeployment, MIGRATIONS};

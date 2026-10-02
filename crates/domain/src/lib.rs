//! # orkestr-domain
//!
//! The Orkestr domain model (MASTER_SPEC §51): identifiers, entities, enums and
//! value objects shared by every engine and — via `ts-rs` — by the TypeScript
//! clients (`@orkestr/protocol`).
//!
//! Rules for this crate (ENGINEERING_CONTRACT §2):
//! * **Pure data.** No I/O, no async, no OS-specific code. Small pure helpers
//!   (e.g. [`PrivacyPolicy::allows_provider`]) are fine.
//! * Every type that crosses the wire derives `Serialize`, `Deserialize` and
//!   `TS` with `#[serde(rename_all = "camelCase")]` and `#[ts(export)]`.
//! * Ids are UUID v7 newtypes (see [`ids`]), serialized as plain strings.
//! * Timestamps are `chrono::DateTime<Utc>` serialized as RFC 3339 strings.
//! * Enum variants serialize as their PascalCase Rust name (e.g. `"Running"`),
//!   which is also how they appear in TypeScript string-literal unions.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod agent;
pub mod approval;
pub mod audit;
pub mod capability;
pub mod common;
pub mod environment;
pub mod execution;
pub mod ids;
pub mod memory;
pub mod node;
pub mod notification;
pub mod plugin;
pub mod project;
pub mod provider;
pub mod routing;
pub mod secret;
pub mod task;
pub mod usage;
pub mod user;

pub use agent::*;
pub use approval::*;
pub use audit::*;
pub use capability::*;
pub use common::*;
pub use environment::*;
pub use execution::*;
pub use ids::*;
pub use memory::*;
pub use node::*;
pub use notification::*;
pub use plugin::*;
pub use project::*;
pub use provider::*;
pub use routing::*;
pub use secret::*;
pub use task::*;
pub use usage::*;
pub use user::*;

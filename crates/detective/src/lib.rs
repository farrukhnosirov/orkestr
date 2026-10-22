//! Orkestr Bug Detective (Phase 7, ADR-0042): incident stage machine,
//! evidence collectors, structured root-cause hypothesis, reproduction, an
//! agent fix on a `detective/<id>` worktree, tests, review and two human
//! gates, under a per-incident budget cap.

#![forbid(unsafe_code)]

pub mod collectors;
pub mod error;
pub mod evidence;
pub mod hypothesis;
pub mod ports;
pub mod service;
pub mod stacktrace;
pub mod stage;
pub mod store;
pub mod vcs_git;

pub use error::DetectiveError;
pub use evidence::{EvidenceBudget, EvidenceCollector};
pub use orkestr_protocol::methods::detective::{Evidence, EvidenceKind, Incident, IncidentStage};
pub use ports::Ports;
pub use service::{Detective, DetectiveDeps};
pub use store::{migrations as detective_migrations, DetectiveSettings, MIGRATIONS};
pub use vcs_git::GitVcs;

//! Orkestr Bug Detective (Phase 7, ADR-0042). **Owner: WP6.**

#![forbid(unsafe_code)]

pub mod collectors;
pub mod error;
pub mod evidence;
pub mod hypothesis;
pub mod service;
pub mod stage;
pub mod store;

pub use error::DetectiveError;
pub use evidence::{EvidenceBudget, EvidenceCollector};
pub use orkestr_protocol::methods::detective::{Evidence, EvidenceKind, Incident, IncidentStage};
pub use service::{Detective, DetectiveDeps};
pub use store::{migrations as detective_migrations, MIGRATIONS};

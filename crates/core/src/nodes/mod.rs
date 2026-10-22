//! Multi-node coordinator + worker plumbing (Phase 6, ADR-0033).
//!
//! Module ownership (Phase 6 work packages):
//! * WP-A: [`registry_store`] (SQLite registry + enrollments), [`connector`]
//!   (gateway core-to-core E2E links), [`heartbeat`] (pull heartbeat loop +
//!   health state machine);
//! * WP-C: [`worker`] (worker-side job host), [`lease`] (epochs, fencing,
//!   expiry), [`coordinator_jobs`] (offer/accept, progress intake),
//!   [`migration`] (planned + unplanned migration, side-effect gate);
//! * shared test seam: [`loopback::LoopbackLink`] (a worker router in-process).

pub mod connector;
pub mod coordinator_jobs;
pub mod heartbeat;
pub mod lease;
pub mod loopback;
pub mod migration;
pub mod registry_store;
pub mod worker;

pub use loopback::LoopbackLink;

//! # orkestr-events
//!
//! Event-driven backbone of the core (§49, §50, contract §4):
//!
//! * [`EventBus`] — assigns monotonic `seq` numbers, persists events through an
//!   [`EventLog`] and broadcasts them (tokio `broadcast`) to topic-filtered
//!   [`Subscription`]s.
//! * [`EventLog`] / [`SqliteEventLog`] — durable, append-only log with range
//!   queries used by `events.replay` and Execution Replay (§21).
//! * [`correlation`] — ambient correlation ids for tracing one user request
//!   through Manager, agents and tools.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bus;
pub mod correlation;
pub mod log;

pub use bus::{EventBus, Received, Subscription, DEFAULT_CAPACITY};
pub use correlation::{current_correlation, new_correlation, with_correlation};
pub use log::{events_migrations, EventLog, MemoryEventLog, ReplayPage, ReplayQuery, SqliteEventLog};

/// Errors from the event subsystem.
#[derive(Debug, thiserror::Error)]
pub enum EventError {
    /// Persistence failed; the event was **not** published.
    #[error("event log: {0}")]
    Log(#[from] orkestr_storage::StorageError),
    /// Invalid query.
    #[error("invalid: {0}")]
    Invalid(String),
}

/// Result alias.
pub type EventResult<T> = Result<T, EventError>;

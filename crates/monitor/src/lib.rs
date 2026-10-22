//! Orkestr infrastructure monitor (Phase 7, ADR-0042). **Owner: WP5.**

#![forbid(unsafe_code)]

pub mod alerts;
pub mod checks;
pub mod error;
pub mod logs;
pub mod metrics;
pub mod service;
pub mod store;

pub use error::MonitorError;
pub use logs::LogSource;
pub use orkestr_protocol::methods::monitor::{LogLine, LogQuery};
pub use service::{MonitorDeps, MonitorService};
pub use store::{migrations as monitor_migrations, MIGRATIONS};

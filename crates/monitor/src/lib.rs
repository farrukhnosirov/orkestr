//! Orkestr infrastructure monitor (Phase 7, ADR-0042). **Owner: WP5.**
//!
//! * [`metrics`] — per-node ring buffers (local `NodeMonitor` sampler +
//!   [`metrics::NodeMetricsSource`] for enrolled nodes);
//! * [`checks`] — HTTP/TCP probes with up/down hysteresis;
//! * [`logs`] — [`LogSource`] implementations (file tail, deployment logs via
//!   [`logs::DeploymentLogFeed`], `monitor.ingest`), redacted before storage
//!   and broadcast, non-blocking live tail;
//! * [`alerts`] — rules (check down, metric threshold, log rate / pattern)
//!   with dedup and `AlertRaised` / `AlertResolved`;
//! * [`service`] — [`MonitorService`], including the evidence API used by the
//!   Bug Detective (see the module docs).

#![forbid(unsafe_code)]

pub mod alerts;
pub mod checks;
pub mod error;
pub mod logs;
pub mod metrics;
pub mod service;
pub mod store;

pub use alerts::{AlertRule, RuleKind};
pub use error::MonitorError;
pub use logs::LogSource;
pub use logs::{DeploymentLogFeed, DeploymentRef, TailReceiver};
pub use metrics::NodeMetricsSource;
pub use orkestr_protocol::methods::monitor::{LogLine, LogQuery};
pub use service::{MonitorConfig, MonitorDeps, MonitorService};
pub use store::{migrations as monitor_migrations, MIGRATIONS};

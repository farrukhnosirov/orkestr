//! # orkestr-insight — Architecture Map and Execution Replay (§21)
//!
//! * [`arch`] — scans a project root and builds an [`ArchitectureMap`]:
//!   modules / packages (Cargo workspaces, npm workspaces, Python, Go, .NET,
//!   plain directories), services (binaries, Dockerfiles, docker-compose),
//!   databases (compose images, SQLite usage, migration folders, ORM
//!   schemas), HTTP APIs (route declarations of axum / actix / express /
//!   fastapi / flask / ASP.NET / gin / net/http, OpenAPI documents) and
//!   dependencies (internal edges from manifests and imports, external
//!   dependencies grouped per ecosystem). Scans are incremental (a per-file
//!   fact cache keyed by size + mtime), snapshots are persisted and diffed,
//!   and [`arch::spawn_auto_refresh`] refreshes maps after `FileChanged`
//!   events (debounced, rate-limited).
//! * [`replay`] — rebuilds the timeline of a Manager run or a task from the
//!   persisted event log (request → plan → agents → delegation → tools →
//!   files → tests → reviews → approvals → merge) and reconstructs the state
//!   at any step. Only persisted, visible output is used; hidden
//!   chain-of-thought is never stored, so it cannot be replayed.
//!
//! [`ArchitectureMap`]: orkestr_protocol::methods::architecture::ArchitectureMap
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod arch;
pub mod error;
pub mod replay;

pub use arch::{insight_migrations, ArchitectureService, RefreshOutcome, ScanOptions};
pub use error::{InsightError, InsightResult};

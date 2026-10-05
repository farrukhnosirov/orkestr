//! # orkestr-quality
//!
//! Automatic testing and quality gates (MASTER_SPEC §17, §58):
//!
//! * [`detect`] — project toolchain detection from manifests (Cargo,
//!   npm/pnpm/yarn, Python, Go, .NET, Maven/Gradle, Makefile; monorepo
//!   aware) → proposed build/test/lint/typecheck/format/E2E/smoke checks.
//! * [`QualityService`] — per-project quality profiles (user overrides
//!   persisted), runs in the project root or an agent/task worktree through a
//!   [`CheckExecutor`] (production: the terminal run API → security
//!   `CommandGate`), per-check timeouts, an overall time budget,
//!   cancellation, structured results, `BuildStarted`/`BuildSucceeded`/
//!   `BuildFailed`/`TestRunFinished` events.
//! * [`parse`] — output parsers (libtest, Jest, Vitest, pytest, go test,
//!   dotnet, Maven, Gradle; rustc/clippy, tsc, ESLint, MSBuild, javac and the
//!   generic `file:line:col` form).
//! * [`gates`] — configurable gates + Definition of Done → `GateReport`;
//!   review state and security findings come through [`sources`] traits.
//!
//! No autonomous loop lives here: a run is a bounded sequence of commands
//! (max checks, per-check timeout, overall budget).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod detect;
pub mod error;
pub mod executor;
pub mod gates;
pub mod parse;
mod service;
pub mod sources;
pub mod store;

pub use detect::{detect, DetectOptions};
pub use error::{QualityError, QualityResult};
pub use executor::{CheckExecutor, ExecError, ExecOutput, ExecRequest, TerminalExecutor};
pub use service::{
    select_checks, validate_checks, QualityConfig, QualityDeps, QualityService, Redactor, StartRun, MAX_CHECKS,
};
pub use sources::{
    NoSecurityScanner, ReviewStatus, ReviewStatusSource, SecurityFindingsSource, SecurityStatus, TaskFieldReview,
};
pub use store::{quality_migrations, QualityStore};

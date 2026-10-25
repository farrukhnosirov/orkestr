//! # orkestr-computer
//!
//! The personal computer assistant (ADR-0035): the action vocabulary of
//! `orkestr_protocol::methods::computer`, a generated catalogue of 1000+
//! commands (`data/catalogue.json`), user command sets (LAITIS-style), a
//! fast local matcher, an AI planner for unknown commands, safety
//! classification + confirmations, and OS backends (Windows; others report
//! `Unsupported`).
//!
//! `unsafe` is denied everywhere except the Windows backend module (Win32 /
//! COM FFI), see ADR-0035.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod apps;
pub mod backend;
pub mod catalogue;
pub mod dialog;
pub mod error;
pub mod executor;
pub mod keys;
pub mod matcher;
pub mod planner;
pub mod procs;
pub mod safety;
pub mod service;
pub mod store;
pub mod text;

pub use backend::{default_backend, ComputerBackend, InstalledApp, MockBackend, MockCall, UnsupportedBackend};
pub use error::{ComputerError, ComputerResult};
pub use planner::{PlanModel, PLANNER_MARKER};
pub use service::{ComputerDeps, ComputerService, MY_SET};

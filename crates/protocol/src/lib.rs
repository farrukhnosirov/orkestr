//! # orkestr-protocol
//!
//! The shared Desktop/Web/Mobile wire protocol (MASTER_SPEC §65.19,
//! ENGINEERING_CONTRACT §4).
//!
//! * [`frames`] — JSON frames exchanged over `ws://127.0.0.1:<port>/rpc`.
//! * [`error`] — [`RpcError`] and its stable error codes.
//! * [`event`] — the [`Event`] envelope and the append-only [`EventKind`] enum.
//! * [`topic`] — subscription topics and event → topic routing.
//! * [`methods`] — `<area>.<verb>` method names with typed params/results.
//! * [`ui_config`] — the versioned, validated UI configuration schema (§29).
//!
//! All types derive `ts_rs::TS`; `cargo test -p orkestr-protocol export_bindings`
//! (or `npm run gen:protocol`) regenerates `packages/protocol/src/generated`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod event;
pub mod frames;
pub mod methods;
pub mod topic;
pub mod ui_config;
pub mod ui_patch;
pub mod ui_prefs;
pub mod ui_theme;

#[cfg(test)]
mod export;

pub use error::{RpcError, RpcErrorCode};
pub use event::{Event, EventKind, NewEvent};
pub use frames::{ClientFrame, ResFrame, ServerFrame};
pub use methods::Method;

/// Protocol version. Bumped only on breaking wire changes; additive changes
/// (new methods, new event kinds, new optional fields) keep the version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Re-export of the domain crate so clients of the protocol need one dependency.
pub use orkestr_domain as domain;

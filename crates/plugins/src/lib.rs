//! Orkestr plugin host (Phase 7, ADR-0034). **Owner: WP1.**
//!
//! * [`package`]: `.orkp` = zip with `orkestr-plugin.json` (manifest,
//!   [`orkestr_domain::PluginManifest`]), `signature.json` (ed25519 over the
//!   sha256 of every file) and payload files; safe extraction (path traversal,
//!   zip-bomb guards).
//! * [`signing`]: publisher trust store, reused by `orkestr-marketplace`.
//! * [`runtime`]: wasmtime sandbox, ABI v1 ([`abi`]): exports
//!   `orkestr_alloc(i32)->i32`, `orkestr_call(ptr,len)->i64 (ptr<<32|len)`,
//!   import `"orkestr" "host_call"(ptr,len)->i64`; JSON payloads.
//! * [`host`]: [`host::HostApi`]: every host call is capability-checked
//!   against the plugin's grants and audited.
//! * [`service`]: [`PluginService`]: install / grant / enable / disable /
//!   kill switch / safe mode / invoke.

#![forbid(unsafe_code)]

pub mod abi;
pub mod error;
pub mod grants;
pub mod host;
pub mod package;
pub mod runtime;
pub mod service;
pub mod signing;
pub mod store;

pub use error::PluginError;
pub use host::{HostApi, HostOp};
pub use service::{PluginDeps, PluginService};
pub use store::{migrations as plugin_migrations, MIGRATIONS};

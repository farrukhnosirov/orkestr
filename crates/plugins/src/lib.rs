//! Orkestr plugin host (Phase 7, ADR-0034). **Owner: WP1.**
//!
//! * [`package`]: `.orkp` = zip with `orkestr-plugin.json` (manifest,
//!   [`orkestr_domain::PluginManifest`]), `signature.json` (ed25519 over the
//!   canonical content digest, see [`signing`]) and payload files
//!   (`plugin.wasm`, `widgets/*`, ...); safe in-memory parsing (path
//!   traversal, link and zip-bomb guards), manifest + ABI validation, builder.
//! * [`signing`]: publisher key pairs, signature verification and the
//!   [`signing::TrustStore`], reused by `orkestr-marketplace`.
//! * [`runtime`]: wasmtime sandbox, ABI v1 ([`abi`]): exports
//!   `orkestr_alloc(i32)->i32`, `orkestr_call(ptr,len)->i64 (ptr<<32|len)`,
//!   import `"orkestr" "host_call"(ptr,len)->i64`; JSON payloads. Fuel,
//!   epoch deadline (timeout + kill switch) and `StoreLimits` memory cap.
//! * [`host`]: [`host::HostApi`]: every host call is capability-checked
//!   against the plugin's grants ([`grants`], principal `plugin:<id>`) and
//!   audited; denials publish `PluginCallDenied`.
//! * [`service`]: [`PluginService`]: install / grant / enable / disable /
//!   kill switch / safe mode / invoke / logs, tool + widget catalogues.
//! * [`events`]: delivery of `subscribes` events to the optional guest export
//!   `orkestr_on_event` (grant `Read` `event/<Kind>`, bounded drop-oldest queue).
//! * [`deploy`]: `DeploymentProvider` plugins as `orkestr_deploy` adapters
//!   (guest export `orkestr_deploy`), registered while the plugin is enabled.
//! * [`fixtures`] + [`wat`]: signed test packages (`hello-tool`, `evil-*`).

#![forbid(unsafe_code)]

pub mod abi;
pub mod deploy;
pub mod error;
pub mod events;
pub mod fixtures;
pub mod grants;
pub mod host;
pub mod package;
pub mod runtime;
pub mod service;
pub mod signing;
pub mod store;
pub mod wat;

pub use error::PluginError;
pub use host::{HostApi, HostOp, RpcReader};
pub use service::{PluginDeps, PluginService, PluginTool, PluginWidget};
pub use signing::{PackageSignature, PublisherKeyPair, SignerInfo, TrustStore, TrustedKey};
pub use store::{migrations as plugin_migrations, MIGRATIONS};

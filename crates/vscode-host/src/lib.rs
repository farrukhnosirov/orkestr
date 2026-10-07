//! # orkestr-vscode-host
//!
//! The embedded **VS Code** of Orkestr: the open-source VSCodium "remote
//! extension host — web" server (MIT; extensions from Open VSX, never the
//! Microsoft Marketplace), downloaded on demand, verified and supervised.
//!
//! * [`target`] — which release asset fits this OS/arch (values come from
//!   `orkestr-platform`; no `cfg` code here).
//! * [`release`] — GitHub release metadata, pinned download URLs, `.sha256`
//!   parsing and verification.
//! * [`install`] — streaming download with sha256, safe atomic `.tar.gz`
//!   extraction (path traversal and links rejected), package validation,
//!   on-disk layout under `<data_dir>/vscode-server/`.
//! * [`server`] — command line (`127.0.0.1`, random token via
//!   `--connection-token-file`, telemetry off), UI URL, output parsing and
//!   token redaction.
//! * [`lifecycle`] — state machine + crash-restart back-off.
//! * [`VsCodeHost`] — the service the core wires into `vscode.*` RPC methods:
//!   one server instance; the UI opens any project folder through the URL.
//!
//! **Security:** extensions run as the OS user with full access to files and
//! network (they are untrusted code, threat model TB8). Installing requires
//! explicit consent; the server only listens on loopback and every request
//! needs the connection token; it is stopped on core shutdown, on
//! `emergency.stopAll`, and (by default) when no VS Code view is open.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod extensions;
mod host;
pub mod install;
pub mod lifecycle;
pub mod release;
pub mod server;
pub mod target;

pub use error::{HostError, HostResult};
pub use host::{HostOptions, VsCodeHost};

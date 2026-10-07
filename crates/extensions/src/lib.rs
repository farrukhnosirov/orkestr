//! # orkestr-extensions
//!
//! VS Code extension support for Orkestr's own Monaco editor (ADR-0025):
//!
//! * [`openvsx`] — Open VSX client (search, details, SHA-256 verified
//!   downloads). The Microsoft Marketplace is never used.
//! * [`archive`] — safe `.vsix` extraction (zip-slip, links, size limits,
//!   no executable bits).
//! * [`manifest`] — `package.json` parsing, NLS, engine range, classification
//!   of contributions into "works in Orkestr" / "needs full VS Code".
//! * [`theme`], [`icon_theme`], [`snippets`], [`jsonc`] — declarative
//!   contributions read as data; extension JavaScript is never executed.
//! * [`service`] — install / enable / uninstall / preferences on SQLite
//!   ([`store`]) and `<data_dir>/extensions/<publisher>.<name>-<version>/`.
//! * [`lsp`] — language-server recipes, `Content-Length` framing and
//!   supervised processes with restart backoff (started only after a user
//!   approval, enforced by the core).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod archive;
pub mod error;
pub mod icon_theme;
pub mod jsonc;
pub mod lsp;
pub mod manifest;
pub mod openvsx;
pub mod paths;
pub mod service;
pub mod snippets;
pub mod store;
pub mod theme;

pub use error::{ExtError, ExtResult};
pub use openvsx::OpenVsxClient;
pub use service::{ExtensionEvents, ExtensionService, NoEvents};
pub use store::{extensions_migrations, EXTENSIONS_MIGRATIONS};

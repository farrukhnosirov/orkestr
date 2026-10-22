//! Orkestr marketplace (Phase 7, ADR-0040). **Owner: WP2.**
//!
//! A registry is a static directory: `index.json` (entries
//! `{id, kind, version, publisher, keyId, sha256, url, revoked}` + revocation
//! list), `index.sig` (ed25519 over `index.json`) and package files. Sources:
//! the bundled official catalogue (embedded key), a local directory, or HTTP
//! (e.g. `orkestr-gateway` at `/registry/*`). Signatures are verified through
//! [`signing::SignatureVerifier`] (to be unified with `orkestr_plugins::signing`).

#![forbid(unsafe_code)]

pub mod error;
pub mod index;
pub mod keys;
pub mod publish;
pub mod service;
pub mod signing;
pub mod sources;
pub mod store;

pub use error::MarketplaceError;
pub use service::{Installers, Marketplace, MarketplaceDeps, PluginSink, TemplateSink};
pub use store::{migrations as marketplace_migrations, MIGRATIONS};

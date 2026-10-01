//! # orkestr-storage
//!
//! Local persistence for the Orkestr core (contract §3, spec §45):
//!
//! * [`Db`] — a SQLite connection (WAL, foreign keys, busy timeout) guarded by a
//!   mutex and driven from Tokio via `spawn_blocking`.
//! * [`migrations`] — ordered, named, reversible migrations (`up`/`down` SQL)
//!   recorded in `_orkestr_migrations` with checksums. Every engine crate
//!   contributes its own migration list; `orkestr-core` composes them in a
//!   fixed order.
//! * [`metadata`] — the [`MetadataStore`] interface and its SQLite
//!   implementation (projects today; other engines add repositories in their
//!   own crates on top of [`Db`]).
//! * [`objects`] — the content-addressed [`ObjectStore`] (blake3, optional zstd).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod db;
pub mod error;
pub mod metadata;
pub mod migrations;
pub mod objects;

pub use db::Db;
pub use error::{StorageError, StorageResult};
pub use metadata::{MetadataStore, ProjectRepository, SqliteMetadataStore};
pub use migrations::{Migration, MigrationReport, Migrator};
pub use objects::{Compression, FsObjectStore, ObjectId, ObjectStore};

/// Re-export so engine crates don't need a direct rusqlite dependency for
/// simple queries inside [`Db::call`].
pub use rusqlite;

/// Migrations owned by this crate (projects table).
#[must_use]
pub fn storage_migrations() -> Vec<Migration> {
    metadata::MIGRATIONS.to_vec()
}

//! # orkestr-files
//!
//! Workspace file service for the Files and Code panels (§14, §15) and,
//! later, agent file tools:
//!
//! * [`path`] — path confinement (SR-21): lexical validation + physical
//!   resolution that rejects traversal, absolute/UNC/device/verbatim paths,
//!   alternate data streams, reserved names and symlink/junction escapes.
//! * [`ops`] — blocking list/stat/read/write/create/mkdir/rename/delete with
//!   atomic writes and optimistic concurrency (blake3 hashes).
//! * [`search`] — parallel `.gitignore`-aware content search and fuzzy find.
//! * [`watch`] — debounced `notify` watchers emitting `FileChanged` payloads.
//! * [`trash`] — copies of deleted files in the object store (`files_trash`).
//! * [`open`] — OS default app / reveal / external IDE.
//! * [`FilesManager`] — async façade with actor attribution (user/agent).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod manager;
pub mod open;
pub mod ops;
pub mod path;
pub mod search;
pub mod text;
pub mod trash;
pub mod watch;

pub use error::{FilesError, FilesResult};
pub use manager::{ChangeActor, FilesConfig, FilesManager, Mutation, Workspace};
pub use open::{ExternalOpener, OpenRequest, RecordingOpener, SystemOpener};
pub use path::{RelPath, WorkspaceRoot};
pub use search::SearchOptions;
pub use trash::{files_migrations, MemoryTrash, StorageTrash, TrashStore, FILES_MIGRATIONS};
pub use watch::{ChangeSink, RecentWrites, WatcherHub};

//! Orkestr app updates (ADR-0048).
//!
//! * [`settings`] — preferences (remote endpoint, channel, schedule, retention).
//! * [`manifest`] — `latest.json` (the `tauri-plugin-updater` format).
//! * [`signature`] — minisign verification (and signing for the release tool).
//! * [`check`] — remote / local update checks, downloads with progress, verification.
//! * [`backup`] — data backups (SQLite online-backup API) with retention.
//! * [`startup`] — first start of a new version: downgrade guard + pre-migration backup.
//! * [`status`] — the state the UI shows.
//!
//! Installing is done by the desktop shell through `tauri-plugin-updater`
//! (`Updater::restore_update` + `Update::install`), which verifies the
//! signature again and hands over to the NSIS installer in update mode.
#![forbid(unsafe_code)]

pub mod backup;
pub mod check;
pub mod error;
pub mod manifest;
pub mod settings;
pub mod signature;
pub mod startup;
pub mod status;

pub use backup::{create_backup, list_backups, BackupInfo, BackupReason, BackupRequest};
pub use check::{CheckOutcome, Checker, SourceKind, UpdateInfo, UpdateSource};
pub use error::{ErrorInfo, UpdateError, UpdateResult};
pub use manifest::{Location, Manifest, Target};
pub use settings::{Channel, UpdateSettings, DEFAULT_ENDPOINT};
pub use startup::{prepare_start, StartKind, StartRequest, StartupReport};
pub use status::{Phase, Progress, UpdateStatus};

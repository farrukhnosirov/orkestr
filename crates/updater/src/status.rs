//! The update state shown in Settings → Updates and the title-bar badge
//! (sent by the desktop shell as the `orkestr://updates/status` event).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::backup::BackupInfo;
use crate::check::UpdateInfo;
use crate::error::ErrorInfo;
use crate::settings::UpdateSettings;
use crate::startup::StartupReport;

/// Where the update flow is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Nothing checked yet.
    #[default]
    Idle,
    /// A check is running.
    Checking,
    /// The last check found nothing newer.
    UpToDate,
    /// An update is available (not downloaded).
    Available,
    /// Downloading / verifying.
    Downloading,
    /// Downloaded and verified; ready to install (now or on exit).
    Ready,
    /// Backing up and launching the installer.
    Installing,
    /// The last operation failed (`last_error`).
    Error,
}

/// Download progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Bytes so far.
    pub downloaded: u64,
    /// Total, when the server announced it.
    pub total: Option<u64>,
}

/// Full update state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Running version.
    pub current_version: String,
    /// Flow phase.
    pub phase: Phase,
    /// Preferences.
    pub settings: UpdateSettings,
    /// The update on offer.
    pub available: Option<UpdateInfo>,
    /// Download progress.
    pub progress: Option<Progress>,
    /// Last completed check.
    pub last_checked_at: Option<DateTime<Utc>>,
    /// Version the last check saw (even when not newer).
    pub latest_seen: Option<String>,
    /// Last failure.
    pub last_error: Option<ErrorInfo>,
    /// The downloaded update installs when the app quits.
    pub install_on_exit: bool,
    /// What happened at this start (upgrade, rollback info, pre-migration backup).
    pub startup: Option<StartupReport>,
    /// Folder with data backups.
    pub backups_dir: String,
    /// Backups, newest first.
    pub backups: Vec<BackupInfo>,
}

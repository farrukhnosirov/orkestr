//! Update preferences (Settings → Updates), persisted by the desktop shell in
//! `<config_dir>/updates.json`. They describe the installed binary, not
//! business data, so they live with the shell rather than in the core DB.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{UpdateError, UpdateResult};

/// Placeholder endpoint of the future update server. It stays **disabled**
/// until the user (or a later release) turns the remote channel on.
/// Variables: `{{target}}` (`windows`), `{{arch}}` (`x86_64`),
/// `{{current_version}}`, `{{channel}}` (`stable` / `beta`), `{{bundle_type}}` (`nsis`).
pub const DEFAULT_ENDPOINT: &str = "https://updates.orkestr.dev/{{target}}/{{arch}}/{{current_version}}";

/// File name of the settings inside the config dir.
pub const SETTINGS_FILE: &str = "updates.json";

/// Release channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    /// Only final releases (`1.2.3`).
    #[default]
    Stable,
    /// Pre-releases too (`1.3.0-beta.1`).
    Beta,
}

impl Channel {
    /// `stable` / `beta` (the `{{channel}}` endpoint variable).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

/// Update preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateSettings {
    /// Check the remote endpoint at all (off until an update server exists).
    pub remote_enabled: bool,
    /// Endpoint template (see [`DEFAULT_ENDPOINT`]).
    pub endpoint: String,
    /// Release channel.
    pub channel: Channel,
    /// Check once shortly after start.
    pub check_on_startup: bool,
    /// Periodic check interval in hours; `0` = never.
    pub check_interval_hours: u32,
    /// Download a found update in the background (installing always needs the user).
    pub auto_download: bool,
    /// Last local file picked for a manual update (installer `.exe` / `.msi` or `latest.json`).
    pub local_path: Option<String>,
    /// How many data backups to keep (oldest are pruned); at least 1.
    pub backup_retention: u32,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            remote_enabled: false,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            channel: Channel::Stable,
            check_on_startup: true,
            check_interval_hours: 24,
            auto_download: false,
            local_path: None,
            backup_retention: 5,
        }
    }
}

/// Bounds of [`UpdateSettings::check_interval_hours`] (0 = off, max one week).
pub const MAX_CHECK_INTERVAL_HOURS: u32 = 24 * 7;
/// Bounds of [`UpdateSettings::backup_retention`].
pub const MAX_BACKUP_RETENTION: u32 = 50;

impl UpdateSettings {
    /// Validates and normalizes (trims the endpoint, clamps numbers).
    pub fn normalized(mut self) -> UpdateResult<Self> {
        self.endpoint = self.endpoint.trim().to_owned();
        if self.endpoint.is_empty() {
            self.endpoint = DEFAULT_ENDPOINT.to_owned();
        }
        validate_endpoint(&self.endpoint)?;
        self.check_interval_hours = self.check_interval_hours.min(MAX_CHECK_INTERVAL_HOURS);
        self.backup_retention = self.backup_retention.clamp(1, MAX_BACKUP_RETENTION);
        self.local_path = self.local_path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
        Ok(self)
    }

    /// Loads `<config_dir>/updates.json`; defaults when missing or unreadable
    /// (a broken preferences file must never stop the app).
    #[must_use]
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(SETTINGS_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes).map(Self::normalized) {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "invalid update settings; using defaults");
                    Self::default()
                }
                Err(e) => {
                    tracing::warn!(error = %e, "unreadable update settings; using defaults");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Saves atomically (write to a temp file, then rename).
    pub fn save(&self, config_dir: &Path) -> UpdateResult<PathBuf> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join(SETTINGS_FILE);
        let tmp = config_dir.join(format!("{SETTINGS_FILE}.tmp"));
        let json = serde_json::to_vec_pretty(self).map_err(|e| UpdateError::Io(e.to_string()))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

/// An endpoint must be `https://`; `http://` is accepted only for loopback
/// hosts (a local test/self-hosted server). Signatures protect the payload
/// either way, TLS protects the release notes and privacy.
pub fn validate_endpoint(template: &str) -> UpdateResult<()> {
    // Variables are substituted later; validate a representative expansion.
    let sample = template
        .replace("{{target}}", "windows")
        .replace("{{arch}}", "x86_64")
        .replace("{{current_version}}", "0.0.0")
        .replace("{{channel}}", "stable")
        .replace("{{bundle_type}}", "nsis");
    let url = reqwest::Url::parse(&sample).map_err(|e| UpdateError::InvalidEndpoint(format!("{template}: {e}")))?;
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&url) => Ok(()),
        other => Err(UpdateError::InvalidEndpoint(format!(
            "{template}: the scheme must be https (got {other}; http is allowed only for localhost)"
        ))),
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

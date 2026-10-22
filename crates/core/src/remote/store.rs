//! Persistent remote-access state: `<data_dir>/remote/state.json` (config +
//! authoritative device registry) and `<data_dir>/remote/cik.key` (core
//! identity key, P-256 scalar, hex). Both files are written atomically and
//! restricted to the owner (0600 on Unix; per-user profile ACL on Windows).

use std::path::{Path, PathBuf};

use orkestr_domain::{DeviceId, ProjectId, Timestamp};
use orkestr_protocol::methods::remote::DeviceInfo;
use orkestr_protocol::policy::Role;
use orkestr_security::webauthn::{Credential, RelyingParty};
use p256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};

/// A paired device (authoritative record, remote-access.md §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRecord {
    /// Device id.
    pub device_id: DeviceId,
    /// Name.
    pub name: String,
    /// Platform.
    pub platform: String,
    /// Device key (SEC1 P-256, base64url).
    pub public_key: String,
    /// Role.
    pub role: Role,
    /// Remote terminal allowed.
    pub remote_terminal: bool,
    /// Project restriction.
    #[serde(default)]
    pub project_ids: Option<Vec<ProjectId>>,
    /// Paired at.
    pub created_at: Timestamp,
    /// Last handshake.
    #[serde(default)]
    pub last_seen_at: Option<Timestamp>,
    /// Revoked.
    #[serde(default)]
    pub revoked: bool,
    /// Step-up WebAuthn credential (registered over E2E).
    #[serde(default)]
    pub step_up: Option<Credential>,
}

impl DeviceRecord {
    /// Protocol view.
    #[must_use]
    pub fn info(&self) -> DeviceInfo {
        DeviceInfo {
            device_id: self.device_id,
            name: self.name.clone(),
            role: self.role,
            remote_terminal: self.remote_terminal,
            project_ids: self.project_ids.clone(),
            created_at: self.created_at,
            last_seen_at: self.last_seen_at,
            revoked: self.revoked,
            platform: self.platform.clone(),
            step_up_registered: self.step_up.is_some(),
        }
    }
}

/// Persisted state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteState {
    /// Kill switch: `false` ⇒ never dial.
    pub enabled: bool,
    /// Gateway base URL.
    pub gateway_url: Option<String>,
    /// Core id assigned by the gateway at enrollment.
    pub core_id: Option<String>,
    /// Account id at the gateway.
    pub account_id: Option<String>,
    /// Bearer token for gateway HTTP APIs (sync); never sent over E2E.
    pub core_token: Option<String>,
    /// One-time enrollment token waiting to be used on the next dial.
    pub enrollment_token: Option<String>,
    /// WebAuthn relying party of the gateway (for step-up verification).
    pub rp: Option<RelyingParty>,
    /// Devices.
    #[serde(default)]
    pub devices: Vec<DeviceRecord>,
}

/// File-backed store.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

impl Store {
    /// Store rooted at `dir` (created on first write).
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Loads the state (default when missing or unreadable).
    #[must_use]
    pub fn load(&self) -> RemoteState {
        match std::fs::read(self.dir.join("state.json")) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "remote state unreadable; starting disabled");
                RemoteState::default()
            }),
            Err(_) => RemoteState::default(),
        }
    }

    /// Saves the state.
    ///
    /// # Errors
    /// I/O failure.
    pub fn save(&self, state: &RemoteState) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let bytes = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
        write_private(&self.dir.join("state.json"), &bytes)
    }

    /// Loads or creates the core identity key.
    ///
    /// # Errors
    /// I/O failure.
    pub fn load_or_create_cik(&self) -> std::io::Result<SigningKey> {
        let path = self.dir.join("cik.key");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(k) = hex::decode(text.trim()).ok().and_then(|b| SigningKey::from_slice(&b).ok()) {
                return Ok(k);
            }
            tracing::warn!("core identity key unreadable; generating a new one (devices must re-pair)");
        }
        let key = SigningKey::random(&mut rand_core06::OsRng);
        std::fs::create_dir_all(&self.dir)?;
        write_private(&path, hex::encode(key.to_bytes()).as_bytes())?;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_and_key_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("remote"));
        assert_eq!(store.load(), RemoteState::default());
        let k1 = store.load_or_create_cik().unwrap();
        let k2 = store.load_or_create_cik().unwrap();
        assert_eq!(k1.to_bytes(), k2.to_bytes());
        let st = RemoteState { enabled: true, core_id: Some("c".into()), ..RemoteState::default() };
        store.save(&st).unwrap();
        assert_eq!(store.load(), st);
    }
}

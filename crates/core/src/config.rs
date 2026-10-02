//! Core configuration.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use orkestr_platform::Autostart;

use crate::error::CoreError;

/// What `settings.update { autostart: true }` registers to start at OS login (§12).
#[derive(Clone)]
pub struct AutostartConfig {
    /// Registration id (`[A-Za-z0-9._-]+`): HKCU Run value / LaunchAgent label / `.desktop` name.
    pub id: String,
    /// Executable; `None` → the current executable (desktop app or `orkestrd`).
    pub program: Option<PathBuf>,
    /// Arguments, e.g. `["--autostart"]` (desktop: start hidden in the tray).
    pub args: Vec<String>,
    /// Replaces the OS implementation (tests use `MemoryAutostart`).
    pub backend: Option<Arc<dyn Autostart>>,
}

impl Default for AutostartConfig {
    fn default() -> Self {
        Self { id: "dev.orkestr.orkestr".into(), program: None, args: Vec::new(), backend: None }
    }
}

impl std::fmt::Debug for AutostartConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutostartConfig")
            .field("id", &self.id)
            .field("program", &self.program)
            .field("args", &self.args)
            .field("custom_backend", &self.backend.is_some())
            .finish()
    }
}

/// Configuration for [`crate::start`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Data directory. `None` → `ORKESTR_HOME` or the OS default.
    pub data_dir: Option<PathBuf>,
    /// Bind address. Must be loopback (contract §3: never bind 0.0.0.0).
    /// Default `127.0.0.1:0` (ephemeral port).
    pub bind_addr: SocketAddr,
    /// Fixed bearer token (tests). `None` → random 256-bit token.
    pub token: Option<String>,
    /// Write `<data_dir>/core.json` + `core.token` for discovery.
    pub write_discovery: bool,
    /// Event bus buffer size.
    pub event_capacity: usize,
    /// Secret value backend. Default: `ORKESTR_SECRET_BACKEND` (`auto`, `vault`,
    /// `vault-file`, `memory`) or `Auto` (OS keyring, encrypted vault fallback).
    pub secret_backend: orkestr_security::SecretBackendChoice,
    /// Autostart registration used by `settings.update`.
    pub autostart: AutostartConfig,
    /// Interval of the live resource sampler / `NodeStatus` heartbeat.
    pub node_sample_interval: Duration,
    /// Detect the node inventory (GPUs, runtimes, models) in the background
    /// this long after start so Settings/onboarding open instantly.
    /// `None` → detect lazily on the first `nodes.info`.
    pub node_info_prefetch_after: Option<Duration>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            bind_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            token: None,
            write_discovery: true,
            event_capacity: orkestr_events::DEFAULT_CAPACITY,
            secret_backend: std::env::var("ORKESTR_SECRET_BACKEND")
                .ok()
                .and_then(|v| orkestr_security::SecretBackendChoice::parse(&v))
                .unwrap_or_default(),
            autostart: AutostartConfig::default(),
            node_sample_interval: Duration::from_secs(2),
            node_info_prefetch_after: Some(Duration::from_secs(5)),
        }
    }
}

impl CoreConfig {
    /// Config rooted at a specific data dir (tests, portable installs).
    #[must_use]
    pub fn with_data_dir(dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: Some(dir.into()), ..Self::default() }
    }

    /// Validates security-relevant settings.
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.bind_addr.ip().is_loopback() {
            return Err(CoreError::Config(format!(
                "refusing to bind {}: the core only listens on loopback (remote access goes through the gateway, §35)",
                self.bind_addr
            )));
        }
        if let Some(t) = &self.token {
            if t.len() < 32 {
                return Err(CoreError::Config("token must be at least 32 characters".into()));
            }
        }
        if self.node_sample_interval < Duration::from_millis(250) {
            return Err(CoreError::Config("node_sample_interval must be at least 250 ms".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_and_short_tokens() {
        let c = CoreConfig { bind_addr: "0.0.0.0:0".parse().unwrap(), ..CoreConfig::default() };
        assert!(c.validate().is_err());
        let c = CoreConfig { token: Some("short".into()), ..CoreConfig::default() };
        assert!(c.validate().is_err());
        CoreConfig::default().validate().unwrap();
        let c = CoreConfig { bind_addr: "[::1]:0".parse().unwrap(), ..CoreConfig::default() };
        c.validate().unwrap();
        let c = CoreConfig { node_sample_interval: Duration::from_millis(10), ..CoreConfig::default() };
        assert!(c.validate().is_err());
    }
}

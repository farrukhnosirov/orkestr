//! Core configuration.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use crate::error::CoreError;

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
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            bind_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            token: None,
            write_discovery: true,
            event_capacity: orkestr_events::DEFAULT_CAPACITY,
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
    }
}

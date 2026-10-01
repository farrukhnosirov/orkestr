//! Data directories. Never hardcode OS paths (contract §1).
//!
//! Resolution order:
//! 1. `ORKESTR_HOME` environment variable (tests MUST use this or
//!    [`Paths::from_root`]): everything lives under that single directory.
//! 2. OS conventions via the `directories` crate
//!    (Windows `%APPDATA%\Orkestr\Orkestr\data` / `%LOCALAPPDATA%` for cache,
//!    macOS `~/Library/Application Support/dev.orkestr.Orkestr`,
//!    Linux `$XDG_DATA_HOME/orkestr`).

use std::path::{Path, PathBuf};

use crate::error::{PlatformError, PlatformResult};

/// Environment variable overriding the data root.
pub const ORKESTR_HOME_ENV: &str = "ORKESTR_HOME";

/// Resolved directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Persistent data (database, objects, discovery file).
    pub data_dir: PathBuf,
    /// User configuration.
    pub config_dir: PathBuf,
    /// Disposable caches.
    pub cache_dir: PathBuf,
    /// Log files.
    pub logs_dir: PathBuf,
}

impl Paths {
    /// Resolves paths from `ORKESTR_HOME` or OS conventions.
    pub fn resolve() -> PlatformResult<Self> {
        if let Some(home) = std::env::var_os(ORKESTR_HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::from_root(PathBuf::from(home)));
        }
        let dirs = directories::ProjectDirs::from("dev", "Orkestr", "Orkestr").ok_or(PlatformError::NoDataDir)?;
        let data_dir = dirs.data_dir().to_path_buf();
        Ok(Self {
            logs_dir: data_dir.join("logs"),
            data_dir,
            config_dir: dirs.config_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// All directories under one root (portable installs, tests).
    #[must_use]
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            logs_dir: root.join("logs"),
            data_dir: root,
        }
    }

    /// Creates every directory.
    pub fn ensure(&self) -> PlatformResult<()> {
        for d in [&self.data_dir, &self.config_dir, &self.cache_dir, &self.logs_dir] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// `<data>/orkestr.db`.
    #[must_use]
    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("orkestr.db")
    }

    /// `<data>/objects`.
    #[must_use]
    pub fn objects_dir(&self) -> PathBuf {
        self.data_dir.join("objects")
    }

    /// `<data>/core.json` — running-core discovery file.
    #[must_use]
    pub fn discovery_file(&self) -> PathBuf {
        self.data_dir.join("core.json")
    }

    /// `<data>/core.token` — bearer token of the running core.
    #[must_use]
    pub fn token_file(&self) -> PathBuf {
        self.data_dir.join("core.token")
    }

    /// `<data>/session.lock` — present while a core runs; left behind by crashes.
    #[must_use]
    pub fn session_marker(&self) -> PathBuf {
        self.data_dir.join("session.lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_root_layout() {
        let p = Paths::from_root("/tmp/ork");
        assert_eq!(p.database_file(), PathBuf::from("/tmp/ork").join("orkestr.db"));
        assert_eq!(p.discovery_file(), PathBuf::from("/tmp/ork").join("core.json"));
        assert!(p.logs_dir.ends_with("logs"));
    }

    #[test]
    fn ensure_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::from_root(dir.path().join("x"));
        p.ensure().unwrap();
        assert!(p.cache_dir.is_dir() && p.config_dir.is_dir() && p.logs_dir.is_dir());
    }

    #[test]
    fn resolve_without_override_is_absolute() {
        // Only meaningful when ORKESTR_HOME is not set in the test environment.
        if std::env::var_os(ORKESTR_HOME_ENV).is_none() {
            let p = Paths::resolve().unwrap();
            assert!(p.data_dir.is_absolute());
        }
    }
}

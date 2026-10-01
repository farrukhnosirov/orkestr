//! Start-on-login registration (§12: autostart only with user permission).
//!
//! * Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` via `reg.exe`.
//! * macOS: `~/Library/LaunchAgents/<id>.plist`.
//! * Linux: `$XDG_CONFIG_HOME/autostart/<id>.desktop`.

use std::path::PathBuf;

use crate::error::{PlatformError, PlatformResult};

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartEntry {
    /// Identifier (`[A-Za-z0-9._-]+`), e.g. `dev.orkestr.orkestrd`.
    pub id: String,
    /// Executable.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
}

/// Autostart registration.
pub trait Autostart: Send + Sync {
    /// Registers (or updates) an entry.
    fn enable(&self, entry: &AutostartEntry) -> PlatformResult<()>;
    /// Removes an entry. Returns whether it existed.
    fn disable(&self, id: &str) -> PlatformResult<bool>;
    /// Whether an entry is registered.
    fn is_enabled(&self, id: &str) -> PlatformResult<bool>;
}

fn validate_id(id: &str) -> PlatformResult<()> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        Ok(())
    } else {
        Err(PlatformError::Invalid(format!("invalid autostart id `{id}`")))
    }
}

fn reject_control_chars(s: &str) -> PlatformResult<()> {
    if s.chars().any(char::is_control) {
        return Err(PlatformError::Invalid("autostart arguments must not contain control characters".into()));
    }
    Ok(())
}

/// Native implementation. `base_dir` overrides the per-user directory on
/// macOS/Linux (tests); ignored on Windows.
#[derive(Debug, Default, Clone)]
pub struct NativeAutostart {
    /// Override for the LaunchAgents/autostart directory.
    pub base_dir: Option<PathBuf>,
}

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(target_os = "windows")]
fn quote_windows(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

#[cfg(target_os = "windows")]
impl Autostart for NativeAutostart {
    fn enable(&self, entry: &AutostartEntry) -> PlatformResult<()> {
        validate_id(&entry.id)?;
        let mut command = format!("\"{}\"", entry.program.display());
        for a in &entry.args {
            reject_control_chars(a)?;
            command.push(' ');
            command.push_str(&quote_windows(a));
        }
        crate::error::run_command("reg", &["add", RUN_KEY, "/v", &entry.id, "/t", "REG_SZ", "/d", &command, "/f"])
            .map(|_| ())
    }

    fn disable(&self, id: &str) -> PlatformResult<bool> {
        validate_id(id)?;
        if !self.is_enabled(id)? {
            return Ok(false);
        }
        crate::error::run_command("reg", &["delete", RUN_KEY, "/v", id, "/f"]).map(|_| true)
    }

    fn is_enabled(&self, id: &str) -> PlatformResult<bool> {
        validate_id(id)?;
        let status = std::process::Command::new("reg")
            .args(["query", RUN_KEY, "/v", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        Ok(status.success())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl NativeAutostart {
    fn dir(&self) -> PlatformResult<PathBuf> {
        if let Some(d) = &self.base_dir {
            return Ok(d.clone());
        }
        let base = directories::BaseDirs::new().ok_or(PlatformError::NoDataDir)?;
        #[cfg(target_os = "macos")]
        {
            Ok(base.home_dir().join("Library").join("LaunchAgents"))
        }
        #[cfg(target_os = "linux")]
        {
            Ok(base.config_dir().join("autostart"))
        }
    }

    fn file(&self, id: &str) -> PlatformResult<PathBuf> {
        #[cfg(target_os = "macos")]
        let name = format!("{id}.plist");
        #[cfg(target_os = "linux")]
        let name = format!("{id}.desktop");
        Ok(self.dir()?.join(name))
    }
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Autostart for NativeAutostart {
    fn enable(&self, entry: &AutostartEntry) -> PlatformResult<()> {
        validate_id(&entry.id)?;
        for a in &entry.args {
            reject_control_chars(a)?;
        }
        let path = self.file(&entry.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(target_os = "macos")]
        let content = {
            let mut args = format!("<string>{}</string>", xml_escape(&entry.program.to_string_lossy()));
            for a in &entry.args {
                args.push_str(&format!("<string>{}</string>", xml_escape(a)));
            }
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
                 \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\
                 <key>Label</key><string>{}</string><key>ProgramArguments</key><array>{args}</array>\
                 <key>RunAtLoad</key><true/></dict></plist>\n",
                xml_escape(&entry.id)
            )
        };
        #[cfg(target_os = "linux")]
        let content = {
            let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$"));
            let mut exec = quote(&entry.program.to_string_lossy());
            for a in &entry.args {
                exec.push(' ');
                exec.push_str(&quote(a));
            }
            format!(
                "[Desktop Entry]\nType=Application\nName=Orkestr ({})\nExec={exec}\nX-GNOME-Autostart-enabled=true\n",
                entry.id
            )
        };
        std::fs::write(path, content)?;
        Ok(())
    }

    fn disable(&self, id: &str) -> PlatformResult<bool> {
        validate_id(id)?;
        match std::fs::remove_file(self.file(id)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn is_enabled(&self, id: &str) -> PlatformResult<bool> {
        validate_id(id)?;
        Ok(self.file(id)?.is_file())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
impl Autostart for NativeAutostart {
    fn enable(&self, _entry: &AutostartEntry) -> PlatformResult<()> {
        Err(PlatformError::not_supported("autostart", "unsupported OS"))
    }
    fn disable(&self, _id: &str) -> PlatformResult<bool> {
        Err(PlatformError::not_supported("autostart", "unsupported OS"))
    }
    fn is_enabled(&self, _id: &str) -> PlatformResult<bool> {
        Err(PlatformError::not_supported("autostart", "unsupported OS"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_ids() {
        let a = NativeAutostart::default();
        assert!(a.is_enabled("bad id; rm -rf /").is_err());
        assert!(a.disable("").is_err());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn file_based_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let a = NativeAutostart { base_dir: Some(dir.path().to_path_buf()) };
        let e = AutostartEntry {
            id: "dev.orkestr.test".into(),
            program: "/usr/bin/true".into(),
            args: vec!["--x y".into()],
        };
        a.enable(&e).unwrap();
        assert!(a.is_enabled(&e.id).unwrap());
        assert!(a.disable(&e.id).unwrap());
        assert!(!a.is_enabled(&e.id).unwrap());
    }

    /// Modifies the real HKCU Run key (adds and removes a dummy entry).
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "modifies the real user autostart registry key"]
    fn registry_roundtrip() {
        let a = NativeAutostart::default();
        let e = AutostartEntry {
            id: "dev.orkestr.test-autostart".into(),
            program: "C:/Windows/System32/cmd.exe".into(),
            args: vec!["/c".into(), "exit 0".into()],
        };
        a.enable(&e).unwrap();
        assert!(a.is_enabled(&e.id).unwrap());
        assert!(a.disable(&e.id).unwrap());
        assert!(!a.is_enabled(&e.id).unwrap());
    }
}

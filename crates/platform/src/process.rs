//! Process control.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::error::{run_command, PlatformResult};

/// Process helpers needed by the core (discovery, terminal, agents).
pub trait ProcessControl: Send + Sync {
    /// Finds an executable on `PATH` (honouring `PATHEXT` on Windows).
    fn which(&self, name: &str) -> Option<PathBuf>;
    /// Whether a process with this pid is running.
    fn is_alive(&self, pid: u32) -> bool;
    /// Kills a process and its children.
    fn kill_tree(&self, pid: u32) -> PlatformResult<()>;
    /// Pid of the current process.
    fn current_pid(&self) -> u32 {
        std::process::id()
    }
}

/// Native implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeProcessControl;

/// A `Command` for a background helper process (git, build tools, …) that
/// never shows a window: on Windows it sets `CREATE_NO_WINDOW`, so a console
/// program spawned by the GUI app does not flash a console. Arguments are
/// passed as a vector by the caller (never through a shell).
#[must_use]
pub fn background_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn sys() -> &'static Mutex<sysinfo::System> {
    static SYS: std::sync::OnceLock<Mutex<sysinfo::System>> = std::sync::OnceLock::new();
    SYS.get_or_init(|| Mutex::new(sysinfo::System::new()))
}

/// Searches `PATH` for `name`.
#[must_use]
pub fn which(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for file in executable_names(name) {
            let full = dir.join(&file);
            if is_executable(&full) {
                return Some(full);
            }
        }
    }
    None
}

#[cfg(windows)]
fn executable_names(name: &str) -> Vec<String> {
    let has_ext = Path::new(name).extension().is_some();
    let mut out = Vec::new();
    if has_ext {
        out.push(name.to_owned());
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    for ext in exts.split(';').filter(|e| !e.is_empty()) {
        out.push(format!("{name}{}", ext.to_ascii_lowercase()));
    }
    out
}

#[cfg(not(windows))]
fn executable_names(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(any(windows, unix)))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

impl ProcessControl for NativeProcessControl {
    fn which(&self, name: &str) -> Option<PathBuf> {
        which(name)
    }

    fn is_alive(&self, pid: u32) -> bool {
        let spid = sysinfo::Pid::from_u32(pid);
        let mut s = sys().lock();
        s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[spid]), true);
        s.process(spid).is_some()
    }

    fn kill_tree(&self, pid: u32) -> PlatformResult<()> {
        let pid_s = pid.to_string();
        #[cfg(windows)]
        {
            run_command("taskkill", &["/PID", &pid_s, "/T", "/F"]).map(|_| ())
        }
        #[cfg(not(windows))]
        {
            // Try the process group first (agents/terminals are spawned as group leaders),
            // then the process itself.
            let group = format!("-{pid_s}");
            if run_command("kill", &["-TERM", "--", &group]).is_ok() {
                return Ok(());
            }
            run_command("kill", &["-TERM", &pid_s]).map(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        let p = NativeProcessControl;
        assert!(p.is_alive(std::process::id()));
        assert!(!p.is_alive(u32::MAX - 7));
    }

    #[test]
    fn which_finds_a_system_tool() {
        #[cfg(windows)]
        let tool = "cmd";
        #[cfg(not(windows))]
        let tool = "sh";
        assert!(which(tool).is_some(), "{tool} should be on PATH");
        assert!(which("definitely-not-a-real-binary-orkestr").is_none());
    }
}

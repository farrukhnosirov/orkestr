//! Process control.
//!
//! Besides `which`/`is_alive`/`kill_tree` this module provides
//! [`run_with_timeout`]: the sanctioned way for engines to run short probe
//! commands (`git --version`, `nvidia-smi`, `reg query`, …). It never blocks
//! longer than the timeout, kills the child when it expires, captures
//! stdout/stderr (decoding UTF-16LE output such as `wsl.exe` prints) and — on
//! Windows — never flashes a console window from the GUI process.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::error::{run_command, PlatformError, PlatformResult};

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
    /// Runs a short command with a hard timeout (see [`run_with_timeout`]).
    fn run(&self, program: &Path, args: &[&str], timeout: Duration) -> PlatformResult<CommandOutput> {
        run_with_timeout(program, args, timeout)
    }
}

/// Captured result of [`run_with_timeout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code (`None` when killed by a signal).
    pub code: Option<i32>,
    /// Whether the process exited with status 0.
    pub success: bool,
    /// Decoded stdout (lossy UTF-8, or UTF-16LE when detected).
    pub stdout: String,
    /// Decoded stderr.
    pub stderr: String,
}

impl CommandOutput {
    /// stdout if non-empty, otherwise stderr (`java -version` prints to stderr).
    #[must_use]
    pub fn text(&self) -> &str {
        if self.stdout.trim().is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

/// Maximum captured bytes per stream (probe commands are small).
const MAX_CAPTURE: u64 = 1024 * 1024;

/// Decodes command output: UTF-16LE (BOM or mostly-NUL odd bytes) or lossy UTF-8.
#[must_use]
pub fn decode_output(bytes: &[u8]) -> String {
    let odd_nuls = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    let looks_utf16 = bytes.starts_with(&[0xFF, 0xFE])
        || (bytes.len() >= 4 && bytes.len().is_multiple_of(2) && odd_nuls * 4 >= bytes.len());
    if looks_utf16 {
        let body = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
        let units: Vec<u16> = body.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return String::from_utf16_lossy(&units).replace('\0', "");
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(windows)]
pub(crate) fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`: no console window for console programs started from a GUI process.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn hide_console(_cmd: &mut Command) {}

fn spawn_reader<R: Read + Send + 'static>(stream: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stream {
            let _ = s.take(MAX_CAPTURE).read_to_end(&mut buf);
        }
        buf
    })
}

fn program_label(program: &Path) -> String {
    program.file_name().map_or_else(|| program.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Runs `program args…` with stdin closed, capturing output, and kills it
/// after `timeout` ([`PlatformError::Timeout`]). A missing program yields
/// [`PlatformError::Io`] (`NotFound`).
pub fn run_with_timeout(program: &Path, args: &[&str], timeout: Duration) -> PlatformResult<CommandOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn()?;
    let out_reader = spawn_reader(child.stdout.take());
    let err_reader = spawn_reader(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // The reader threads end once the pipes close; never join them here
            // (a grandchild may still hold a pipe open).
            tracing::debug!(command = %program_label(program), "probe command timed out");
            return Err(PlatformError::Timeout {
                command: program_label(program),
                after_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            });
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(CommandOutput {
        code: status.code(),
        success: status.success(),
        stdout: decode_output(&stdout),
        stderr: decode_output(&stderr),
    })
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

/// Force-kills exactly these processes (no tree walk, so a caller can kill a
/// process tree level by level, top-down): one `taskkill /F` on Windows,
/// `SIGKILL` elsewhere. Already-exited processes are ignored.
pub fn kill_pids(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    #[cfg(windows)]
    {
        let mut cmd = background_command("taskkill");
        cmd.arg("/F");
        for pid in pids {
            cmd.arg("/PID").arg(pid.to_string());
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Err(e) = cmd.status() {
            tracing::warn!(error = %e, "taskkill failed to start");
        }
    }
    #[cfg(not(windows))]
    {
        let spids: Vec<sysinfo::Pid> = pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        let mut s = sys().lock();
        s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&spids), true);
        for pid in &spids {
            if let Some(p) = s.process(*pid) {
                // SIGKILL; a dead/zombie process simply reports failure.
                let _ = p.kill();
            }
        }
    }
}

/// Prepares a child command for supervised execution: no console window on
/// Windows, its own process group on Unix (so the whole tree can be killed).
pub fn isolate_command(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = cmd;
    }
}

/// Appends a pre-quoted argument tail verbatim on Windows (`cmd /s /c "…"`
/// needs its own quoting); a normal argument elsewhere.
pub fn push_raw_arg(cmd: &mut tokio::process::Command, tail: &str) {
    #[cfg(windows)]
    {
        cmd.raw_arg(tail);
    }
    #[cfg(not(windows))]
    {
        cmd.arg(tail);
    }
}

fn sys() -> &'static Mutex<sysinfo::System> {
    static SYS: std::sync::OnceLock<Mutex<sysinfo::System>> = std::sync::OnceLock::new();
    SYS.get_or_init(|| Mutex::new(sysinfo::System::new()))
}

/// Executable path of a running process (`None` when it is gone or not
/// accessible). Lets callers verify a recorded pid still belongs to the
/// program they started before killing it (pids are reused).
#[must_use]
pub fn process_exe(pid: u32) -> Option<PathBuf> {
    let spid = sysinfo::Pid::from_u32(pid);
    let mut s = sys().lock();
    s.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[spid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    s.process(spid).and_then(|p| p.exe().map(Path::to_path_buf))
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
    fn process_exe_of_self() {
        let exe = process_exe(std::process::id()).expect("own exe");
        assert_eq!(exe.file_name(), std::env::current_exe().unwrap().file_name());
        assert!(process_exe(u32::MAX - 7).is_none());
    }

    #[test]
    fn current_process_is_alive() {
        let p = NativeProcessControl;
        assert!(p.is_alive(std::process::id()));
        assert!(!p.is_alive(u32::MAX - 7));
    }

    #[test]
    fn run_with_timeout_captures_and_times_out() {
        #[cfg(windows)]
        let (shell, echo, sleep): (&str, Vec<&str>, Vec<&str>) =
            ("cmd", vec!["/C", "echo hello"], vec!["/C", "ping -n 6 127.0.0.1 >NUL"]);
        #[cfg(not(windows))]
        let (shell, echo, sleep): (&str, Vec<&str>, Vec<&str>) =
            ("sh", vec!["-c", "echo hello"], vec!["-c", "sleep 5"]);
        let path = which(shell).expect("shell on PATH");
        let out = run_with_timeout(&path, &echo, Duration::from_secs(10)).unwrap();
        assert!(out.success);
        assert_eq!(out.text().trim(), "hello");
        let started = Instant::now();
        let err = run_with_timeout(&path, &sleep, Duration::from_millis(300)).unwrap_err();
        assert!(matches!(err, PlatformError::Timeout { .. }), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
        let missing = run_with_timeout(Path::new("definitely-not-a-real-binary-orkestr"), &[], Duration::from_secs(1));
        assert!(matches!(missing, Err(PlatformError::Io(_))));
    }

    #[test]
    fn decodes_utf16_and_utf8() {
        let utf16: Vec<u8> = "WSL version: 2.1.5".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_output(&utf16), "WSL version: 2.1.5");
        let mut bom = vec![0xFF, 0xFE];
        bom.extend_from_slice(&utf16);
        assert_eq!(decode_output(&bom), "WSL version: 2.1.5");
        assert_eq!(decode_output(b"git version 2.45.1"), "git version 2.45.1");
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

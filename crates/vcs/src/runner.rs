//! Spawning `git` (ADR-0008 invocation rules).
//!
//! * argument vectors only — never a shell, never string interpolation;
//! * global args `--no-pager -c core.quotepath=off -c color.ui=never
//!   -c gc.auto=0 -c core.longpaths=true`, pathspecs literal
//!   (`GIT_LITERAL_PATHSPECS=1`);
//! * environment `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`,
//!   `GIT_EDITOR=true`, `LC_ALL=C`, `GIT_OPTIONAL_LOCKS=0` for reads;
//! * a timeout (process killed on expiry);
//! * mutating commands retry while another process holds a `*.lock` file
//!   (backoff 50 ms → 2 s, ≤ 10 s total).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use crate::error::{VcsError, VcsResult};

/// Default timeout for local operations.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Total budget for `*.lock` retries.
pub const LOCK_RETRY_BUDGET: Duration = Duration::from_secs(10);

/// Output of a finished git process.
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// Exit code (`None` when killed by a signal).
    pub code: Option<i32>,
    /// Raw stdout.
    pub stdout: Vec<u8>,
    /// Stderr (lossy UTF-8, trimmed).
    pub stderr: String,
}

impl GitOutput {
    /// Exit code 0.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Stdout as lossy UTF-8.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// First stdout line, trimmed.
    #[must_use]
    pub fn line(&self) -> String {
        self.text().lines().next().unwrap_or_default().trim().to_owned()
    }
}

/// Per-invocation options.
#[derive(Debug, Clone, Default)]
pub struct RunOptions<'a> {
    /// Bytes written to stdin.
    pub stdin: Option<&'a [u8]>,
    /// Read-only command: sets `GIT_OPTIONAL_LOCKS=0`, no lock retries.
    pub read_only: bool,
    /// Override of the timeout.
    pub timeout: Option<Duration>,
}

impl RunOptions<'_> {
    /// Read-only options.
    #[must_use]
    pub fn read() -> Self {
        Self { read_only: true, ..Self::default() }
    }

    /// Mutating options.
    #[must_use]
    pub fn write() -> Self {
        Self::default()
    }
}

/// Spawns git processes.
#[derive(Debug, Clone)]
pub struct GitRunner {
    git: Option<PathBuf>,
    timeout: Duration,
}

impl GitRunner {
    /// Runner for an explicit git binary (`None` = git missing).
    #[must_use]
    pub fn new(git: Option<PathBuf>) -> Self {
        Self { git, timeout: DEFAULT_TIMEOUT }
    }

    /// Finds `git` on PATH.
    #[must_use]
    pub fn discover() -> Self {
        Self::new(orkestr_platform::process::which("git"))
    }

    /// Changes the default timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Resolved git binary.
    #[must_use]
    pub fn git_path(&self) -> Option<&Path> {
        self.git.as_deref()
    }

    /// Runs `git <args>` in `dir`; returns the output whatever the exit code.
    pub async fn run<I, S>(&self, dir: &Path, args: I, opts: RunOptions<'_>) -> VcsResult<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let git = self.git.as_ref().ok_or(VcsError::GitNotFound)?;
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        let command = args.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
        let timeout = opts.timeout.unwrap_or(self.timeout);
        let started = Instant::now();
        let mut delay = Duration::from_millis(50);
        loop {
            let out = self.run_once(git, dir, &args, &opts, &command, timeout).await?;
            if !opts.read_only && !out.success() && is_lock_error(&out.stderr) {
                if started.elapsed() + delay > LOCK_RETRY_BUDGET {
                    return Err(VcsError::Locked(first_line(&out.stderr)));
                }
                tracing::debug!(command, delay_ms = delay.as_millis() as u64, "git lock busy, retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(2));
                continue;
            }
            tracing::debug!(
                command,
                dir = %dir.display(),
                code = ?out.code,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "git finished"
            );
            return Ok(out);
        }
    }

    /// Runs and fails with [`VcsError::Command`] on a non-zero exit code.
    pub async fn run_ok<I, S>(&self, dir: &Path, args: I, opts: RunOptions<'_>) -> VcsResult<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
        let out = self.run(dir, &args, opts).await?;
        if out.success() {
            Ok(out)
        } else {
            Err(command_error(&args, &out))
        }
    }

    async fn run_once(
        &self,
        git: &Path,
        dir: &Path,
        args: &[OsString],
        opts: &RunOptions<'_>,
        command: &str,
        timeout: Duration,
    ) -> VcsResult<GitOutput> {
        let mut std_cmd = orkestr_platform::process::background_command(git);
        std_cmd
            .arg("--no-pager")
            .args(["-c", "core.quotepath=off", "-c", "color.ui=never", "-c", "gc.auto=0", "-c", "core.longpaths=true"])
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(if opts.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if opts.read_only {
            std_cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                if dir.is_dir() {
                    VcsError::GitNotFound
                } else {
                    VcsError::NotFound(format!("directory {} does not exist", dir.display()))
                }
            } else {
                VcsError::Io(e)
            }
        })?;
        if let (Some(input), Some(mut stdin)) = (opts.stdin, child.stdin.take()) {
            let input = input.to_vec();
            // Write in the background so a full stdout pipe cannot deadlock us.
            tokio::spawn(async move {
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            });
        }
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => Ok(GitOutput {
                code: out.status.code(),
                stdout: out.stdout,
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            }),
            Ok(Err(e)) => Err(VcsError::Io(e)),
            Err(_) => {
                tracing::warn!(command, dir = %dir.display(), "git timed out; killed");
                Err(VcsError::Timeout { command: command.to_owned(), secs: timeout.as_secs() })
            }
        }
    }
}

/// Builds a [`VcsError::Command`] (or a more specific error) from a failed run.
pub(crate) fn command_error(args: &[OsString], out: &GitOutput) -> VcsError {
    let command = args.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    let stderr = out.stderr.clone();
    if stderr.contains("not a git repository") {
        return VcsError::NotARepository(first_line(&stderr));
    }
    VcsError::Command {
        command,
        code: out.code,
        stderr: if stderr.is_empty() { out.text().trim().to_owned() } else { stderr },
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_owned()
}

/// Whether stderr reports a busy `*.lock` file.
#[must_use]
pub fn is_lock_error(stderr: &str) -> bool {
    stderr.contains(".lock")
        && (stderr.contains("File exists")
            || stderr.contains("Unable to create")
            || stderr.contains("unable to create"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lock_errors() {
        assert!(is_lock_error(
            "fatal: Unable to create 'C:/r/.git/index.lock': File exists.\n\nAnother git process seems to be running"
        ));
        assert!(!is_lock_error("fatal: pathspec 'x' did not match any files"));
    }

    #[tokio::test]
    async fn missing_git_is_reported() {
        let r = GitRunner::new(None);
        let err = r.run(Path::new("."), ["status"], RunOptions::read()).await.unwrap_err();
        assert!(matches!(err, VcsError::GitNotFound));
    }
}

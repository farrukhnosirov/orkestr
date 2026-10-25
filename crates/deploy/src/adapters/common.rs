//! Helpers shared by the built-in adapters: settings, shell commands with
//! cancellation and tree kill, health waits, directory copies, file walks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::{DeployError, DeployRequest, LogSink};

/// Trimmed, non-empty setting.
pub fn setting<'a>(req: &'a DeployRequest, key: &str) -> Option<&'a str> {
    req.settings.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// Required setting.
pub fn required<'a>(req: &'a DeployRequest, key: &str) -> Result<&'a str, DeployError> {
    setting(req, key).ok_or_else(|| DeployError::Invalid(format!("adapter setting `{key}` is required")))
}

/// Numeric setting with a default.
pub fn number<T: std::str::FromStr>(req: &DeployRequest, key: &str, default: T) -> Result<T, DeployError> {
    match setting(req, key) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| DeployError::Invalid(format!("adapter setting `{key}` must be a number"))),
    }
}

/// Path under the source dir (no escape).
pub fn under_source(req: &DeployRequest, rel: &str) -> Result<PathBuf, DeployError> {
    orkestr_security::confine(&req.source_dir, rel)
        .map_err(|e| DeployError::Invalid(format!("`{rel}` is outside the source directory: {e}")))
}

/// Directory-safe form of a version (`v1.2/abc` → `v1.2_abc`).
#[must_use]
pub fn sanitize(version: &str) -> String {
    let s: String = version
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .take(80)
        .collect();
    let s = s.trim_matches('.').to_owned();
    if s.is_empty() {
        "unversioned".into()
    } else {
        s
    }
}

/// A platform shell command for `line` (`cmd /d /s /c` on Windows, `sh -c` elsewhere).
#[must_use]
pub fn shell(line: &str) -> Command {
    #[cfg(windows)]
    let mut cmd = {
        let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let mut c = Command::new(comspec);
        c.arg("/d").arg("/s").arg("/c");
        orkestr_platform::process::push_raw_arg(&mut c, &format!("\"{line}\""));
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(line);
        c
    };
    orkestr_platform::process::isolate_command(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    cmd
}

/// Kills a process and its whole tree (best effort).
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut c = orkestr_platform::process::background_command("taskkill");
        c.args(["/F", "/T", "/PID", &pid.to_string()]);
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let _ = c.status();
    }
    #[cfg(not(windows))]
    {
        // The child leads its own process group (`isolate_command`).
        let _ = orkestr_platform::process::background_command("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        orkestr_platform::process::kill_pids(&[pid]);
    }
}

/// Kills `child` (tree) and reaps it.
pub async fn kill_child(child: &mut Child) {
    if let Some(pid) = child.id() {
        tokio::task::spawn_blocking(move || kill_tree(pid)).await.ok();
    }
    let _ = child.kill().await;
}

/// Forwards stdout/stderr lines of `child` to `log` (prefixing stderr).
pub fn pipe_output(child: &mut Child, log: &LogSink) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();
    if let Some(out) = child.stdout.take() {
        let log = log.clone();
        handles.push(tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                log.line(&l);
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let log = log.clone();
        handles.push(tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                log.line(&format!("[stderr] {l}"));
            }
        }));
    }
    handles
}

/// Runs a shell command to completion, streaming output; kills the tree on cancel.
pub async fn run_shell(
    line: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    log: &LogSink,
    cancel: &CancellationToken,
) -> Result<(), DeployError> {
    log.line(&format!("$ {line}"));
    let mut cmd = shell(line);
    cmd.current_dir(cwd).envs(env);
    run_command(cmd, line, log, cancel).await
}

/// Runs a prepared command to completion, streaming output; kills the tree on cancel.
pub async fn run_command(
    mut cmd: Command,
    display: &str,
    log: &LogSink,
    cancel: &CancellationToken,
) -> Result<(), DeployError> {
    orkestr_platform::process::isolate_command(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| DeployError::Failed(format!("cannot start `{display}`: {e}")))?;
    let readers = pipe_output(&mut child, log);
    tokio::select! {
        status = child.wait() => {
            let status = status?;
            for r in readers {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(2), r).await;
            }
            if status.success() {
                Ok(())
            } else {
                Err(DeployError::Failed(format!(
                    "`{display}` exited with {}",
                    status.code().map_or("a signal".into(), |c| c.to_string())
                )))
            }
        }
        () = cancel.cancelled() => {
            kill_child(&mut child).await;
            Err(DeployError::Cancelled)
        }
    }
}

/// Polls `url` until it answers 2xx/3xx, `timeout` passes, or `cancel` fires.
pub async fn wait_healthy(
    url: &str,
    timeout: Duration,
    log: &LogSink,
    cancel: &CancellationToken,
) -> Result<(), DeployError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| DeployError::Internal(e.to_string()))?;
    log.line(&format!("waiting for {url} to become healthy (up to {}s)", timeout.as_secs()));
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cancel.is_cancelled() {
            return Err(DeployError::Cancelled);
        }
        let last = match client.get(url).send().await {
            Ok(r) if r.status().is_success() || r.status().is_redirection() => {
                log.line(&format!("health check {url} → {}", r.status().as_u16()));
                return Ok(());
            }
            Ok(r) => format!("HTTP {}", r.status().as_u16()),
            Err(e) if e.is_connect() => "connection refused".to_owned(),
            Err(e) => e.to_string(),
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(DeployError::Failed(format!("health check {url} failed: {last}")));
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(250)) => {}
            () = cancel.cancelled() => return Err(DeployError::Cancelled),
        }
    }
}

/// Whether a relative path segment is skipped by copies and uploads.
fn skipped(name: &str, excludes: &[String]) -> bool {
    name == ".git" || excludes.iter().any(|e| e == name)
}

/// Copies `src` into a fresh `dst` (blocking; replaces an existing `dst` atomically).
pub async fn copy_dir(src: PathBuf, dst: PathBuf, excludes: Vec<String>) -> Result<u64, DeployError> {
    tokio::task::spawn_blocking(move || {
        if !src.is_dir() {
            return Err(DeployError::Invalid(format!("directory {} does not exist", src.display())));
        }
        let parent = dst.parent().ok_or_else(|| DeployError::Internal("no parent".into()))?;
        std::fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(
            ".tmp-{}-{}",
            dst.file_name().and_then(|n| n.to_str()).unwrap_or("v"),
            std::process::id()
        ));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
        let n = copy_rec(&src, &tmp, &excludes)?;
        if dst.exists() {
            std::fs::remove_dir_all(&dst)?;
        }
        std::fs::rename(&tmp, &dst)?;
        Ok(n)
    })
    .await
    .map_err(|e| DeployError::Internal(e.to_string()))?
}

fn copy_rec(src: &Path, dst: &Path, excludes: &[String]) -> Result<u64, DeployError> {
    std::fs::create_dir_all(dst)?;
    let mut n = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_s) = name.to_str() else { continue };
        if skipped(name_s, excludes) {
            continue;
        }
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            n += copy_rec(&entry.path(), &dst.join(&name), excludes)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), dst.join(&name))?;
            n += 1;
        }
    }
    Ok(n)
}

/// One file to upload: `/`-separated relative path, absolute path, bytes.
#[derive(Debug, Clone)]
pub struct UploadFile {
    /// Relative path (`assets/app.js`).
    pub rel: String,
    /// Contents.
    pub bytes: Vec<u8>,
    /// SHA-1 hex digest (Vercel / Netlify file digests).
    pub sha1: String,
}

/// Max files / bytes uploaded to a cloud provider in one deployment.
const MAX_FILES: usize = 5000;
const MAX_BYTES: u64 = 200 * 1024 * 1024;

/// Collects files under `dir` for upload (skips `.git`, `node_modules`, `.env*`).
pub async fn collect_files(dir: PathBuf) -> Result<Vec<UploadFile>, DeployError> {
    tokio::task::spawn_blocking(move || {
        if !dir.is_dir() {
            return Err(DeployError::Invalid(format!("directory {} does not exist", dir.display())));
        }
        let mut out = Vec::new();
        let mut total = 0u64;
        walk(&dir, "", &mut out, &mut total)?;
        out.sort_by(|a, b| a.rel.cmp(&b.rel));
        Ok(out)
    })
    .await
    .map_err(|e| DeployError::Internal(e.to_string()))?
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<UploadFile>, total: &mut u64) -> Result<(), DeployError> {
    use sha1::Digest;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == ".git" || name == "node_modules" || name.starts_with(".env") {
            continue;
        }
        let rel = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&entry.path(), &rel, out, total)?;
        } else if ft.is_file() {
            let bytes = std::fs::read(entry.path())?;
            *total += bytes.len() as u64;
            if out.len() >= MAX_FILES || *total > MAX_BYTES {
                return Err(DeployError::Invalid(format!(
                    "too many files to upload (max {MAX_FILES} files / {} MB)",
                    MAX_BYTES / 1024 / 1024
                )));
            }
            let sha1 = hex::encode(sha1::Sha1::digest(&bytes));
            out.push(UploadFile { rel, bytes, sha1 });
        }
    }
    Ok(())
}

/// Environment for commands: environment variables, then credentials, then extras.
#[must_use]
pub fn command_env(req: &DeployRequest, with_credentials: bool) -> BTreeMap<String, String> {
    let mut env = req.environment.variables.clone();
    if with_credentials {
        for (k, v) in &req.credentials.0 {
            env.insert(k.clone(), v.clone());
        }
    }
    env.insert("ORKESTR_DEPLOY_VERSION".into(), req.version.clone());
    env.insert("ORKESTR_DEPLOY_ENVIRONMENT".into(), req.environment.name.clone());
    env
}

/// Reads a small text file (`None` when missing).
pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Writes a small text file atomically (temp + rename).
pub fn write_atomic(path: &Path, text: &str) -> Result<(), DeployError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_versions() {
        assert_eq!(sanitize("v1.2/abc"), "v1.2_abc");
        assert_eq!(sanitize(".."), "unversioned");
        assert_eq!(sanitize(""), "unversioned");
    }
}

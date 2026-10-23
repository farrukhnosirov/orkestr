//! Helper processes (whisper-cli, piper, sherpa-onnx, PowerShell): argument
//! vectors only (never a shell), no console window, stdin fed from memory,
//! killed on timeout.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::VoiceError;

/// Output of a finished helper.
#[derive(Debug)]
pub struct Output {
    /// Stdout (UTF-8, lossy).
    pub stdout: String,
    /// Stderr (UTF-8, lossy).
    pub stderr: String,
}

/// Runs `program args…` with optional stdin and env, failing on a non-zero
/// exit or after `timeout`.
pub async fn run(
    program: &Path,
    args: &[String],
    env: &[(&str, String)],
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, VoiceError> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = program.parent().filter(|d| !d.as_os_str().is_empty()) {
        cmd.current_dir(dir);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    orkestr_platform::process::isolate_command(&mut cmd);
    let name = program.file_name().map_or_else(|| program.display().to_string(), |n| n.to_string_lossy().into_owned());
    let mut child = cmd.spawn().map_err(|e| VoiceError::Failed(format!("could not start {name}: {e}")))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let input = input.to_vec();
        tokio::spawn(async move {
            let _ = pipe.write_all(&input).await;
            let _ = pipe.shutdown().await;
        });
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| VoiceError::Failed(format!("{name} timed out after {} s", timeout.as_secs())))?
        .map_err(|e| VoiceError::Failed(format!("{name}: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        let tail: String = stderr.trim().chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(VoiceError::Failed(format!("{name} failed ({}): {tail}", out.status)));
    }
    Ok(Output { stdout, stderr })
}

/// A PowerShell script as `-EncodedCommand` arguments (UTF-16LE base64), so
/// no script file and no quoting are involved. Parameters go through env vars.
#[must_use]
pub fn powershell_args(script: &str) -> Vec<String> {
    use base64::Engine as _;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        base64::engine::general_purpose::STANDARD.encode(utf16),
    ]
}

/// Windows PowerShell 5.1 (`powershell.exe`).
#[must_use]
pub fn powershell() -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe")
}

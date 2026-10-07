//! Interactive Claude Code chat sessions over the official `claude` CLI
//! (the Claude Code panel; protocol in `orkestr_protocol::methods::claude_code`).
//!
//! * [`discovery`] — finds the binary (pinned path, the `anthropic.claude-code`
//!   VS Code / native extension, `PATH`).
//! * [`parse`] — `stream-json` output → `ClaudeCodeEvent`s + control messages.
//! * [`control`] — stream-json input lines (user turns, control requests and
//!   permission answers).
//! * [`manager`] — one supervised process per chat session, permission
//!   routing through a [`PermissionPolicy`].
//! * [`history`] — past conversations from the CLI's own transcripts.
//!
//! ## Control protocol (verified against Claude Code 2.1.292)
//!
//! Launch: `claude --print --input-format stream-json --output-format
//! stream-json --verbose --include-partial-messages --permission-prompt-tool
//! stdio --permission-mode <mode> [--allow-dangerously-skip-permissions]
//! [--model M] [--resume ID]`.
//!
//! * Orkestr → CLI `{"type":"control_request","request_id":R,"request":{"subtype":S,…}}`
//!   with `S` ∈ `initialize` (answer: models, commands, account,
//!   `current_permission_mode`, …), `interrupt` (answer: `{still_queued:[]}`;
//!   the turn ends with a `user` "[Request interrupted by user]" line and a
//!   `result` with `subtype:"error_during_execution"`), `set_permission_mode
//!   {mode}` (answer `{mode}` + a `system/status` line with `permissionMode`;
//!   `bypassPermissions` fails with `error_code:"bypass_not_launched"` unless
//!   the process was started with `--allow-dangerously-skip-permissions`),
//!   `set_model {model}` (empty answer + a `<local-command-stdout>` user line).
//!   Unknown subtypes are answered `{"subtype":"error","error":"Unsupported …"}`.
//! * CLI → Orkestr `{"type":"control_request","request_id":R,"request":{"subtype":
//!   "can_use_tool","tool_name","display_name","input","description",
//!   "permission_suggestions":[…],"blocked_path"?,"tool_use_id"}}` — answered with
//!   `{"type":"control_response","response":{"subtype":"success","request_id":R,
//!   "response":{"behavior":"allow","updatedInput":…,"updatedPermissions"?:[…]}
//!   | {"behavior":"deny","message":…}}}`. A deny becomes an `is_error`
//!   tool result with the message; `updatedPermissions` with `destination:
//!   "session"` (e.g. `setMode acceptEdits`) apply to the session only.
//! * Not signed in: `initialize` reports `account: {tokenSource: "none"}` and a
//!   turn yields a synthetic assistant message "Not logged in · Please run
//!   /login" (`error: "authentication_failed"`).

pub mod control;
pub mod discovery;
pub mod history;
pub mod manager;
pub mod parse;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub use manager::{
    ClaudeSessionManager, LaunchSpec, NoPolicy, PermissionOutcome, PermissionPolicy, PolicyContext, PolicyVerdict,
    SessionError, SessionEvent,
};
use orkestr_protocol::methods::claude_code::ClaudeAuthInfo;

/// Timeout of `--version` / `auth status` probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

async fn run_probe(program: &Path, args: &[&str]) -> Result<(Option<i32>, String), String> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let mut cmd = manager::sanitized_command(program, &args);
    cmd.stdin(Stdio::null());
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("`{}` did not answer within {}s", program.display(), PROBE_TIMEOUT.as_secs()))?
        .map_err(|e| format!("could not run `{}`: {e}", program.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}

/// `claude --version` → `2.1.292`.
pub async fn probe_version(program: &Path) -> Result<String, String> {
    let (code, text) = run_probe(program, &["--version"]).await?;
    if code != Some(0) {
        return Err(text.trim().chars().take(300).collect());
    }
    crate::adapters::cli::parse_version(&text).ok_or_else(|| format!("unexpected version output: {}", text.trim()))
}

/// `claude auth status` → login state + the CLI's projects directory.
pub async fn probe_auth(program: &Path) -> Result<(ClaudeAuthInfo, Option<PathBuf>), String> {
    let (_, text) = run_probe(program, &["auth", "status"]).await?;
    parse::auth_status(&text).map(|(info, dir)| (info, dir.map(PathBuf::from))).ok_or_else(|| {
        format!("unexpected `claude auth status` output: {}", text.trim().chars().take(300).collect::<String>())
    })
}

//! Claude Code panel wiring (`claudeCode.*`): binary discovery + login state
//! cache, the user's pinned binary path (`<data_dir>/claude-code.json`), the
//! permission policy (DangerousActionGuard + audit log) and the bridge that
//! publishes session events. Everything specific to the CLI lives in
//! `orkestr_providers::claude_code_session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{now, AuditOutcome, Capability, ProviderKind, UsageRecord, UsageRecordId};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, FileChangeActor, FileChangeKind, FileChangedPayload};
use orkestr_protocol::methods::claude_code::{
    ClaudeAuthInfo, ClaudeBinaryInfo, ClaudeCodeEvent, ClaudeCodeEventPayload, ClaudeCodeStatus,
    ClaudeCodeStatusPayload, ClaudeRisk, ClaudeRiskLevel,
};
use orkestr_protocol::NewEvent;
use orkestr_providers::claude_code_session::{
    self as ccs, discovery, history, ClaudeSessionManager, PermissionOutcome, PermissionPolicy, PolicyContext,
    PolicyVerdict, SessionEvent,
};
use orkestr_security::{Action, DangerClass, NewAudit, Security, ShellKind};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::Services;

/// User configuration file name (inside the data dir).
const CONFIG_FILE: &str = "claude-code.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    #[serde(default)]
    binary_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Cache {
    checked: bool,
    binary: Option<ClaudeBinaryInfo>,
    override_error: Option<String>,
    auth: Option<ClaudeAuthInfo>,
    auth_error: Option<String>,
    projects_dir: Option<PathBuf>,
}

/// Claude Code panel service.
pub struct ClaudeCodeService {
    /// Session supervisor.
    pub manager: ClaudeSessionManager,
    config_file: PathBuf,
    vscode_extensions: PathBuf,
    cache: Mutex<Cache>,
    refresh: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ClaudeCodeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCodeService").field("config_file", &self.config_file).finish_non_exhaustive()
    }
}

impl ClaudeCodeService {
    /// Creates the service.
    #[must_use]
    pub fn new(manager: ClaudeSessionManager, data_dir: &Path, vscode_extensions: PathBuf) -> Self {
        Self {
            manager,
            config_file: data_dir.join(CONFIG_FILE),
            vscode_extensions,
            cache: Mutex::default(),
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    fn config(&self) -> ConfigFile {
        std::fs::read_to_string(&self.config_file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Pins (or clears) the binary path.
    pub fn set_override(&self, path: Option<String>) -> Result<(), String> {
        let path = path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
        if let Some(p) = &path {
            discovery::check_override(p)?;
        }
        let mut cfg = self.config();
        cfg.binary_path = path;
        let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        let tmp = self.config_file.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("cannot save the Claude Code settings: {e}"))?;
        std::fs::rename(&tmp, &self.config_file).map_err(|e| format!("cannot save the Claude Code settings: {e}"))?;
        self.cache.lock().checked = false;
        Ok(())
    }

    /// Status (cached unless `refresh` or never checked).
    pub async fn status(&self, native_extensions: Vec<(String, PathBuf)>, refresh: bool) -> ClaudeCodeStatus {
        if refresh || !self.cache.lock().checked {
            let _guard = self.refresh.lock().await;
            if refresh || !self.cache.lock().checked {
                self.check(native_extensions).await;
            }
        }
        let c = self.cache.lock().clone();
        ClaudeCodeStatus {
            binary: c.binary,
            binary_override: self.config().binary_path,
            override_error: c.override_error,
            auth: c.auth,
            auth_error: c.auth_error,
            sessions: self.manager.list(None),
        }
    }

    async fn check(&self, native_extensions: Vec<(String, PathBuf)>) {
        let input = discovery::DiscoveryInput {
            override_path: self.config().binary_path,
            vscode_extension_roots: vec![self.vscode_extensions.clone()],
            native_extensions,
        };
        let (found, override_error) = tokio::task::spawn_blocking(move || {
            discovery::discover(&input, &|n: &str| orkestr_platform::process::which(n))
        })
        .await
        .unwrap_or((None, None));
        let mut next = Cache { checked: true, override_error, ..Cache::default() };
        if let Some(d) = found {
            let version = ccs::probe_version(&d.path).await.ok();
            match ccs::probe_auth(&d.path).await {
                Ok((auth, dir)) => {
                    next.auth = Some(auth);
                    next.projects_dir = dir;
                }
                Err(e) => next.auth_error = Some(e),
            }
            next.binary = Some(ClaudeBinaryInfo { path: d.path.display().to_string(), source: d.source, version });
        }
        *self.cache.lock() = next;
    }

    /// The binary to run (after a status check).
    pub async fn binary(&self, native_extensions: Vec<(String, PathBuf)>) -> Option<PathBuf> {
        self.status(native_extensions, false).await.binary.map(|b| PathBuf::from(b.path))
    }

    /// The CLI's per-project transcripts directory.
    #[must_use]
    pub fn projects_dir(&self) -> Option<PathBuf> {
        self.cache.lock().projects_dir.clone().or_else(history::default_projects_dir)
    }

    /// Updates the cached login state from a session.
    pub fn note_auth(&self, auth: ClaudeAuthInfo) {
        let mut c = self.cache.lock();
        if c.checked {
            // Keep e-mail / subscription from `auth status` when a session only reports the flag.
            let merged = match c.auth.take() {
                Some(old) if auth.logged_in && auth.email.is_none() => ClaudeAuthInfo { logged_in: true, ..old },
                _ => auth,
            };
            c.auth = Some(merged);
            c.auth_error = None;
        }
    }
}

/// Permission policy: shell commands and file paths through the
/// `DangerousActionGuard`; every decision in the audit log.
pub struct GuardPolicy {
    security: Arc<Security>,
}

impl GuardPolicy {
    /// Creates the policy.
    #[must_use]
    pub fn new(security: Arc<Security>) -> Self {
        Self { security }
    }
}

fn risk_of(class: DangerClass) -> Option<ClaudeRisk> {
    match class {
        DangerClass::Safe => None,
        DangerClass::Dangerous { category, reason } => {
            Some(ClaudeRisk { level: ClaudeRiskLevel::Dangerous, category, reason })
        }
        DangerClass::Forbidden { category, reason } => {
            Some(ClaudeRisk { level: ClaudeRiskLevel::Forbidden, category, reason })
        }
    }
}

fn str_field<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| input.get(*k).and_then(Value::as_str))
}

/// Capability a Claude Code tool exercises (audit).
#[must_use]
pub fn tool_capability(tool: &str) -> Capability {
    match tool {
        "Bash" | "PowerShell" | "BashOutput" | "KillShell" | "KillBash" => Capability::Terminal,
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Capability::Write,
        "WebFetch" | "WebSearch" => Capability::Network,
        _ => Capability::Read,
    }
}

/// Classifies one tool request with the guard (pure; tested).
#[must_use]
pub fn assess_tool(security: &Security, cwd: &Path, tool: &str, input: &Value) -> Option<ClaudeRisk> {
    let guard = &security.guard;
    let class = match tool {
        "Bash" => str_field(input, &["command"]).map(|c| guard.classify_in(c, ShellKind::Bash, Some(cwd))),
        "PowerShell" => str_field(input, &["command"]).map(|c| guard.classify_in(c, ShellKind::PowerShell, Some(cwd))),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => str_field(input, &["file_path", "notebook_path"])
            .map(|p| guard.classify_action(&Action::WriteFile { path: p.to_owned() })),
        "Read" => {
            str_field(input, &["file_path"]).map(|p| guard.classify_action(&Action::ReadFile { path: p.to_owned() }))
        }
        _ => None,
    };
    class.and_then(risk_of)
}

fn summary(tool: &str, input: &Value) -> String {
    let s =
        str_field(input, &["command", "file_path", "notebook_path", "url", "query", "pattern", "path", "description"])
            .unwrap_or_default();
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = one_line.chars().take(300).collect();
    if short.is_empty() {
        tool.to_owned()
    } else {
        format!("{tool}: {short}")
    }
}

#[async_trait]
impl PermissionPolicy for GuardPolicy {
    fn assess(&self, ctx: &PolicyContext, tool_name: &str, input: &Value) -> PolicyVerdict {
        PolicyVerdict { risk: assess_tool(&self.security, &ctx.cwd, tool_name, input) }
    }

    async fn record(
        &self,
        ctx: &PolicyContext,
        tool_name: &str,
        input: &Value,
        risk: Option<&ClaudeRisk>,
        outcome: PermissionOutcome,
    ) {
        let (audit_outcome, label, remembered) = match outcome {
            PermissionOutcome::AutoDenied => (AuditOutcome::Denied, "autoDenied", false),
            PermissionOutcome::Allowed { session } => (AuditOutcome::Approved, "allowed", session),
            PermissionOutcome::Denied => (AuditOutcome::Rejected, "denied", false),
            PermissionOutcome::Cancelled => (AuditOutcome::Rejected, "cancelled", false),
        };
        let entry =
            NewAudit::new(self.security.user(), "claudeCode.permission", summary(tool_name, input), audit_outcome)
                .capability(tool_capability(tool_name))
                .project(Some(ctx.project_id))
                .details(serde_json::json!({
                    "sessionId": ctx.session_id,
                    "tool": tool_name,
                    "outcome": label,
                    "rememberedForSession": remembered,
                    "risk": risk,
                    "cwd": ctx.cwd.display().to_string(),
                }));
        self.security.audit.record(entry).await;
    }
}

/// Publishes session events (ephemeral `ClaudeCodeEvent` / `ClaudeCodeStatus`),
/// mirrors file writes as `FileChanged` and records usage.
pub fn spawn_bridge(services: Arc<Services>, mut rx: mpsc::Receiver<SessionEvent>) {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                SessionEvent::Status(session) => {
                    publish(&services.events, EventKind::ClaudeCodeStatus(ClaudeCodeStatusPayload { session })).await;
                }
                SessionEvent::Item { session_id, project_id, cwd, event } => {
                    side_effects(&services, session_id, project_id, &cwd, &event).await;
                    publish(
                        &services.events,
                        EventKind::ClaudeCodeEvent(ClaudeCodeEventPayload { session_id, project_id, event }),
                    )
                    .await;
                }
            }
        }
    });
}

async fn publish(bus: &EventBus, kind: EventKind) {
    if let Err(e) = bus.publish(NewEvent::new(kind)).await {
        tracing::debug!(error = %e, "failed to publish a Claude Code event");
    }
}

async fn side_effects(
    services: &Services,
    session_id: orkestr_domain::ClaudeSessionId,
    project_id: orkestr_domain::ProjectId,
    cwd: &Path,
    event: &ClaudeCodeEvent,
) {
    match event {
        ClaudeCodeEvent::Capabilities { account: Some(a), .. } => services.claude.note_auth(a.clone()),
        ClaudeCodeEvent::Error { auth_required: true, .. } => {
            services.claude.note_auth(ClaudeAuthInfo { logged_in: false, ..ClaudeAuthInfo::default() });
        }
        ClaudeCodeEvent::FileChanged { path } => {
            let root = match services.projects.get(project_id).await {
                Ok(Some(p)) => p.root_path.map(PathBuf::from),
                _ => None,
            };
            let full = if Path::new(path).is_absolute() { PathBuf::from(path) } else { cwd.join(path) };
            if let Some(root) = root {
                if let Ok(rel) = orkestr_security::confine_relative(&root, &full.to_string_lossy()) {
                    let kind = EventKind::FileChanged(FileChangedPayload {
                        path: rel,
                        old_path: None,
                        change: FileChangeKind::Modified,
                        actor: FileChangeActor::Agent,
                        agent_id: None,
                    });
                    if let Err(e) = services.events.publish(NewEvent::new(kind).project(project_id)).await {
                        tracing::debug!(error = %e, "failed to publish FileChanged");
                    }
                }
            }
        }
        ClaudeCodeEvent::Result { usage, total_cost_usd, is_error, duration_ms, .. } => {
            // Usage goes to the cost engine when a Claude Code provider is
            // configured (subscription: API-equivalent only, never spend).
            let provider = services.providers.list().into_iter().find(|p| p.provider.kind == ProviderKind::ClaudeCode);
            if let Some(p) = provider {
                let model = services
                    .claude
                    .manager
                    .get(session_id)
                    .ok()
                    .and_then(|i| i.model)
                    .unwrap_or_else(|| "claude-code".into());
                let rec = UsageRecord {
                    id: UsageRecordId::new(),
                    ts: now(),
                    provider_id: p.provider.id,
                    model,
                    project_id: Some(project_id),
                    task_id: None,
                    agent_id: None,
                    execution_id: None,
                    input_tokens: usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens,
                    output_tokens: usage.output_tokens,
                    cached_input_tokens: usage.cache_read_tokens,
                    reasoning_tokens: 0,
                    cost_usd: 0.0,
                    api_equivalent_usd: *total_cost_usd,
                    estimated: false,
                    latency_ms: *duration_ms,
                    success: !is_error,
                };
                if let Err(e) = services.cost.ingest(&rec).await {
                    tracing::debug!(error = %e, "Claude Code usage ingest failed");
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_per_tool() {
        assert_eq!(tool_capability("Bash"), Capability::Terminal);
        assert_eq!(tool_capability("Edit"), Capability::Write);
        assert_eq!(tool_capability("WebFetch"), Capability::Network);
        assert_eq!(tool_capability("Glob"), Capability::Read);
    }

    #[test]
    fn summaries_are_short() {
        let s = summary("Bash", &serde_json::json!({"command": "echo   hi\n  there"}));
        assert_eq!(s, "Bash: echo hi there");
        assert_eq!(summary("TodoWrite", &serde_json::json!({"todos": []})), "TodoWrite");
    }
}

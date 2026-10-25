//! Codex (ChatGPT subscription) wiring — ADR-0047: the official CLI's
//! installer / updater (npm into `<data_dir>/tools/codex`), version check,
//! the shared app-server hub (panel sessions, sign-in, limits, models,
//! threads), the approval policy (Dangerous Action Guard + audit log), the
//! event bridge, automatic provider registration and Codex agent templates.
//!
//! Orkestr never reads Codex's credentials: the account comes from the CLI
//! (`account/read`), the sign-in is the CLI's own ChatGPT flow.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orkestr_domain::{
    now, AgentRole, AgentState, AuditOutcome, Capability, MemoryAccess, ModelRef, ProviderId, ProviderKind,
    TemplateSource, UsageRecord, UsageRecordId, WorktreePolicy,
};
use orkestr_protocol::event::{EventKind, FileChangeActor, FileChangeKind, FileChangedPayload};
use orkestr_protocol::methods::claude_code::ClaudeRisk;
use orkestr_protocol::methods::codex::{
    CodexAccountPayload, CodexBinaryInfo, CodexBinarySource, CodexEvent, CodexEventPayload, CodexLimits,
    CodexModelsResult, CodexStatus, CodexStatusPayload, CodexTemplateInfo, CodexTemplateOrigin,
};
use orkestr_protocol::methods::templates::TemplateDraft;
use orkestr_protocol::NewEvent;
use orkestr_providers::adapters::claude_chat::CliSpawner;
use orkestr_providers::adapters::{cli, codex as codex_adapter};
use orkestr_providers::claude_code_session::PermissionOutcome;
use orkestr_providers::codex_app::hub::{CodexHub, CodexPolicy, CodexPolicyContext, HubEvent};
use orkestr_providers::codex_app::map;
use orkestr_providers::{NewProvider, ProviderPatch};
use orkestr_security::{NewAudit, Security};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::Services;

/// npm package of the official CLI.
pub const NPM_PACKAGE: &str = "@openai/codex";
/// How long the latest-version answer is reused.
pub const LATEST_TTL: Duration = Duration::from_secs(6 * 3600);
/// How long plan limits are reused.
pub const LIMITS_TTL: Duration = Duration::from_secs(60);
/// Role of Codex agent templates (also a routing purpose).
pub const CODEX_ROLE: &str = "Codex";

#[derive(Debug, Default)]
struct Cache {
    checked: bool,
    binary: Option<CodexBinaryInfo>,
    latest: Option<(Instant, Option<String>)>,
    npm: Option<bool>,
    limits: Option<(Instant, CodexLimits)>,
    models: Option<CodexModelsResult>,
    error: Option<String>,
}

/// Codex service.
pub struct CodexService {
    /// The panel hub (one shared app-server process).
    pub hub: CodexHub,
    tools_dir: PathBuf,
    cache: Mutex<Cache>,
    refresh: tokio::sync::Mutex<()>,
    install: tokio::sync::Mutex<()>,
    /// Open the sign-in page in the browser (off in tests).
    pub open_browser: bool,
}

impl std::fmt::Debug for CodexService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexService").field("tools_dir", &self.tools_dir).finish_non_exhaustive()
    }
}

/// npm's arguments for installing / updating the CLI into `prefix` (pure; tested).
#[must_use]
pub fn npm_install_args(prefix: &Path) -> Vec<String> {
    vec![
        "install".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--loglevel=error".into(),
        format!("{NPM_PACKAGE}@latest"),
    ]
}

/// Whether `latest` is newer than `current` (`x.y.z`, numeric parts).
#[must_use]
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split(['.', '-'])
            .take(3)
            .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(current)
}

async fn run_npm(args: &[String], timeout: Duration) -> Result<String, String> {
    let npm = orkestr_platform::process::which("npm")
        .ok_or("npm was not found — install Node.js (https://nodejs.org) first")?;
    let mut cmd = tokio::process::Command::new(&npm);
    cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    orkestr_platform::process::HideConsole::hide_console(&mut cmd);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("npm did not finish within {}s", timeout.as_secs()))?
        .map_err(|e| format!("could not run npm: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

fn tail(text: &str, lines: usize) -> String {
    let v: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

impl CodexService {
    /// Creates the service.
    #[must_use]
    pub fn new(data_dir: &Path, events: mpsc::UnboundedSender<HubEvent>, security: Arc<Security>) -> Self {
        Self {
            hub: CodexHub::new(Arc::new(CliSpawner), events, Arc::new(GuardPolicy { security })),
            tools_dir: data_dir.join("tools").join("codex"),
            cache: Mutex::default(),
            refresh: tokio::sync::Mutex::new(()),
            install: tokio::sync::Mutex::new(()),
            open_browser: true,
        }
    }

    /// Orkestr's own installation folder.
    #[must_use]
    pub fn tools_dir(&self) -> &Path {
        &self.tools_dir
    }

    /// Finds the binary: pinned path (provider setting) → Orkestr's
    /// installation → `PATH`.
    #[must_use]
    pub fn locate(&self, pinned: Option<&str>) -> Option<(PathBuf, CodexBinarySource)> {
        if let Some(p) = pinned.map(str::trim).filter(|p| !p.is_empty()) {
            let path = PathBuf::from(p);
            if path.is_file() {
                let source = if path.starts_with(&self.tools_dir) {
                    CodexBinarySource::Orkestr
                } else {
                    CodexBinarySource::Override
                };
                return Some((path, source));
            }
        }
        if let Some(p) = codex_adapter::tools_binary(&self.tools_dir) {
            return Some((p, CodexBinarySource::Orkestr));
        }
        cli::resolve_binary("codex", None).map(|p| (p, CodexBinarySource::Path))
    }

    /// The binary in use (cached; `refresh` re-probes the version).
    pub async fn binary(&self, pinned: Option<&str>, refresh: bool) -> Option<CodexBinaryInfo> {
        let _g = self.refresh.lock().await;
        {
            let c = self.cache.lock();
            if c.checked && !refresh {
                if let Some(b) = c.binary.as_ref().filter(|b| Path::new(&b.path).is_file()) {
                    return Some(b.clone());
                }
                if c.binary.is_none() && self.locate(pinned).is_none() {
                    return None;
                }
            }
        }
        let found = self.locate(pinned);
        let info = match found {
            Some((path, source)) => {
                let version = match cli::probe(&path, &["--version"]).await {
                    Ok((true, text)) => cli::parse_version(&text),
                    _ => None,
                };
                Some(CodexBinaryInfo { path: path.to_string_lossy().into_owned(), source, version })
            }
            None => None,
        };
        self.hub.set_program(info.as_ref().map(|b| PathBuf::from(&b.path)));
        let mut c = self.cache.lock();
        c.checked = true;
        c.binary.clone_from(&info);
        info
    }

    /// Latest published version (`npm view`), cached.
    pub async fn latest_version(&self, refresh: bool) -> Option<String> {
        if !refresh {
            if let Some((at, v)) = self.cache.lock().latest.clone() {
                if at.elapsed() < LATEST_TTL {
                    return v;
                }
            }
        }
        let args: Vec<String> = ["view", NPM_PACKAGE, "version"].iter().map(|s| (*s).to_owned()).collect();
        let r = run_npm(&args, Duration::from_secs(30)).await;
        let npm_ok = !matches!(&r, Err(e) if e.contains("npm was not found"));
        let v = r.ok().and_then(|t| cli::parse_version(&t));
        let mut c = self.cache.lock();
        c.npm = Some(npm_ok);
        c.latest = Some((Instant::now(), v.clone()));
        v
    }

    /// Installs / updates the CLI into Orkestr's tools folder.
    pub async fn install(&self) -> Result<(PathBuf, String), String> {
        let _g = self.install.lock().await;
        std::fs::create_dir_all(&self.tools_dir)
            .map_err(|e| format!("cannot create {}: {e}", self.tools_dir.display()))?;
        let out =
            run_npm(&npm_install_args(&self.tools_dir), Duration::from_secs(600)).await.map_err(|e| tail(&e, 20))?;
        let bin = codex_adapter::tools_binary(&self.tools_dir)
            .ok_or_else(|| format!("npm finished but the codex binary is missing\n{}", tail(&out, 20)))?;
        self.cache.lock().checked = false;
        Ok((bin, tail(&out, 20)))
    }

    /// Plan limits (cached about a minute).
    pub async fn limits(&self, refresh: bool) -> CodexLimits {
        if !refresh {
            if let Some((at, l)) = self.cache.lock().limits.clone() {
                if at.elapsed() < LIMITS_TTL {
                    return l;
                }
            }
        }
        let l = match self.hub.rate_limits().await {
            Ok(l) => l,
            Err(e) => unavailable(&e.to_string()),
        };
        self.cache.lock().limits = Some((Instant::now(), l.clone()));
        l
    }

    /// Stores limits pushed by the CLI.
    pub fn note_limits(&self, l: &CodexLimits) {
        self.cache.lock().limits = Some((Instant::now(), l.clone()));
    }

    /// Cached limits without asking the CLI.
    #[must_use]
    pub fn cached_limits(&self) -> Option<CodexLimits> {
        self.cache.lock().limits.as_ref().map(|(_, l)| l.clone())
    }

    /// Models (live, else the built-in list).
    pub async fn models(&self, refresh: bool) -> CodexModelsResult {
        if !refresh {
            if let Some(m) = self.cache.lock().models.clone().filter(|m| !m.fallback) {
                return m;
            }
        }
        let r = match self.hub.models().await {
            Ok(m) if !m.is_empty() => CodexModelsResult { models: m, fallback: false, error: None },
            Ok(_) => CodexModelsResult { models: map::fallback_models(), fallback: true, error: None },
            Err(e) => CodexModelsResult { models: map::fallback_models(), fallback: true, error: Some(e.to_string()) },
        };
        self.cache.lock().models = Some(r.clone());
        r
    }

    /// Full status.
    pub async fn status(&self, pinned: Option<&str>, refresh: bool) -> CodexStatus {
        let binary = self.binary(pinned, refresh).await;
        let latest = self.latest_version(false).await;
        let npm_available = self.cache.lock().npm.unwrap_or(false);
        let (logged_in, account, error) = match &binary {
            None => (false, None, Some("Codex CLI is not installed".to_owned())),
            Some(_) => {
                let cached = self.hub.cached_account().filter(|_| !refresh);
                match cached {
                    Some((l, a)) => (l, a, None),
                    None => match tokio::time::timeout(Duration::from_secs(30), self.hub.account()).await {
                        Ok(Ok((l, a))) => (l, a, None),
                        Ok(Err(e)) => (false, None, Some(e.to_string())),
                        Err(_) => (false, None, Some("Codex did not answer".to_owned())),
                    },
                }
            }
        };
        self.cache.lock().error.clone_from(&error);
        let update_available = match (&binary, &latest) {
            (Some(b), Some(l)) => b.version.as_deref().is_some_and(|v| is_newer(l, v)),
            _ => false,
        };
        CodexStatus {
            binary,
            latest_version: latest,
            update_available,
            npm_available,
            logged_in,
            account,
            login: self.hub.login_state(),
            provider_id: None,
            sessions: self.hub.sessions(None),
            error,
        }
    }
}

/// Limits answer when the CLI cannot be asked.
#[must_use]
pub fn unavailable(why: &str) -> CodexLimits {
    CodexLimits {
        available: false,
        plan: None,
        primary: None,
        secondary: None,
        credits: None,
        limit_reached: None,
        usage_allowed: None,
        fetched_at: now(),
        error: Some(why.to_owned()),
    }
}

/// Approval policy: commands and file paths through the Dangerous Action
/// Guard (same rules as the Claude Code panel); every decision audited.
pub struct GuardPolicy {
    security: Arc<Security>,
}

#[async_trait]
impl CodexPolicy for GuardPolicy {
    fn assess(&self, ctx: &CodexPolicyContext, tool: &str, input: &serde_json::Value) -> Option<ClaudeRisk> {
        // Every path of a multi-file change is checked; the worst wins.
        if let Some(paths) = input.get("paths").and_then(serde_json::Value::as_array) {
            for p in paths.iter().filter_map(serde_json::Value::as_str) {
                let one = serde_json::json!({ "file_path": p });
                if let Some(r) = crate::claude_code::assess_tool(&self.security, &ctx.cwd, tool, &one) {
                    return Some(r);
                }
            }
        }
        crate::claude_code::assess_tool(&self.security, &ctx.cwd, tool, input)
    }

    async fn record(
        &self,
        ctx: &CodexPolicyContext,
        tool: &str,
        input: &serde_json::Value,
        risk: Option<&ClaudeRisk>,
        outcome: PermissionOutcome,
    ) {
        let (audit_outcome, label, remembered) = match outcome {
            PermissionOutcome::AutoDenied => (AuditOutcome::Denied, "autoDenied", false),
            PermissionOutcome::Allowed { session } => (AuditOutcome::Approved, "allowed", session),
            PermissionOutcome::Denied => (AuditOutcome::Rejected, "denied", false),
            PermissionOutcome::Cancelled => (AuditOutcome::Rejected, "cancelled", false),
        };
        let what = input
            .get("command")
            .or_else(|| input.get("file_path"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let summary: String =
            format!("{tool}: {}", what.split_whitespace().collect::<Vec<_>>().join(" ")).chars().take(300).collect();
        let entry = NewAudit::new(self.security.user(), "codex.approval", summary, audit_outcome)
            .capability(crate::claude_code::tool_capability(tool))
            .project(Some(ctx.project_id))
            .details(serde_json::json!({
                "sessionId": ctx.session_id,
                "tool": tool,
                "outcome": label,
                "rememberedForSession": remembered,
                "risk": risk,
                "cwd": ctx.cwd.display().to_string(),
            }));
        self.security.audit.record(entry).await;
    }
}

/// The Codex provider, if one is registered (one with a pinned binary first).
#[must_use]
pub fn provider_id(services: &Services) -> Option<ProviderId> {
    codex_provider(services).map(|v| v.provider.id)
}

fn codex_provider(services: &Services) -> Option<orkestr_providers::ProviderView> {
    let all: Vec<_> =
        services.providers.list().into_iter().filter(|v| v.provider.kind == ProviderKind::Codex).collect();
    let pinned = all.iter().position(|v| v.provider.settings.get("binaryPath").and_then(|x| x.as_str()).is_some());
    match pinned {
        Some(i) => all.into_iter().nth(i),
        None => all.into_iter().next(),
    }
}

fn pinned(services: &Services) -> Option<String> {
    codex_provider(services)
        .and_then(|v| v.provider.settings.get("binaryPath").and_then(|x| x.as_str()).map(str::to_owned))
}

/// Status with the provider id (registering it once signed in).
pub async fn status(services: &Services, refresh: bool) -> CodexStatus {
    let pin = pinned(services);
    let mut st = services.codex.status(pin.as_deref(), refresh).await;
    st.provider_id = if st.logged_in { ensure_provider(services, &st).await.ok() } else { provider_id(services) };
    st
}

/// Registers (or re-points) the Codex provider. `Err` = why not.
pub async fn ensure_provider(services: &Services, st: &CodexStatus) -> Result<ProviderId, String> {
    let bin = st.binary.as_ref().ok_or("Codex CLI is not installed")?;
    let pin = (bin.source != CodexBinarySource::Path).then(|| bin.path.clone());
    if let Some(v) = codex_provider(services) {
        let current = v.provider.settings.get("binaryPath").and_then(|x| x.as_str()).map(str::to_owned);
        if bin.source == CodexBinarySource::Orkestr && current.as_deref() != Some(bin.path.as_str()) {
            let patch = ProviderPatch { binary_path: Some(bin.path.clone()), ..ProviderPatch::default() };
            if let Err(e) = services.providers.update(v.provider.id, patch).await {
                tracing::warn!(error = %e, "could not re-point the Codex provider");
            }
        }
        return Ok(v.provider.id);
    }
    if !st.logged_in {
        return Err("Codex is installed but not signed in — sign in with ChatGPT from the Codex panel".into());
    }
    let input = NewProvider {
        kind: Some(ProviderKind::Codex),
        name: Some("Codex".into()),
        default_model: Some("default".into()),
        binary_path: pin,
        ..NewProvider::default()
    };
    let view = services.providers.create(input).await.map_err(|e| e.to_string())?;
    tracing::info!(provider = %view.provider.id, "Codex (ChatGPT subscription) registered as a provider");
    Ok(view.provider.id)
}

/// Registers the provider in the background shortly after start.
pub fn spawn_auto_register(services: Arc<Services>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        let st = status(&services, false).await;
        match st.provider_id {
            Some(id) => tracing::info!(provider = %id, "Codex provider ready"),
            None => tracing::info!(reason = ?st.error, "Codex provider not registered"),
        }
    });
}

/// Publishes hub events, mirrors file changes, records usage, registers the
/// provider after a sign-in.
pub fn spawn_bridge(services: Arc<Services>, mut rx: mpsc::UnboundedReceiver<HubEvent>) {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                HubEvent::Status(session) => {
                    publish(&services, NewEvent::new(EventKind::CodexStatus(CodexStatusPayload { session }))).await;
                }
                HubEvent::Item { session_id, project_id, cwd, event } => {
                    side_effects(&services, project_id, &cwd, &event).await;
                    let kind = EventKind::CodexEvent(CodexEventPayload { session_id, project_id, event });
                    publish(&services, NewEvent::new(kind)).await;
                }
                HubEvent::Account(p) => {
                    if p.logged_in {
                        let st = services.codex.status(pinned(&services).as_deref(), false).await;
                        if let Err(e) = ensure_provider(&services, &st).await {
                            tracing::info!(reason = %e, "Codex provider not registered after sign-in");
                        }
                    }
                    publish(&services, NewEvent::new(EventKind::CodexAccount(p))).await;
                }
                HubEvent::Limits(l) => services.codex.note_limits(&l),
            }
        }
    });
}

async fn publish(services: &Services, ev: NewEvent) {
    if let Err(e) = services.events.publish(ev).await {
        tracing::debug!(error = %e, "failed to publish a Codex event");
    }
}

async fn side_effects(services: &Services, project_id: orkestr_domain::ProjectId, cwd: &Path, event: &CodexEvent) {
    match event {
        CodexEvent::FileChanged { path } => {
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
                    publish(services, NewEvent::new(kind).project(project_id)).await;
                }
            }
        }
        CodexEvent::TokenUsage { last, .. } => {
            // Subscription usage: tokens only, never spend.
            let Some(provider_id) = provider_id(services) else { return };
            let rec = UsageRecord {
                id: UsageRecordId::new(),
                ts: now(),
                provider_id,
                model: "codex".into(),
                project_id: Some(project_id),
                task_id: None,
                agent_id: None,
                execution_id: None,
                input_tokens: last.input_tokens,
                output_tokens: last.output_tokens,
                cached_input_tokens: last.cached_input_tokens,
                reasoning_tokens: last.reasoning_tokens,
                cost_usd: 0.0,
                api_equivalent_usd: None,
                estimated: false,
                latency_ms: None,
                success: true,
            };
            if let Err(e) = services.cost.ingest(&rec).await {
                tracing::debug!(error = %e, "Codex usage ingest failed");
            }
        }
        CodexEvent::RateLimits { limits } => services.codex.note_limits(limits),
        _ => {}
    }
}

/// The account payload for a status.
#[must_use]
pub fn account_payload(st: &CodexStatus) -> CodexAccountPayload {
    CodexAccountPayload { logged_in: st.logged_in, account: st.account.clone(), login: st.login.clone() }
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Built-in Codex agents: `(name, role, description, prompt, writes, shell)`.
pub const BUILTIN: &[(&str, &str, &str, &str, bool, bool)] = &[
    (
        "codex-engineer",
        "Backend",
        "Implements a task end to end with Codex: reads the code, edits, runs the tests.",
        "You are a senior software engineer working through the OpenAI Codex CLI. Read the relevant code first, make focused changes, run the project's tests and fix failures before you finish.",
        true,
        true,
    ),
    (
        "codex-reviewer",
        "Reviewer",
        "Reviews the worktree's changes with Codex (read-only) and reports findings.",
        "You are a meticulous code reviewer. Inspect the changes in this worktree (git diff), look for correctness bugs, security problems and missing tests. Do not modify files; report findings with file:line references.",
        false,
        true,
    ),
    (
        "codex-tester",
        "QA",
        "Writes and runs tests for the task with Codex.",
        "You are a QA engineer. Add or extend automated tests that cover the task, run them, and fix the tests (not the product code) unless a real bug is found — then report it.",
        true,
        true,
    ),
    (
        "codex-explainer",
        "Researcher",
        "Explores the codebase with Codex and answers questions (read-only).",
        "You are a codebase guide. Explore the repository read-only and answer precisely, citing files and symbols.",
        false,
        false,
    ),
];

fn role_of(name: &str) -> AgentRole {
    match name {
        "Backend" => AgentRole::Backend,
        "QA" => AgentRole::Qa,
        _ => AgentRole::Custom(name.to_owned()),
    }
}

fn tools(writes: bool, shell: bool) -> Vec<String> {
    let mut out: Vec<&str> = vec!["read_file", "list_dir", "search_files", "git_status", "git_diff"];
    if writes {
        out.extend(["write_file", "edit_file", "create_snapshot"]);
    }
    if shell {
        out.extend(["run_command", "git_commit"]);
    }
    out.push("submit_result");
    out.into_iter().map(str::to_owned).collect()
}

/// A template draft for a Codex agent.
#[must_use]
pub fn draft(
    name: &str,
    role: AgentRole,
    description: &str,
    prompt: &str,
    writes: bool,
    shell: bool,
    provider: Option<ProviderId>,
) -> TemplateDraft {
    TemplateDraft {
        name: name.to_owned(),
        role,
        description: description.chars().take(480).collect(),
        system_prompt: prompt.chars().take(32_000).collect(),
        preferred_model: provider.map(|provider_id| ModelRef { provider_id, model: "default".into() }),
        fallback_models: Vec::new(),
        tools: tools(writes, shell),
        permissions: Vec::new(),
        memory_access: MemoryAccess::default(),
        budget: None,
        max_turns: 80,
        worktree_policy: WorktreePolicy::default(),
        quality_gates: Vec::new(),
        voice: None,
        avatar: None,
        visual_state: AgentState::default(),
        version: "1.0.0".into(),
    }
}

/// `(name, description, body)` of a `~/.codex/prompts/*.md` file (front
/// matter `description:` honoured; pure; tested).
#[must_use]
pub fn parse_prompt_file(file_name: &str, text: &str) -> Option<(String, String, String)> {
    let stem = file_name.strip_suffix(".md")?;
    let name: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    if name.trim_matches('-').is_empty() {
        return None;
    }
    let (mut description, mut body) = (String::new(), text.to_owned());
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some(d) = line.trim().strip_prefix("description:") {
                    description = d.trim().trim_matches('"').to_owned();
                }
            }
            body = rest[end + 4..].trim_start().to_owned();
        }
    }
    if body.trim().is_empty() {
        return None;
    }
    if description.is_empty() {
        description = body
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .trim_start_matches('#')
            .trim()
            .chars()
            .take(200)
            .collect();
    }
    Some((format!("codex-{}", name.trim_matches('-')), description, body))
}

/// Mirrors the built-in Codex agents, the user's `~/.codex/prompts/*.md` and
/// the project's `AGENTS.md` as templates.
pub async fn templates(services: &Services, project_root: Option<PathBuf>) -> Vec<CodexTemplateInfo> {
    let provider = provider_id(services);
    let mut list: Vec<(CodexTemplateInfo, TemplateDraft)> = Vec::new();
    for (name, role, desc, prompt, w, s) in BUILTIN {
        list.push((
            CodexTemplateInfo {
                name: (*name).to_owned(),
                description: (*desc).to_owned(),
                origin: CodexTemplateOrigin::BuiltIn,
                path: None,
                template_id: None,
            },
            draft(name, role_of(role), desc, prompt, *w, *s, provider),
        ));
    }
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| orkestr_platform::paths::user_home().map(|h| h.join(".codex")));
    if let Some(dir) = codex_home.map(|h| h.join("prompts")) {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            files.sort();
            for path in files.into_iter().take(200) {
                let Some(fname) = path.file_name().and_then(|f| f.to_str()) else { continue };
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if let Some((name, desc, body)) = parse_prompt_file(fname, &text) {
                    list.push((
                        CodexTemplateInfo {
                            name: name.clone(),
                            description: desc.clone(),
                            origin: CodexTemplateOrigin::Prompt,
                            path: Some(path.display().to_string()),
                            template_id: None,
                        },
                        draft(&name, AgentRole::Custom(CODEX_ROLE.into()), &desc, &body, true, true, provider),
                    ));
                }
            }
        }
    }
    if let Some(root) = project_root {
        let path = root.join("AGENTS.md");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !text.trim().is_empty() {
                let desc = "The project's AGENTS.md instructions, run with Codex.";
                list.push((
                    CodexTemplateInfo {
                        name: "codex-project-agent".into(),
                        description: desc.into(),
                        origin: CodexTemplateOrigin::AgentsMd,
                        path: Some(path.display().to_string()),
                        template_id: None,
                    },
                    draft(
                        "codex-project-agent",
                        AgentRole::Custom(CODEX_ROLE.into()),
                        desc,
                        &text,
                        true,
                        true,
                        provider,
                    ),
                ));
            }
        }
    }
    let mut out = Vec::new();
    for (mut info, d) in list {
        match services.agents.templates().upsert_mirrored(d, TemplateSource::Codex).await {
            Ok(t) => info.template_id = Some(t.id),
            Err(e) => tracing::warn!(agent = %info.name, error = %e, "could not mirror a Codex agent as a template"),
        }
        out.push(info);
    }
    let sec = &services.security;
    sec.audit
        .record(
            NewAudit::new(sec.user(), "codex.templates", format!("{} templates", out.len()), AuditOutcome::Executed)
                .capability(Capability::Read),
        )
        .await;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_and_versions() {
        let a = npm_install_args(Path::new("C:/data/tools/codex"));
        assert_eq!(a[0], "install");
        assert!(a.contains(&"--prefix".to_owned()) && a.last().unwrap() == "@openai/codex@latest");
        assert!(!a.iter().any(|x| x == "-g" || x == "--global"), "never a global install");
        assert!(is_newer("0.164.0", "0.163.0"));
        assert!(is_newer("1.0.0", "0.999.9"));
        assert!(!is_newer("0.163.0", "0.163.0"));
        assert!(!is_newer("0.162.9", "0.163.0"));
    }

    #[test]
    fn prompt_files_become_templates() {
        let (n, d, b) =
            parse_prompt_file("Fix Bug.md", "---\ndescription: \"Fix a bug\"\n---\nFind and fix it.").unwrap();
        assert_eq!((n.as_str(), d.as_str(), b.as_str()), ("codex-fix-bug", "Fix a bug", "Find and fix it."));
        let (_, d, _) = parse_prompt_file("x.md", "# Review code\nbody").unwrap();
        assert_eq!(d, "Review code");
        assert!(parse_prompt_file("x.txt", "a").is_none());
        assert!(parse_prompt_file("x.md", "---\ndescription: d\n---\n").is_none());
        assert!(BUILTIN.iter().all(|b| b.0.starts_with("codex-")));
        assert_eq!(tools(false, false).iter().filter(|t| *t == "write_file").count(), 0);
    }
}

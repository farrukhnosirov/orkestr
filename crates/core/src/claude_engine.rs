//! Claude Code as Orkestr's engine (`claudeEngine.*`):
//!
//! * the user's Claude Code subscription becomes a provider automatically
//!   once the official binary is found (Orkestr's VS Code extension folder,
//!   the native extension, `PATH`) and `claude auth status` reports a
//!   signed-in account — its pinned path follows extension updates;
//! * "use Claude Code for everything": AI Manager, every agent role, review,
//!   adversarial review, consensus, PM and merge route to Claude Code, the
//!   previous models stay as fallbacks;
//! * Claude Code's sub-agents (built in + `.claude/agents/*.md` of the user
//!   and the project) are mirrored as agent templates (`source` =
//!   `ClaudeCode`) so they can be spawned on tasks like any Orkestr agent;
//!   their files can be created / edited / deleted (with confirmation).
//!
//! Orkestr never reads the CLI's credentials or settings; only the agent
//! definition files are read and written.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use orkestr_agent_runtime::cli::CLAUDE_BUILTIN_MARKER;
use orkestr_domain::{
    AgentRole, AgentState, AuditOutcome, Capability, MemoryAccess, ModelRef, ProjectId, ProviderId, ProviderKind,
    RoutingRule, TemplateSource, WorktreePolicy,
};
use orkestr_protocol::methods::claude_engine::{
    ClaudeAgentDef, ClaudeAgentFileResult, ClaudeAgentScope, ClaudeAgentsResult, ClaudeEngineStatus,
    DeleteClaudeAgentParams, SaveClaudeAgentParams, UseClaudeForEverythingResult,
};
use orkestr_protocol::methods::templates::TemplateDraft;
use orkestr_protocol::{RpcError, RpcErrorCode};
use orkestr_providers::claude_code_session::agents as cagents;
use orkestr_providers::{NewProvider, ProviderPatch};
use orkestr_security::NewAudit;
use parking_lot::Mutex;

use crate::Services;

/// Default model of the Claude Code provider / of "use for everything".
pub const DEFAULT_MODEL: &str = "sonnet";
/// Role of templates mirroring Claude Code agents (also a routing purpose).
pub const CLAUDE_ROLE: &str = "Claude Code";

/// Routing purposes moved to Claude Code by "use for everything": every
/// built-in agent role, the Manager and the quality / decision roles.
pub const PURPOSES: &[&str] = &[
    "Manager",
    "Architect",
    "Backend",
    "Frontend",
    "Database",
    "QA",
    "Security",
    "DevOps",
    "UI/UX",
    "Product Manager",
    "Reviewer",
    "Researcher",
    "Review",
    "Adversarial Review",
    "Consensus",
    "Coding",
    "Merge",
    CLAUDE_ROLE,
];

fn builtin_cache() -> &'static Mutex<Option<Vec<ClaudeAgentDef>>> {
    static CACHE: OnceLock<Mutex<Option<Vec<ClaudeAgentDef>>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn is_extension_binary(path: &str) -> bool {
    path.to_ascii_lowercase().contains(cagents_ext_marker())
}

fn cagents_ext_marker() -> &'static str {
    orkestr_providers::claude_code_session::discovery::EXTENSION_ID
}

/// The Claude Code provider, if one is registered.
#[must_use]
pub fn provider_id(services: &Services) -> Option<ProviderId> {
    services.providers.list().into_iter().find(|v| v.provider.kind == ProviderKind::ClaudeCode).map(|v| v.provider.id)
}

/// Registers (or re-points) the Claude Code provider. `Err` = why not.
pub async fn ensure_provider(services: &Services, refresh: bool) -> Result<ProviderId, String> {
    let exts = services.extensions.enabled_dirs().await.unwrap_or_default();
    let st = services.claude.status(exts, refresh).await;
    let Some(bin) = st.binary else {
        return Err("Claude Code is not installed (install the Claude Code extension or the CLI)".into());
    };
    let signed_in = st.auth.as_ref().is_some_and(|a| a.logged_in);
    let from_path = bin.source == orkestr_protocol::methods::claude_code::ClaudeBinarySource::Path;
    if let Some(v) = services.providers.list().into_iter().find(|v| v.provider.kind == ProviderKind::ClaudeCode) {
        let current = v.provider.settings.get("binaryPath").and_then(|x| x.as_str()).map(str::to_owned);
        // Follow extension updates; a path the user pinned elsewhere stays.
        let stale = match current.as_deref() {
            None => !from_path,
            Some(c) => c != bin.path && (is_extension_binary(c) || !Path::new(c).is_file()),
        };
        if stale {
            let patch = ProviderPatch { binary_path: Some(bin.path.clone()), ..ProviderPatch::default() };
            if let Err(e) = services.providers.update(v.provider.id, patch).await {
                tracing::warn!(error = %e, "could not re-point the Claude Code provider");
            }
        }
        return Ok(v.provider.id);
    }
    if !signed_in {
        return Err("Claude Code is installed but not signed in — sign in from the Claude Code panel".into());
    }
    let input = NewProvider {
        kind: Some(ProviderKind::ClaudeCode),
        name: Some("Claude Code".into()),
        default_model: Some(DEFAULT_MODEL.into()),
        binary_path: (!from_path).then(|| bin.path.clone()),
        ..NewProvider::default()
    };
    let view = services.providers.create(input).await.map_err(|e| e.to_string())?;
    tracing::info!(provider = %view.provider.id, binary = %bin.path, "Claude Code subscription registered as a provider");
    Ok(view.provider.id)
}

/// Registers the provider in the background shortly after start.
pub fn spawn_auto_register(services: Arc<Services>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        match ensure_provider(&services, false).await {
            Ok(id) => tracing::info!(provider = %id, "Claude Code provider ready"),
            Err(reason) => tracing::info!(%reason, "Claude Code provider not registered"),
        }
    });
}

/// Engine status (optionally re-probing and registering).
pub async fn status(services: &Services, refresh: bool) -> ClaudeEngineStatus {
    let exts = services.extensions.enabled_dirs().await.unwrap_or_default();
    let st = services.claude.status(exts, refresh).await;
    let (provider, reason) = if st.binary.is_some() {
        match ensure_provider(services, false).await {
            Ok(id) => (Some(id), None),
            Err(r) => (None, Some(r)),
        }
    } else {
        (provider_id(services), Some("Claude Code is not installed".to_owned()))
    };
    let mgr = services.providers.manager_config().await.unwrap_or_default();
    let routing = services.providers.routing_config().await.unwrap_or_default();
    let on = |m: &ModelRef| Some(m.provider_id) == provider;
    ClaudeEngineStatus {
        installed: st.binary.is_some(),
        signed_in: st.auth.as_ref().is_some_and(|a| a.logged_in),
        subscription: st.auth.as_ref().and_then(|a| a.subscription_type.clone()),
        provider_id: provider,
        manager_on_claude: mgr.model.as_ref().is_some_and(on),
        roles_on_claude: u32::try_from(routing.rules.iter().filter(|r| on(&r.model)).count()).unwrap_or(u32::MAX),
        reason,
    }
}

fn valid_model(m: &str) -> bool {
    orkestr_providers::claude_code_session::manager::is_valid_model(m)
}

/// Puts `model` first in a (model, fallbacks) pair, keeping the previous
/// model as the first fallback (pure; tested).
pub fn promote(current: &mut ModelRef, fallback: &mut Vec<ModelRef>, model: &ModelRef) {
    if current != model {
        let old = std::mem::replace(current, model.clone());
        if old.provider_id != model.provider_id && !fallback.contains(&old) {
            fallback.insert(0, old);
        }
    }
    fallback.retain(|m| m != model);
    fallback.truncate(8);
}

/// `claudeEngine.useForEverything`.
pub async fn use_for_everything(services: &Services, model: Option<String>) -> Result<UseClaudeForEverythingResult, RpcError> {
    let id = ensure_provider(services, true).await.map_err(RpcError::bad_request)?;
    let model = model.map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into());
    if !valid_model(&model) {
        return Err(RpcError::bad_request(format!("invalid model name `{model}`")));
    }
    let mref = ModelRef { provider_id: id, model };
    let to_rpc = crate::rpc::providers::to_rpc;
    let mut mgr = services.providers.manager_config().await.map_err(to_rpc)?;
    match mgr.model.as_mut() {
        Some(cur) => promote(cur, &mut mgr.fallback_chain, &mref),
        None => {
            mgr.model = Some(mref.clone());
            mgr.fallback_chain.retain(|m| m != &mref);
        }
    }
    let manager = services.providers.set_manager_config(mgr).await.map_err(to_rpc)?;
    let mut routing = services.providers.routing_config().await.map_err(to_rpc)?;
    for purpose in PURPOSES {
        match routing.rules.iter_mut().find(|r| r.purpose.eq_ignore_ascii_case(purpose)) {
            Some(rule) => promote(&mut rule.model, &mut rule.fallback, &mref),
            None => routing.rules.push(RoutingRule {
                purpose: (*purpose).to_owned(),
                model: mref.clone(),
                fallback: Vec::new(),
            }),
        }
    }
    let routing = services.providers.set_routing_config(routing).await.map_err(to_rpc)?;
    let sec = &services.security;
    sec.audit
        .record(
            NewAudit::new(sec.user(), "claudeEngine.useForEverything", mref.model.clone(), AuditOutcome::Executed)
                .details(serde_json::json!({ "providerId": id, "purposes": PURPOSES })),
        )
        .await;
    Ok(UseClaudeForEverythingResult { status: status(services, false).await, manager, routing })
}

async fn project_root(services: &Services, project: ProjectId) -> Result<PathBuf, RpcError> {
    let p = services
        .projects
        .get(project)
        .await
        .map_err(crate::error::storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {project} not found")))?;
    let root = PathBuf::from(p.root_path.ok_or_else(|| RpcError::bad_request("the project has no root folder"))?);
    if !root.is_dir() {
        return Err(RpcError::bad_request(format!("the project folder {} does not exist", root.display())));
    }
    Ok(root)
}

fn user_agents_dir() -> Option<PathBuf> {
    orkestr_platform::paths::claude_user_dir().map(|d| d.join("agents"))
}

async fn scope_dir(services: &Services, scope: ClaudeAgentScope, project: Option<ProjectId>) -> Result<PathBuf, RpcError> {
    match scope {
        ClaudeAgentScope::User => {
            user_agents_dir().ok_or_else(|| RpcError::bad_request("the user's home folder is unknown"))
        }
        ClaudeAgentScope::Project => {
            let p = project.ok_or_else(|| RpcError::bad_request("projectId is required for a project agent"))?;
            Ok(cagents::agents_dir(&project_root(services, p).await?))
        }
        ClaudeAgentScope::BuiltIn => Err(RpcError::bad_request("built-in agents cannot be edited")),
    }
}

/// Maps Claude Code tool names onto Orkestr template tools (the bridge
/// enforces them: no write tool → edits are denied; no `run_command` →
/// shell is denied). Empty = every tool.
#[must_use]
pub fn orkestr_tools(claude_tools: &[String]) -> Vec<String> {
    let all = claude_tools.is_empty() || claude_tools.iter().any(|t| t == "*");
    let has = |names: &[&str]| all || claude_tools.iter().any(|t| names.iter().any(|n| t.eq_ignore_ascii_case(n) || t.starts_with(&format!("{n}("))));
    let mut out: Vec<&str> = vec!["read_file", "list_dir", "search_files", "git_status", "git_diff"];
    if has(&["Edit", "Write", "MultiEdit", "NotebookEdit"]) {
        out.extend(["write_file", "edit_file", "create_snapshot"]);
    }
    if has(&["Bash", "PowerShell"]) {
        out.extend(["run_command", "git_commit"]);
    }
    out.push("submit_result");
    out.into_iter().map(str::to_owned).collect()
}

fn model_alias(m: Option<&str>) -> String {
    match m.map(str::trim) {
        Some(m) if !m.is_empty() && m != "inherit" && valid_model(m) => m.to_owned(),
        _ => DEFAULT_MODEL.to_owned(),
    }
}

/// The template draft mirroring a Claude Code agent.
#[must_use]
pub fn draft_for(def: &ClaudeAgentDef, provider: Option<ProviderId>) -> TemplateDraft {
    let description: String = def.description.chars().take(480).collect();
    let system_prompt = if def.prompt.trim().is_empty() {
        format!("{CLAUDE_BUILTIN_MARKER}\n{}", if description.is_empty() { def.name.clone() } else { description.clone() })
    } else {
        def.prompt.chars().take(32_000).collect()
    };
    TemplateDraft {
        name: def.name.clone(),
        role: AgentRole::Custom(CLAUDE_ROLE.into()),
        description,
        system_prompt,
        preferred_model: provider.map(|provider_id| ModelRef { provider_id, model: model_alias(def.model.as_deref()) }),
        fallback_models: Vec::new(),
        tools: orkestr_tools(&def.tools),
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

/// `claudeEngine.agents`: project + user files and the CLI's built-ins,
/// each mirrored as a template.
pub async fn agents(services: &Services, project: Option<ProjectId>, refresh: bool) -> Result<ClaudeAgentsResult, RpcError> {
    let root = match project {
        Some(p) => Some(project_root(services, p).await?),
        None => None,
    };
    let mut list: Vec<ClaudeAgentDef> = Vec::new();
    if let Some(r) = &root {
        list.extend(cagents::read_dir(&cagents::agents_dir(r), ClaudeAgentScope::Project));
    }
    if let Some(d) = user_agents_dir() {
        list.extend(cagents::read_dir(&d, ClaudeAgentScope::User));
    }
    let mut builtin_error = None;
    let cached = builtin_cache().lock().clone();
    let builtins = match cached.filter(|_| !refresh) {
        Some(b) => b,
        None => {
            let exts = services.extensions.enabled_dirs().await.unwrap_or_default();
            match services.claude.binary(exts).await {
                Some(bin) => {
                    let cwd = root.clone().unwrap_or_else(std::env::temp_dir);
                    match cagents::probe_initialize(&bin, &cwd).await {
                        Ok(resp) => {
                            let b = cagents::builtin_from_initialize(&resp);
                            *builtin_cache().lock() = Some(b.clone());
                            b
                        }
                        Err(e) => {
                            builtin_error = Some(e);
                            Vec::new()
                        }
                    }
                }
                None => {
                    builtin_error = Some("Claude Code is not installed".into());
                    Vec::new()
                }
            }
        }
    };
    // File agents shadow built-ins of the same name (project wins over user).
    for b in builtins {
        if !list.iter().any(|a| a.name == b.name) {
            list.push(b);
        }
    }
    let mut seen = std::collections::HashSet::new();
    list.retain(|a| seen.insert(a.name.clone()));
    let provider = provider_id(services);
    for a in &mut list {
        if !orkestr_providers::adapters::claude_code::is_valid_agent_name(&a.name) {
            continue;
        }
        match services.agents.templates().upsert_mirrored(draft_for(a, provider), TemplateSource::ClaudeCode).await {
            Ok(t) => a.template_id = Some(t.id),
            Err(e) => tracing::warn!(agent = %a.name, error = %e, "could not mirror a Claude Code agent as a template"),
        }
    }
    Ok(ClaudeAgentsResult { agents: list, builtin_error })
}

async fn audit_file(services: &Services, action: &str, path: &Path, project: Option<ProjectId>) {
    let sec = &services.security;
    sec.audit
        .record(
            NewAudit::new(sec.user(), action, path.display().to_string(), AuditOutcome::Executed)
                .capability(Capability::Write)
                .project(project),
        )
        .await;
}

/// `claudeEngine.saveAgent`.
pub async fn save_agent(services: &Services, p: SaveClaudeAgentParams) -> Result<ClaudeAgentFileResult, RpcError> {
    if !p.confirmed {
        return Err(RpcError::new(RpcErrorCode::ApprovalRequired, "confirm writing the agent file first"));
    }
    let name = p.name.trim().to_owned();
    if !cagents::is_valid_file_name(&name) {
        return Err(RpcError::bad_request("the name may only contain lowercase letters, digits, - and _ (max 64)"));
    }
    if p.description.trim().is_empty() || p.description.chars().count() > 2_000 {
        return Err(RpcError::bad_request("the description must be 1..=2000 characters"));
    }
    if p.prompt.trim().is_empty() || p.prompt.chars().count() > 32_000 {
        return Err(RpcError::bad_request("the system prompt must be 1..=32000 characters"));
    }
    let tools: Vec<String> = p.tools.iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect();
    if tools.iter().any(|t| t.contains(['\n', '\r', ',']) || t.len() > 120) {
        return Err(RpcError::bad_request("invalid tool name"));
    }
    let model = p.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
    if model.is_some_and(|m| m != "inherit" && !valid_model(m)) {
        return Err(RpcError::bad_request("invalid model name"));
    }
    let dir = scope_dir(services, p.scope, p.project_id).await?;
    let path = dir.join(format!("{name}.md"));
    let text = cagents::render_agent_file(&name, &p.description, &tools, model, &p.prompt);
    std::fs::create_dir_all(&dir).map_err(|e| RpcError::internal(format!("cannot create {}: {e}", dir.display())))?;
    let tmp = dir.join(format!(".{name}.md.tmp"));
    std::fs::write(&tmp, &text).map_err(|e| RpcError::internal(format!("cannot write {}: {e}", path.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| RpcError::internal(format!("cannot write {}: {e}", path.display())))?;
    audit_file(services, "claudeEngine.saveAgent", &path, p.project_id).await;
    let mut agent = cagents::parse_agent_file(&text, p.scope, Some(&path));
    if let Some(a) = agent.as_mut() {
        let draft = draft_for(a, provider_id(services));
        if let Ok(t) = services.agents.templates().upsert_mirrored(draft, TemplateSource::ClaudeCode).await {
            a.template_id = Some(t.id);
        }
    }
    Ok(ClaudeAgentFileResult { path: path.display().to_string(), agent })
}

/// `claudeEngine.deleteAgent`.
pub async fn delete_agent(services: &Services, p: DeleteClaudeAgentParams) -> Result<ClaudeAgentFileResult, RpcError> {
    if !p.confirmed {
        return Err(RpcError::new(RpcErrorCode::ApprovalRequired, "confirm deleting the agent file first"));
    }
    let name = p.name.trim().to_owned();
    if !cagents::is_valid_file_name(&name) {
        return Err(RpcError::bad_request("invalid agent name"));
    }
    let dir = scope_dir(services, p.scope, p.project_id).await?;
    let path = dir.join(format!("{name}.md"));
    if !path.is_file() {
        return Err(RpcError::not_found(format!("{} not found", path.display())));
    }
    std::fs::remove_file(&path).map_err(|e| RpcError::internal(format!("cannot delete {}: {e}", path.display())))?;
    audit_file(services, "claudeEngine.deleteAgent", &path, p.project_id).await;
    let templates = services.agents.templates();
    if let Ok(list) = templates.list(None, Some(TemplateSource::ClaudeCode)).await {
        for t in list.into_iter().filter(|t| t.name == name) {
            let _ = templates.remove_mirrored(t.id).await;
        }
    }
    Ok(ClaudeAgentFileResult { path: path.display().to_string(), agent: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_keeps_the_previous_model_as_fallback() {
        let other = ProviderId::new();
        let claude = ProviderId::new();
        let mut cur = ModelRef { provider_id: other, model: "gpt".into() };
        let mut fb = vec![ModelRef { provider_id: claude, model: "sonnet".into() }];
        let target = ModelRef { provider_id: claude, model: "sonnet".into() };
        promote(&mut cur, &mut fb, &target);
        assert_eq!(cur, target);
        assert_eq!(fb, vec![ModelRef { provider_id: other, model: "gpt".into() }]);
        promote(&mut cur, &mut fb, &target);
        assert_eq!(fb.len(), 1, "idempotent");
    }

    #[test]
    fn tools_map_onto_orkestr_tools() {
        let ro = orkestr_tools(&["Read".into(), "Grep".into(), "Glob".into()]);
        assert!(ro.contains(&"read_file".to_owned()) && !ro.contains(&"write_file".to_owned()));
        assert!(!ro.contains(&"run_command".to_owned()));
        let all = orkestr_tools(&[]);
        assert!(all.contains(&"write_file".to_owned()) && all.contains(&"run_command".to_owned()));
        let bash = orkestr_tools(&["Bash(npm test)".into()]);
        assert!(bash.contains(&"run_command".to_owned()));
    }

    #[test]
    fn drafts_validate() {
        let def = ClaudeAgentDef {
            name: "Explore".into(),
            description: "Fast codebase exploration".into(),
            tools: vec![],
            model: Some("haiku".into()),
            prompt: String::new(),
            scope: ClaudeAgentScope::BuiltIn,
            path: None,
            template_id: None,
        };
        let d = orkestr_agent_runtime::templates::validate_draft(draft_for(&def, Some(ProviderId::new()))).unwrap();
        assert!(d.system_prompt.starts_with(CLAUDE_BUILTIN_MARKER));
        assert_eq!(d.preferred_model.unwrap().model, "haiku");
        assert_eq!(d.role, AgentRole::Custom(CLAUDE_ROLE.into()));
    }
}

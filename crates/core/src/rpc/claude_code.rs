//! `claudeCode.*` handlers: the Claude Code panel (official `claude` CLI in
//! stream-json mode; see `orkestr_providers::claude_code_session`).
//!
//! * Sessions run in the project root, a folder inside it, or one of the
//!   project's git worktrees — nowhere else.
//! * A session gets a child of the project's emergency token, so Stop All AI
//!   / stop project kill its process tree; starting fails while Stop All is
//!   active.
//! * `bypassPermissions` needs `confirmBypass: true` on the call (the UI asks
//!   with a danger dialog every time); every start / mode change / permission
//!   decision is audited.

use std::path::{Path, PathBuf};

use orkestr_domain::{AuditOutcome, Capability, ProjectId};
use orkestr_protocol::methods::claude_code::{
    self as m, ClaudeLoginParams, ClaudeLoginResult, ClaudeMcpStatusResult, ClaudeOpenInTerminalParams,
    ClaudePermissionMode, ClaudeRespondPermissionParams, ClaudeSendParams, ClaudeSessionParams, ClaudeSessionsParams,
    ClaudeSessionsResult, ClaudeSetBinaryPathParams, ClaudeSetChatMetaParams, ClaudeSetModeParams,
    ClaudeSetModelParams, ClaudeSetThinkingParams, ClaudeStartParams, ClaudeStartResult, ClaudeStatusParams,
    ClaudeTranscriptParams, ClaudeTranscriptResult, ClaudeUsageParams,
};
use orkestr_protocol::{RpcError, RpcErrorCode};
use orkestr_providers::claude_code_session::{history, usage, LaunchSpec, Query, SessionError};
use orkestr_security::NewAudit;
use orkestr_terminal::{OpenRequest, ShellSpec, TerminalOwner};

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Status, _, _>(|ctx: RpcContext, p: ClaudeStatusParams| async move {
            let exts = native_extensions(&ctx).await;
            Ok::<_, RpcError>(ctx.services.claude.status(exts, p.refresh.unwrap_or(false)).await)
        })
        .method::<m::Sessions, _, _>(sessions)
        .method::<m::Start, _, _>(start)
        .method::<m::Send, _, _>(|ctx: RpcContext, p: ClaudeSendParams| async move {
            let images: Vec<(String, String)> =
                p.images.unwrap_or_default().into_iter().map(|i| (i.media_type, i.data)).collect();
            ctx.services.claude.manager.send_with_images(p.session_id, &p.text, &images).await.map_err(to_rpc)
        })
        .method::<m::Capabilities, _, _>(|ctx: RpcContext, p: ClaudeSessionParams| async move {
            let event = ctx.services.claude.manager.capabilities(p.session_id).map_err(to_rpc)?;
            Ok::<_, RpcError>(m::ClaudeCapabilitiesResult { event })
        })
        .method::<m::Usage, _, _>(|ctx: RpcContext, p: ClaudeUsageParams| async move {
            let exts = native_extensions(&ctx).await;
            let program = ctx.services.claude.binary(exts).await;
            Ok::<_, RpcError>(ctx.services.claude.usage(p.session_id, program, p.refresh.unwrap_or(false)).await)
        })
        .method::<m::ContextUsage, _, _>(|ctx: RpcContext, p: ClaudeSessionParams| async move {
            let v = ctx.services.claude.manager.query(p.session_id, Query::Context).await.map_err(to_rpc)?;
            Ok::<_, RpcError>(usage::context_usage(&v))
        })
        .method::<m::McpStatus, _, _>(|ctx: RpcContext, p: ClaudeSessionParams| async move {
            let v = ctx.services.claude.manager.query(p.session_id, Query::Mcp).await.map_err(to_rpc)?;
            Ok::<_, RpcError>(ClaudeMcpStatusResult { servers: usage::mcp_servers(&v) })
        })
        .method::<m::SetThinking, _, _>(|ctx: RpcContext, p: ClaudeSetThinkingParams| async move {
            ctx.services.claude.manager.set_thinking(p.session_id, p.enabled).await.map_err(to_rpc)
        })
        .method::<m::SetChatMeta, _, _>(|ctx: RpcContext, p: ClaudeSetChatMetaParams| async move {
            ctx.services.claude.set_chat_meta(&p).map_err(RpcError::bad_request)
        })
        .method::<m::OpenInTerminal, _, _>(open_in_terminal)
        .method::<m::Interrupt, _, _>(|ctx: RpcContext, p: ClaudeSessionParams| async move {
            ctx.services.claude.manager.interrupt(p.session_id).await.map_err(to_rpc)
        })
        .method::<m::SetMode, _, _>(|ctx: RpcContext, p: ClaudeSetModeParams| async move {
            let confirmed = p.confirm_bypass.unwrap_or(false);
            let info = ctx.services.claude.manager.set_mode(p.session_id, p.mode, confirmed).await.map_err(to_rpc)?;
            audit(
                &ctx,
                "claudeCode.setMode",
                p.mode.as_cli().to_owned(),
                Some(info.project_id),
                serde_json::json!({ "sessionId": p.session_id, "confirmedBypass": confirmed }),
            )
            .await;
            Ok(info)
        })
        .method::<m::SetModel, _, _>(|ctx: RpcContext, p: ClaudeSetModelParams| async move {
            ctx.services.claude.manager.set_model(p.session_id, &p.model).await.map_err(to_rpc)
        })
        .method::<m::RespondPermission, _, _>(|ctx: RpcContext, p: ClaudeRespondPermissionParams| async move {
            ctx.services
                .claude
                .manager
                .respond(p.session_id, &p.request_id, p.decision, p.message.as_deref())
                .await
                .map_err(to_rpc)
        })
        .method::<m::Stop, _, _>(|ctx: RpcContext, p: ClaudeSessionParams| async move {
            ctx.services.claude.manager.stop(p.session_id).await.map_err(to_rpc)
        })
        .method::<m::LoginInTerminal, _, _>(login_in_terminal)
        .method::<m::SetBinaryPath, _, _>(|ctx: RpcContext, p: ClaudeSetBinaryPathParams| async move {
            let path = p.path.clone();
            ctx.services.claude.set_override(p.path).map_err(RpcError::bad_request)?;
            audit(
                &ctx,
                "claudeCode.setBinaryPath",
                path.clone().unwrap_or_else(|| "(automatic)".into()),
                None,
                serde_json::Value::Null,
            )
            .await;
            let exts = native_extensions(&ctx).await;
            Ok(ctx.services.claude.status(exts, true).await)
        })
        .method::<m::Transcript, _, _>(|ctx: RpcContext, p: ClaudeTranscriptParams| async move {
            let cwd = resolve_cwd(&ctx, p.project_id, p.cwd.as_deref()).await?;
            let (events, truncated) = load_transcript(&ctx, &cwd, &p.claude_session_id).await?;
            Ok(ClaudeTranscriptResult { events, truncated })
        });
}

/// Maps session errors.
pub fn to_rpc(e: SessionError) -> RpcError {
    match e {
        SessionError::NotFound(_) => RpcError::not_found(e.to_string()),
        SessionError::BadRequest(_) => RpcError::bad_request(e.to_string()),
        SessionError::Conflict(_) => RpcError::conflict(e.to_string()),
        SessionError::Spawn(_) | SessionError::Protocol(_) | SessionError::Timeout(_) => {
            RpcError::new(RpcErrorCode::ProviderError, e.to_string())
        }
    }
}

async fn audit(
    ctx: &RpcContext,
    action: &str,
    resource: String,
    project: Option<ProjectId>,
    details: serde_json::Value,
) {
    let sec = &ctx.services.security;
    let entry = NewAudit::new(sec.user(), action, resource, AuditOutcome::Executed)
        .capability(Capability::Terminal)
        .project(project)
        .details(details);
    sec.audit.record(entry).await;
}

async fn native_extensions(ctx: &RpcContext) -> Vec<(String, PathBuf)> {
    ctx.services.extensions.enabled_dirs().await.unwrap_or_default()
}

async fn project_root(ctx: &RpcContext, project_id: ProjectId) -> Result<PathBuf, RpcError> {
    let project = ctx
        .services
        .projects
        .get(project_id)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {project_id} not found")))?;
    let root = PathBuf::from(project.root_path.ok_or_else(|| RpcError::bad_request("the project has no root folder"))?);
    if !root.is_dir() {
        return Err(RpcError::bad_request(format!("the project folder {} does not exist", root.display())));
    }
    Ok(root)
}

fn canonical(p: &Path) -> PathBuf {
    dunce_like(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
}

/// Strips the Windows verbatim prefix (`\\?\C:\…` → `C:\…`) so paths compare
/// and display like the user typed them.
fn dunce_like(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

/// The session folder: the project root, a folder inside it or one of its worktrees.
async fn resolve_cwd(ctx: &RpcContext, project_id: ProjectId, cwd: Option<&str>) -> Result<PathBuf, RpcError> {
    let root = project_root(ctx, project_id).await?;
    let Some(raw) = cwd.map(str::trim).filter(|c| !c.is_empty()) else { return Ok(root) };
    let p = PathBuf::from(raw);
    if !p.is_absolute() || !p.is_dir() {
        return Err(RpcError::bad_request(format!("{raw} is not an existing absolute folder")));
    }
    let (cp, croot) = (canonical(&p), canonical(&root));
    if cp.starts_with(&croot) {
        return Ok(cp);
    }
    let worktrees = ctx.services.vcs.worktrees(project_id, &root).await.unwrap_or_default();
    if worktrees.iter().any(|w| canonical(Path::new(&w.path)) == cp) {
        return Ok(cp);
    }
    Err(RpcError::forbidden("Claude Code runs only in the project folder or one of its git worktrees"))
}

async fn load_transcript(
    ctx: &RpcContext,
    cwd: &Path,
    session: &str,
) -> Result<(Vec<orkestr_protocol::methods::claude_code::ClaudeCodeEvent>, bool), RpcError> {
    if !history::is_session_id(session) {
        return Err(RpcError::bad_request("invalid conversation id"));
    }
    let dir = ctx.services.claude.projects_dir().ok_or_else(|| RpcError::not_found("no Claude Code history found"))?;
    let (cwd, session) = (cwd.to_path_buf(), session.to_owned());
    tokio::task::spawn_blocking(move || {
        let folder =
            history::folder_for(&dir, &cwd).ok_or_else(|| "no Claude Code history for this folder".to_owned())?;
        history::transcript(&folder, &session)
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))?
    .map_err(RpcError::not_found)
}

async fn sessions(ctx: RpcContext, p: ClaudeSessionsParams) -> Result<ClaudeSessionsResult, RpcError> {
    let cwd = resolve_cwd(&ctx, p.project_id, p.cwd.as_deref()).await?;
    let live = ctx.services.claude.manager.list(Some(p.project_id));
    // Make sure the projects directory reported by the CLI is known.
    let exts = native_extensions(&ctx).await;
    let _ = ctx.services.claude.status(exts, false).await;
    let history = match ctx.services.claude.projects_dir() {
        Some(dir) => tokio::task::spawn_blocking(move || history::folder_for(&dir, &cwd).map(|f| history::list(&f)))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(ClaudeSessionsResult { live, history, meta: ctx.services.claude.chat_meta() })
}

async fn start(ctx: RpcContext, p: ClaudeStartParams) -> Result<ClaudeStartResult, RpcError> {
    let cwd = resolve_cwd(&ctx, p.project_id, p.cwd.as_deref()).await?;
    let mode = p.permission_mode.unwrap_or_default();
    let bypass = mode == ClaudePermissionMode::BypassPermissions;
    if bypass && p.confirm_bypass != Some(true) {
        return Err(RpcError::bad_request("bypassPermissions needs confirmBypass: true"));
    }
    let exts = native_extensions(&ctx).await;
    let program = ctx.services.claude.binary(exts).await.ok_or_else(|| {
        RpcError::not_found("Claude Code was not found: install the Claude Code extension or set the path to `claude`")
            .with_data(serde_json::json!({ "reason": "binaryNotFound" }))
    })?;
    let stop = ctx
        .services
        .security
        .emergency
        .register_project(p.project_id)
        .map_err(|e| RpcError::forbidden(e.to_string()))?;
    let history = match p.resume.as_deref() {
        Some(id) => load_transcript(&ctx, &cwd, id).await.map(|(events, _)| events).unwrap_or_default(),
        None => Vec::new(),
    };
    let add_dirs = extra_dirs(&ctx, p.add_dirs.as_deref().unwrap_or_default())?;
    let spec = LaunchSpec {
        project_id: p.project_id,
        cwd: cwd.clone(),
        program: program.clone(),
        model: p.model.clone().map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()),
        mode,
        allow_bypass: bypass,
        resume: p.resume.clone(),
        add_dirs: add_dirs.clone(),
        fallback_model: p.fallback_model.clone().map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()),
        stop: Some(stop),
    };
    let session = ctx.services.claude.manager.start(spec).map_err(to_rpc)?;
    audit(
        &ctx,
        "claudeCode.start",
        cwd.display().to_string(),
        Some(p.project_id),
        serde_json::json!({
            "sessionId": session.session_id,
            "binary": program.display().to_string(),
            "mode": mode.as_cli(),
            "model": p.model,
            "resume": p.resume,
            "addDirs": add_dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
            "fallbackModel": p.fallback_model,
        }),
    )
    .await;
    Ok(ClaudeStartResult { session, history })
}

/// Most extra folders per session.
const MAX_ADD_DIRS: usize = 8;

/// Extra folders (`--add-dir`) the user picked: existing absolute folders,
/// never a drive / filesystem root or Orkestr's own data directory.
fn extra_dirs(ctx: &RpcContext, raw: &[String]) -> Result<Vec<PathBuf>, RpcError> {
    if raw.len() > MAX_ADD_DIRS {
        return Err(RpcError::bad_request(format!("at most {MAX_ADD_DIRS} extra folders")));
    }
    let data = canonical(ctx.services.claude.data_dir());
    let mut out: Vec<PathBuf> = Vec::new();
    for r in raw.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
        let p = PathBuf::from(r);
        if !p.is_absolute() || !p.is_dir() {
            return Err(RpcError::bad_request(format!("{r} is not an existing absolute folder")));
        }
        let c = canonical(&p);
        if c.parent().is_none() {
            return Err(RpcError::forbidden("a drive or filesystem root cannot be added"));
        }
        if c.starts_with(&data) || data.starts_with(&c) {
            return Err(RpcError::forbidden("Orkestr's data folder cannot be added"));
        }
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Ok(out)
}

async fn open_in_terminal(ctx: RpcContext, p: ClaudeOpenInTerminalParams) -> Result<ClaudeLoginResult, RpcError> {
    let cwd = resolve_cwd(&ctx, p.project_id, p.cwd.as_deref()).await?;
    let exts = native_extensions(&ctx).await;
    let program = ctx.services.claude.binary(exts).await.ok_or_else(|| {
        RpcError::not_found("Claude Code was not found").with_data(serde_json::json!({ "reason": "binaryNotFound" }))
    })?;
    let mut args = Vec::new();
    if let Some(id) = p.claude_session_id.as_deref() {
        if !history::is_session_id(id) {
            return Err(RpcError::bad_request("invalid conversation id"));
        }
        args.push("--resume".to_owned());
        args.push(id.to_owned());
    }
    let req = OpenRequest {
        project_id: Some(p.project_id),
        root: Some(cwd.clone()),
        cwd: None,
        owner: TerminalOwner::User,
        shell: ShellSpec::Custom { program, args: args.clone() },
        env: Vec::new(),
        cols: 120,
        rows: 30,
        title: Some("Claude Code".into()),
    };
    let info = ctx.services.terminals.open(req).await.map_err(super::terminal::to_rpc)?;
    audit(
        &ctx,
        "claudeCode.openInTerminal",
        cwd.display().to_string(),
        Some(p.project_id),
        serde_json::json!({ "resume": p.claude_session_id }),
    )
    .await;
    Ok(ClaudeLoginResult { terminal: super::terminal::session_dto(info) })
}

async fn login_in_terminal(ctx: RpcContext, p: ClaudeLoginParams) -> Result<ClaudeLoginResult, RpcError> {
    let exts = native_extensions(&ctx).await;
    let program = ctx.services.claude.binary(exts).await.ok_or_else(|| {
        RpcError::not_found("Claude Code was not found").with_data(serde_json::json!({ "reason": "binaryNotFound" }))
    })?;
    let root = match p.project_id {
        Some(id) => Some(project_root(&ctx, id).await?),
        None => None,
    };
    let req = OpenRequest {
        project_id: p.project_id,
        root,
        cwd: None,
        owner: TerminalOwner::User,
        shell: ShellSpec::Custom { program, args: vec!["auth".into(), "login".into()] },
        env: Vec::new(),
        cols: 120,
        rows: 30,
        title: Some("Claude Code: /login".into()),
    };
    let info = ctx.services.terminals.open(req).await.map_err(super::terminal::to_rpc)?;
    audit(&ctx, "claudeCode.loginInTerminal", "claude auth login".into(), p.project_id, serde_json::Value::Null).await;
    Ok(ClaudeLoginResult { terminal: super::terminal::session_dto(info) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(dunce_like(PathBuf::from(r"\\?\C:\work")), PathBuf::from(r"C:\work"));
        assert_eq!(dunce_like(PathBuf::from(r"\\?\UNC\srv\x")), PathBuf::from(r"\\?\UNC\srv\x"));
        assert_eq!(dunce_like(PathBuf::from("/home/x")), PathBuf::from("/home/x"));
    }
}

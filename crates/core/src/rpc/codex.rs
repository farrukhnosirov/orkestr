//! `codex.*` and `engines.*` handlers (ADR-0047).

use std::path::PathBuf;

use orkestr_domain::{AuditOutcome, Capability, ProjectId};
use orkestr_protocol::methods::codex::{
    self as m, CodexInstallParams, CodexInstallResult, CodexLimitsParams, CodexLoginMode, CodexLoginParams,
    CodexModelsParams, CodexOpenInTerminalParams, CodexRespondApprovalParams, CodexSendParams, CodexSessionParams,
    CodexSetOptionsParams, CodexStartParams, CodexStatusParams, CodexTemplatesParams, CodexTemplatesResult,
    CodexTerminalResult, CodexThreadsParams, CodexThreadsResult, CodexTranscriptParams, CodexTranscriptResult,
    EnginesConfig, EnginesStatusParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{RpcError, RpcErrorCode};
use orkestr_providers::ProviderError;
use orkestr_security::NewAudit;
use orkestr_terminal::{OpenRequest, ShellSpec, TerminalOwner};

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Status, _, _>(|ctx: RpcContext, p: CodexStatusParams| async move {
            Ok::<_, RpcError>(crate::codex::status(&ctx.services, p.refresh.unwrap_or(false)).await)
        })
        .method::<m::Install, _, _>(install)
        .method::<m::Login, _, _>(login)
        .method::<m::LoginCancel, _, _>(|ctx: RpcContext, _p: Empty| async move {
            ctx.services.codex.hub.login_cancel().await.map_err(to_rpc)?;
            Ok::<_, RpcError>(crate::codex::status(&ctx.services, false).await)
        })
        .method::<m::Logout, _, _>(|ctx: RpcContext, _p: Empty| async move {
            ctx.services.codex.hub.logout().await.map_err(to_rpc)?;
            audit(&ctx, "codex.logout", "codex".into(), None).await;
            Ok::<_, RpcError>(crate::codex::status(&ctx.services, true).await)
        })
        .method::<m::OpenInTerminal, _, _>(open_in_terminal)
        .method::<m::Limits, _, _>(|ctx: RpcContext, p: CodexLimitsParams| async move {
            ready(&ctx).await?;
            Ok::<_, RpcError>(ctx.services.codex.limits(p.refresh.unwrap_or(false)).await)
        })
        .method::<m::Models, _, _>(|ctx: RpcContext, p: CodexModelsParams| async move {
            let _ = ready(&ctx).await;
            Ok::<_, RpcError>(ctx.services.codex.models(p.refresh.unwrap_or(false)).await)
        })
        .method::<m::Threads, _, _>(|ctx: RpcContext, p: CodexThreadsParams| async move {
            ready(&ctx).await?;
            let root = match p.project_id {
                Some(id) => Some(project_root(&ctx, id).await?),
                None => None,
            };
            let limit = p.limit.unwrap_or(50).clamp(1, 200);
            let live = ctx.services.codex.hub.sessions(p.project_id);
            Ok::<_, RpcError>(match ctx.services.codex.hub.threads(root.as_deref(), limit).await {
                Ok(threads) => CodexThreadsResult { threads, live, error: None },
                Err(e) => CodexThreadsResult { threads: Vec::new(), live, error: Some(e.to_string()) },
            })
        })
        .method::<m::Transcript, _, _>(|ctx: RpcContext, p: CodexTranscriptParams| async move {
            ready(&ctx).await?;
            let events = ctx.services.codex.hub.transcript(&p.thread_id).await.map_err(to_rpc)?;
            Ok::<_, RpcError>(CodexTranscriptResult { events })
        })
        .method::<m::Start, _, _>(start)
        .method::<m::Send, _, _>(|ctx: RpcContext, p: CodexSendParams| async move {
            if p.text.len() > 200_000 {
                return Err(RpcError::bad_request("the message is too long"));
            }
            ctx.services.codex.hub.send(p.session_id, &p.text).await.map_err(to_rpc)
        })
        .method::<m::Interrupt, _, _>(|ctx: RpcContext, p: CodexSessionParams| async move {
            ctx.services.codex.hub.interrupt(p.session_id).await.map_err(to_rpc)
        })
        .method::<m::Stop, _, _>(|ctx: RpcContext, p: CodexSessionParams| async move {
            ctx.services.codex.hub.stop(p.session_id).await.map_err(to_rpc)
        })
        .method::<m::Compact, _, _>(|ctx: RpcContext, p: CodexSessionParams| async move {
            ctx.services.codex.hub.compact(p.session_id).await.map_err(to_rpc)
        })
        .method::<m::SetOptions, _, _>(|ctx: RpcContext, p: CodexSetOptionsParams| async move {
            ctx.services.codex.hub.set_options(p.session_id, p.model, p.effort, p.mode).map_err(to_rpc)
        })
        .method::<m::RespondApproval, _, _>(|ctx: RpcContext, p: CodexRespondApprovalParams| async move {
            ctx.services.codex.hub.respond(p.session_id, &p.request_id, p.decision).await.map_err(to_rpc)
        })
        .method::<m::Templates, _, _>(|ctx: RpcContext, p: CodexTemplatesParams| async move {
            let root = match p.project_id {
                Some(id) => Some(project_root(&ctx, id).await?),
                None => None,
            };
            Ok::<_, RpcError>(CodexTemplatesResult { templates: crate::codex::templates(&ctx.services, root).await })
        })
        .method::<m::EnginesStatusMethod, _, _>(|ctx: RpcContext, p: EnginesStatusParams| async move {
            Ok::<_, RpcError>(crate::engines::status(&ctx.services, p.refresh.unwrap_or(false)).await)
        })
        .method::<m::EnginesSetConfig, _, _>(|ctx: RpcContext, p: EnginesConfig| async move {
            crate::engines::set_config(&ctx.services, p).await
        });
}

/// Maps provider errors.
pub fn to_rpc(e: ProviderError) -> RpcError {
    match e {
        ProviderError::BadRequest(m) => RpcError::bad_request(m),
        ProviderError::Auth(m) => {
            RpcError::new(RpcErrorCode::ProviderError, m).with_data(serde_json::json!({ "reason": "authRequired" }))
        }
        other => RpcError::new(RpcErrorCode::ProviderError, other.to_string()),
    }
}

/// Makes sure the hub knows the binary.
async fn ready(ctx: &RpcContext) -> Result<(), RpcError> {
    if ctx.services.codex.hub.program().is_none() {
        let st = crate::codex::status(&ctx.services, false).await;
        if st.binary.is_none() {
            return Err(RpcError::not_found("Codex CLI is not installed")
                .with_data(serde_json::json!({ "reason": "binaryNotFound" })));
        }
    }
    Ok(())
}

async fn audit(ctx: &RpcContext, action: &str, resource: String, project: Option<ProjectId>) {
    let sec = &ctx.services.security;
    sec.audit
        .record(
            NewAudit::new(sec.user(), action, resource, AuditOutcome::Executed)
                .capability(Capability::Terminal)
                .project(project),
        )
        .await;
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

async fn install(ctx: RpcContext, p: CodexInstallParams) -> Result<CodexInstallResult, RpcError> {
    let update = p.update.unwrap_or(false);
    let r = ctx.services.codex.install().await;
    let (ok, log) = match r {
        Ok((path, log)) => {
            audit(&ctx, if update { "codex.update" } else { "codex.install" }, path.display().to_string(), None).await;
            (true, log)
        }
        Err(e) => (false, e),
    };
    let status = crate::codex::status(&ctx.services, true).await;
    Ok(CodexInstallResult { ok, log, status })
}

async fn login(ctx: RpcContext, p: CodexLoginParams) -> Result<m::CodexLoginState, RpcError> {
    ready(&ctx).await?;
    let state = ctx.services.codex.hub.login(p.mode).await.map_err(to_rpc)?;
    if p.mode == CodexLoginMode::Browser && ctx.services.codex.open_browser {
        if let Some(url) = state.auth_url.as_deref() {
            if let Err(e) = orkestr_platform::open_url(url) {
                tracing::info!(error = %e, "could not open the ChatGPT sign-in page; the panel shows the link");
            }
        }
    }
    audit(&ctx, "codex.login", format!("{:?}", p.mode), None).await;
    Ok(state)
}

async fn start(ctx: RpcContext, p: CodexStartParams) -> Result<m::CodexSessionInfo, RpcError> {
    ready(&ctx).await?;
    let root = project_root(&ctx, p.project_id).await?;
    ctx.services
        .codex
        .hub
        .start(
            p.project_id,
            &root,
            p.thread_id.as_deref(),
            p.fork.unwrap_or(false),
            p.model,
            p.effort,
            p.mode.unwrap_or_default(),
        )
        .await
        .map_err(to_rpc)
}

async fn open_in_terminal(ctx: RpcContext, p: CodexOpenInTerminalParams) -> Result<CodexTerminalResult, RpcError> {
    ready(&ctx).await?;
    let program = ctx.services.codex.hub.program().ok_or_else(|| RpcError::not_found("Codex CLI is not installed"))?;
    let root = match p.project_id {
        Some(id) => Some(project_root(&ctx, id).await?),
        None => None,
    };
    let mut args: Vec<String> = Vec::new();
    let title = if p.login.unwrap_or(false) {
        args.extend(["login".to_owned(), "--device-auth".to_owned()]);
        "Codex: sign in"
    } else {
        if let Some(t) = p.thread_id.as_deref() {
            if !orkestr_providers::codex_app::is_token(t) {
                return Err(RpcError::bad_request("invalid thread id"));
            }
            args.extend(["resume".to_owned(), t.to_owned()]);
        }
        "Codex"
    };
    let req = OpenRequest {
        project_id: p.project_id,
        root,
        cwd: None,
        owner: TerminalOwner::User,
        shell: ShellSpec::Custom { program, args: args.clone() },
        env: Vec::new(),
        cols: 120,
        rows: 30,
        title: Some(title.into()),
    };
    let info = ctx.services.terminals.open(req).await.map_err(super::terminal::to_rpc)?;
    audit(&ctx, "codex.openInTerminal", format!("codex {}", args.join(" ")), p.project_id).await;
    Ok(CodexTerminalResult { terminal: super::terminal::session_dto(info) })
}

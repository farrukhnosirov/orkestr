//! `lsp.*` handlers (ADR-0025): language servers for the Code panel.
//!
//! Starting a server runs a third-party program. The first start of an
//! exact command (recipe + program + args + binary hash) creates an
//! approval (capability `Terminal`) and answers `approval_required` with
//! its id; once the user approved it, `lsp.start` with that `approvalId`
//! records the trust and starts the server. Later starts of the same
//! command need no approval; `lsp.revoke` forgets it.

use std::path::{Path, PathBuf};

use orkestr_domain::{ApprovalStatus, Capability, ProjectId, RiskLevel};
use orkestr_events::EventBus;
use orkestr_extensions::lsp::recipes::{self, Resolved};
use orkestr_extensions::lsp::{self as elsp, LaunchSpec, LspEvent, LspManager};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::lsp::{
    self as m, LspMessagePayload, LspRevokeParams, LspSendParams, LspServerInfo, LspServersParams, LspServersResult,
    LspSessionParams, LspStartParams, LspStartResult, LspStatusPayload,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError, RpcErrorCode};
use orkestr_security::ApprovalRequest;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::extensions::to_rpc;
use super::{RpcContext, RpcRouter};
use crate::error::{security_to_rpc, storage_to_rpc};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Servers, _, _>(servers)
        .method::<m::Start, _, _>(start)
        .method::<m::Send, _, _>(|ctx: RpcContext, p: LspSendParams| async move {
            ctx.services.lsp.send(p.session_id, p.message).map_err(to_rpc)?;
            Ok(Empty {})
        })
        .method::<m::Stop, _, _>(|ctx: RpcContext, p: LspSessionParams| async move {
            ctx.services.lsp.stop(p.session_id);
            Ok(Empty {})
        })
        .method::<m::Revoke, _, _>(|ctx: RpcContext, p: LspRevokeParams| async move {
            ctx.services.extensions.store().revoke(&p.server_id).await.map_err(to_rpc)?;
            Ok(Empty {})
        });
}

fn which(name: &str) -> Option<PathBuf> {
    orkestr_platform::process::which(name)
}

async fn resolve_all(ctx: &RpcContext) -> Result<Vec<(&'static recipes::Recipe, Option<Resolved>)>, RpcError> {
    let exts = ctx.services.extensions.enabled_dirs().await.map_err(to_rpc)?;
    tokio::task::spawn_blocking(move || {
        recipes::RECIPES.iter().map(|r| (r, recipes::resolve(r, &exts, &which))).collect::<Vec<_>>()
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))
}

async fn fingerprint_of(server_id: &str, r: &Resolved) -> Result<String, RpcError> {
    let (sid, program, args) = (server_id.to_owned(), r.program.clone(), r.args.clone());
    tokio::task::spawn_blocking(move || elsp::fingerprint(&sid, &program, &args))
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(|e| RpcError::bad_request(format!("cannot read the language server program: {e}")))
}

async fn servers(ctx: RpcContext, _p: LspServersParams) -> Result<LspServersResult, RpcError> {
    let mut out = Vec::new();
    for (recipe, resolved) in resolve_all(&ctx).await? {
        let trusted = match &resolved {
            Some(r) => match fingerprint_of(recipe.id, r).await {
                Ok(fp) => ctx.services.extensions.store().is_trusted(&fp).await.map_err(to_rpc)?,
                Err(_) => false,
            },
            None => false,
        };
        out.push(LspServerInfo {
            id: recipe.id.to_owned(),
            name: recipe.name.to_owned(),
            languages: recipe.languages.iter().map(|s| (*s).to_owned()).collect(),
            available: resolved.is_some(),
            command: resolved.as_ref().map(Resolved::display),
            source: resolved.map(|r| r.source),
            trusted,
            install_hint: recipe.install_hint.to_owned(),
        });
    }
    Ok(LspServersResult { servers: out, sessions: ctx.services.lsp.list() })
}

/// `file://` URI of a directory.
fn file_uri(path: &Path) -> String {
    let norm = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !norm.starts_with('/') {
        out.push('/');
    }
    for (i, seg) in norm.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        for b in seg.bytes() {
            let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b':' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=');
            if keep {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out.trim_end_matches('/').to_owned()
}

async fn project_root(ctx: &RpcContext, project_id: ProjectId) -> Result<PathBuf, RpcError> {
    let project = ctx
        .services
        .projects
        .get(project_id)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {project_id} not found")))?;
    let root = project.root_path.ok_or_else(|| RpcError::bad_request("the project has no root folder"))?;
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err(RpcError::bad_request(format!("the project folder {} does not exist", root.display())));
    }
    Ok(root)
}

/// A token the emergency controller cancels on Stop All / stop project.
fn emergency_token(ctx: &RpcContext, project: ProjectId) -> Option<CancellationToken> {
    match ctx.services.security.emergency.register_project(project) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::info!(error = %e, "language server starts without an emergency token");
            None
        }
    }
}

fn preview(r: &Resolved, fingerprint: &str) -> String {
    format!("{}\n# sha256 fingerprint: {fingerprint}", r.display())
}

async fn start(ctx: RpcContext, p: LspStartParams) -> Result<LspStartResult, RpcError> {
    let recipe = recipes::recipe(&p.server_id)
        .ok_or_else(|| RpcError::not_found(format!("unknown language server `{}`", p.server_id)))?;
    let root = project_root(&ctx, p.project_id).await?;
    let exts = ctx.services.extensions.enabled_dirs().await.map_err(to_rpc)?;
    let resolved = tokio::task::spawn_blocking(move || recipes::resolve(recipe, &exts, &which))
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .ok_or_else(|| {
            RpcError::not_found(format!("{} is not installed. Install: {}", recipe.name, recipe.install_hint))
                .with_data(serde_json::json!({ "reason": "serverNotFound", "installHint": recipe.install_hint }))
        })?;
    let fp = fingerprint_of(recipe.id, &resolved).await?;
    let store = ctx.services.extensions.store();
    if !store.is_trusted(&fp).await.map_err(to_rpc)? {
        let approvals = &ctx.services.security.approvals;
        let expected_preview = preview(&resolved, &fp);
        match p.approval_id {
            Some(id) => {
                let a = approvals
                    .get(id)
                    .await
                    .map_err(security_to_rpc)?
                    .ok_or_else(|| RpcError::not_found(format!("approval {id} not found")))?;
                if a.command_preview.as_deref() != Some(expected_preview.as_str()) {
                    return Err(RpcError::forbidden("the approval does not match this language server command"));
                }
                match a.status {
                    ApprovalStatus::Approved => {
                        store.trust(&fp, recipe.id, &resolved.display(), Some(id.to_string())).await.map_err(to_rpc)?;
                    }
                    ApprovalStatus::Pending => {
                        return Err(RpcError::new(RpcErrorCode::ApprovalRequired, "waiting for your approval")
                            .with_data(serde_json::json!({ "approvalId": id })));
                    }
                    other => {
                        return Err(RpcError::forbidden(format!("running {} was not approved ({other:?})", recipe.name)));
                    }
                }
            }
            None => {
                let mut req = ApprovalRequest::new(
                    Capability::Terminal,
                    format!("Run the language server {}", recipe.name),
                    "Language servers are third-party programs that run with your user rights and read the \
                     project folder. Approve once to let the Code panel start this exact program (a changed \
                     binary asks again).",
                    RiskLevel::Medium,
                );
                req.project_id = Some(p.project_id);
                req.command_preview = Some(expected_preview);
                let a = approvals.request(req).await.map_err(security_to_rpc)?;
                return Err(RpcError::new(
                    RpcErrorCode::ApprovalRequired,
                    format!("running {} needs your approval", recipe.name),
                )
                .with_data(serde_json::json!({ "approvalId": a.id, "command": resolved.display() })));
            }
        }
    }
    if resolved.from_extension {
        // Archive entries are unpacked without executable bits; mark this
        // approved program runnable (Unix) now.
        orkestr_platform::fs::make_executable(&resolved.program)
            .map_err(|e| RpcError::internal(format!("cannot make the server runnable: {e}")))?;
    }
    let session = ctx
        .services
        .lsp
        .start(LaunchSpec {
            server_id: recipe.id.to_owned(),
            project_id: p.project_id,
            root_uri: file_uri(&root),
            cwd: root,
            program: resolved.program,
            args: resolved.args,
            stop: emergency_token(&ctx, p.project_id),
        })
        .map_err(to_rpc)?;
    Ok(LspStartResult { session })
}

/// Publishes LSP events and reaps idle sessions. Emergency stops reach the
/// sessions through their emergency-controller tokens (see [`start`]);
/// project deletion stops them in `projects.delete`. (No permanent bus
/// subscriber here: it would change ephemeral event accounting.)
pub fn spawn_bridge(lsp: LspManager, mut rx: mpsc::Receiver<LspEvent>, bus: EventBus, shutdown: CancellationToken) {
    let publish_bus = bus.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let new = match ev {
                LspEvent::Message { session_id, project_id, message } => {
                    NewEvent::new(EventKind::LspMessage(LspMessagePayload { session_id, message })).project(project_id)
                }
                LspEvent::Status(session) => NewEvent::new(EventKind::LspStatus(LspStatusPayload { session })),
            };
            if let Err(e) = publish_bus.publish(new).await {
                tracing::debug!(error = %e, "failed to publish an LSP event");
            }
        }
    });
    let reaper = lsp;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = tick.tick() => { reaper.reap_idle(); }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris() {
        #[cfg(windows)]
        {
            assert_eq!(file_uri(Path::new(r"C:\Users\me\my proj")), "file:///C:/Users/me/my%20proj");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(file_uri(Path::new("/home/me/my proj")), "file:///home/me/my%20proj");
        }
    }
}

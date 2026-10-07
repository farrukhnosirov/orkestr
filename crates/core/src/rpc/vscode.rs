//! `vscode.*` handlers — the embedded VS Code server (`orkestr-vscode-host`).
//!
//! Install, start and uninstall are recorded in the audit log: the server runs
//! extension code as the OS user (untrusted code, threat model TB8).

use orkestr_domain::AuditOutcome;
use orkestr_protocol::methods::vscode::{
    self as m, VsCodeConfigPatch, VsCodeInstallParams, VsCodeLogsResult, VsCodeStartParams, VsCodeUninstallParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;
use orkestr_security::NewAudit;
use orkestr_vscode_host::HostError;

use super::{RpcContext, RpcRouter};

/// Maps host errors to RPC errors (messages are already redacted).
#[must_use]
pub fn host_to_rpc(e: HostError) -> RpcError {
    match e {
        HostError::Unsupported(_) | HostError::Invalid(_) | HostError::ConsentRequired => {
            RpcError::bad_request(e.to_string())
        }
        HostError::NotInstalled => RpcError::not_found(e.to_string()),
        HostError::Busy(_) | HostError::Cancelled => RpcError::conflict(e.to_string()),
        HostError::Release(_)
        | HostError::Checksum(_)
        | HostError::Network(_)
        | HostError::Archive(_)
        | HostError::Server(_)
        | HostError::Io(_) => RpcError::internal(e.to_string()),
    }
}

async fn audit(ctx: &RpcContext, action: &str, resource: String, details: serde_json::Value) {
    let sec = &ctx.services.security;
    let mut entry = NewAudit::new(sec.user(), action, resource, AuditOutcome::Executed);
    entry.details = details;
    sec.audit.record(entry).await;
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Status, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.vscode.status())
        })
        .method::<m::CheckUpdate, _, _>(|ctx: RpcContext, _: Empty| async move {
            ctx.services.vscode.check_update().await.map_err(host_to_rpc)
        })
        .method::<m::Install, _, _>(|ctx: RpcContext, p: VsCodeInstallParams| async move {
            let status = ctx.services.vscode.install(p.accept, p.version.clone()).await.map_err(host_to_rpc)?;
            audit(
                &ctx,
                "vscode.install",
                "vscodium-reh-web".into(),
                serde_json::json!({ "version": p.version, "target": status.target }),
            )
            .await;
            Ok(status)
        })
        .method::<m::CancelInstall, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.vscode.cancel_install())
        })
        .method::<m::Uninstall, _, _>(|ctx: RpcContext, p: VsCodeUninstallParams| async move {
            let purge = p.purge.unwrap_or(false);
            let status = ctx.services.vscode.uninstall(purge).await.map_err(host_to_rpc)?;
            audit(&ctx, "vscode.uninstall", "vscodium-reh-web".into(), serde_json::json!({ "purge": purge })).await;
            Ok(status)
        })
        .method::<m::Start, _, _>(|ctx: RpcContext, p: VsCodeStartParams| async move {
            let folder = match p.project_id {
                Some(id) => {
                    let project = ctx
                        .services
                        .projects
                        .get(id)
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?
                        .ok_or_else(|| RpcError::not_found(format!("project {id} not found")))?;
                    let root =
                        project.root_path.ok_or_else(|| RpcError::bad_request("the project has no folder on disk"))?;
                    Some(std::path::PathBuf::from(root))
                }
                None => None,
            };
            let was_running = ctx.services.vscode.status().running.is_some();
            let result = ctx.services.vscode.start(folder.as_deref()).await.map_err(host_to_rpc)?;
            if !was_running {
                let resource = folder.as_ref().map_or_else(|| "(no folder)".to_owned(), |f| f.display().to_string());
                let sec = &ctx.services.security;
                let mut entry = NewAudit::new(sec.user(), "vscode.start", resource, AuditOutcome::Executed);
                entry.project_id = p.project_id;
                entry.details = serde_json::json!({ "origin": result.origin });
                sec.audit.record(entry).await;
            }
            Ok(result)
        })
        .method::<m::Stop, _, _>(|ctx: RpcContext, _: Empty| async move {
            ctx.services.vscode.stop().await.map_err(host_to_rpc)
        })
        .method::<m::KeepAlive, _, _>(|ctx: RpcContext, _: Empty| async move {
            ctx.services.vscode.keep_alive();
            Ok::<_, RpcError>(Empty {})
        })
        .method::<m::SetConfig, _, _>(|ctx: RpcContext, p: VsCodeConfigPatch| async move {
            ctx.services.vscode.set_config(p).await.map_err(host_to_rpc)
        })
        .method::<m::Logs, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(VsCodeLogsResult { lines: ctx.services.vscode.logs() })
        });
}

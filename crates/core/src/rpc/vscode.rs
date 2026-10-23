//! `vscode.*` handlers — the embedded VS Code server (`orkestr-vscode-host`).
//!
//! Install, start and uninstall are recorded in the audit log: the server runs
//! extension code as the OS user (untrusted code, threat model TB8).

use std::time::{Duration, Instant};

use orkestr_domain::AuditOutcome;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::vscode::{
    self as m, VsCodeCommandExecutedPayload, VsCodeCommandsParams, VsCodeCommandsResult, VsCodeConfigPatch,
    VsCodeExecuteParams, VsCodeExecuteResult, VsCodeExtensionsResult, VsCodeInstallExtensionParams,
    VsCodeInstallParams, VsCodeLogsResult, VsCodeOpenFileParams, VsCodeOpenViewParams, VsCodeRunTaskParams,
    VsCodeStartParams, VsCodeTaskInfo, VsCodeTasksParams, VsCodeTasksResult, VsCodeUninstallParams,
};
use orkestr_protocol::methods::vscode::{
    VsCodeExtensionIdParams, VsCodeInstallVsixParams, VsCodeSetExtensionEnabledParams, VsCodeSyncThemeParams,
    VsCodeThemeKind,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError, RpcErrorCode};
use orkestr_security::NewAudit;
use orkestr_vscode_host::bridge::BridgeError;
use orkestr_vscode_host::catalog::is_dangerous_command;
use orkestr_vscode_host::HostError;
use serde_json::{json, Value};

use super::{RpcContext, RpcRouter};

/// Longest wait for a VS Code window to connect.
const MAX_WAIT_MS: u32 = 120_000;
/// Upper bound of one bridge call (some commands open dialogs and wait).
const CALL_TIMEOUT: Duration = Duration::from_secs(90);

fn wait_of(ms: Option<u32>) -> Duration {
    Duration::from_millis(u64::from(ms.unwrap_or(0).min(MAX_WAIT_MS)))
}

fn valid_command_id(id: &str) -> Result<&str, RpcError> {
    let id = id.trim();
    if id.is_empty() || id.len() > 200 || id.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(RpcError::bad_request("invalid command id"));
    }
    Ok(id)
}

fn approval_required(command_id: &str) -> RpcError {
    RpcError::new(
        RpcErrorCode::ApprovalRequired,
        format!("`{command_id}` looks destructive: confirm it in Orkestr before it runs"),
    )
    .with_data(json!({ "reason": "dangerousCommand", "commandId": command_id }))
}

/// Runs one bridge call: audit (every call), `VsCodeCommandExecuted` event, error mapping.
async fn run_bridge(
    ctx: &RpcContext,
    label: &str,
    method: &str,
    params: Value,
    wait: Duration,
    details: Value,
) -> Result<VsCodeExecuteResult, RpcError> {
    let started = Instant::now();
    let result = ctx.services.vscode.bridge_call(method, params, wait, CALL_TIMEOUT).await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let error = result.as_ref().err().map(|e| orkestr_security::redact::redact(&e.to_string()));
    let sec = &ctx.services.security;
    let outcome = if result.is_ok() { AuditOutcome::Executed } else { AuditOutcome::Failed };
    let mut entry = NewAudit::new(sec.user(), "vscode.execute", label.to_owned(), outcome);
    entry.details = json!({ "bridgeMethod": method, "durationMs": duration_ms, "error": error, "params": details });
    sec.audit.record(entry).await;
    let payload = VsCodeCommandExecutedPayload { command_id: label.to_owned(), ok: result.is_ok(), error, duration_ms };
    if let Err(e) = ctx.services.events.publish(NewEvent::new(EventKind::VsCodeCommandExecuted(payload))).await {
        tracing::debug!(error = %e, "failed to publish VsCodeCommandExecuted");
    }
    let value = result.map_err(host_to_rpc)?;
    Ok(VsCodeExecuteResult { result: value, duration_ms })
}

/// Title of `command_id` from the installed manifests (for the danger check).
async fn command_title(ctx: &RpcContext, command_id: &str) -> String {
    let list = ctx.services.vscode.catalog_with(None, None, false).await;
    list.iter()
        .flat_map(|e| e.commands.iter())
        .find(|c| c.id == command_id)
        .map(|c| c.title.clone())
        .unwrap_or_default()
}

/// Maps host errors to RPC errors (messages are already redacted).
#[must_use]
pub fn host_to_rpc(e: HostError) -> RpcError {
    match e {
        HostError::Bridge(b) => match b {
            BridgeError::NotConnected => {
                RpcError::conflict(b.to_string()).with_data(json!({ "reason": "bridgeNotConnected" }))
            }
            BridgeError::Remote(_) => {
                RpcError::bad_request(b.to_string()).with_data(json!({ "reason": "commandFailed" }))
            }
            BridgeError::Timeout | BridgeError::Disconnected => {
                RpcError::internal(b.to_string()).with_data(json!({ "reason": "bridgeUnavailable" }))
            }
        },
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
        })
        .method::<m::ListExtensions, _, _>(|ctx: RpcContext, _: Empty| async move {
            let extensions = ctx.services.vscode.list_extensions().await.map_err(host_to_rpc)?;
            Ok::<_, RpcError>(VsCodeExtensionsResult { extensions })
        })
        .method::<m::InstallExtension, _, _>(|ctx: RpcContext, p: VsCodeInstallExtensionParams| async move {
            let extensions = ctx.services.vscode.install_extension(&p.id).await.map_err(host_to_rpc)?;
            // Extension code runs as the OS user (TB8): every install is audited.
            audit(
                &ctx,
                "vscode.installExtension",
                p.id.trim().to_ascii_lowercase(),
                serde_json::json!({ "registry": "open-vsx" }),
            )
            .await;
            Ok(VsCodeExtensionsResult { extensions })
        })
        .method::<m::UninstallExtension, _, _>(|ctx: RpcContext, p: VsCodeExtensionIdParams| async move {
            let extensions = ctx.services.vscode.uninstall_extension(&p.id).await.map_err(host_to_rpc)?;
            audit(&ctx, "vscode.uninstallExtension", p.id.trim().to_ascii_lowercase(), json!({})).await;
            Ok::<_, RpcError>(VsCodeExtensionsResult { extensions })
        })
        .method::<m::InstallVsix, _, _>(|ctx: RpcContext, p: VsCodeInstallVsixParams| async move {
            let path = std::path::PathBuf::from(p.path.trim());
            let extensions = ctx.services.vscode.install_vsix(&path).await.map_err(host_to_rpc)?;
            // A local package is unsigned extension code (TB8): audited like Open VSX installs.
            audit(&ctx, "vscode.installExtension", path.display().to_string(), json!({ "registry": "vsix" })).await;
            Ok::<_, RpcError>(VsCodeExtensionsResult { extensions })
        })
        .method::<m::UpdateExtensions, _, _>(|ctx: RpcContext, _: Empty| async move {
            let extensions = ctx.services.vscode.update_extensions().await.map_err(host_to_rpc)?;
            audit(&ctx, "vscode.updateExtensions", "open-vsx".into(), json!({ "count": extensions.len() })).await;
            Ok::<_, RpcError>(VsCodeExtensionsResult { extensions })
        })
        .method::<m::SetExtensionEnabled, _, _>(|ctx: RpcContext, p: VsCodeSetExtensionEnabledParams| async move {
            let extensions = ctx.services.vscode.set_extension_enabled(&p.id, p.enabled).await.map_err(host_to_rpc)?;
            audit(
                &ctx,
                "vscode.setExtensionEnabled",
                p.id.trim().to_ascii_lowercase(),
                json!({ "enabled": p.enabled }),
            )
            .await;
            Ok::<_, RpcError>(VsCodeExtensionsResult { extensions })
        })
        .method::<m::SyncTheme, _, _>(|ctx: RpcContext, p: VsCodeSyncThemeParams| async move {
            // Cosmetic and frequent (every theme switch): not audited.
            let light = p.kind == VsCodeThemeKind::Light;
            ctx.services.vscode.sync_theme(light, &p.colors, wait_of(p.wait_ms)).await.map_err(host_to_rpc)?;
            Ok::<_, RpcError>(Empty {})
        })
        .method::<m::Commands, _, _>(|ctx: RpcContext, p: VsCodeCommandsParams| async move {
            let extensions = ctx.services.vscode.catalog(p.locale.as_deref(), p.extension_id.as_deref()).await;
            let bridge = ctx.services.vscode.bridge();
            let connected = bridge.is_connected();
            Ok::<_, RpcError>(VsCodeCommandsResult {
                extensions,
                bridge_connected: connected,
                workspace_trusted: if connected { bridge.hello().map(|h| h.trusted) } else { None },
            })
        })
        .method::<m::Execute, _, _>(|ctx: RpcContext, p: VsCodeExecuteParams| async move {
            let id = valid_command_id(&p.command_id)?.to_owned();
            let confirmed = p.confirmed.unwrap_or(false);
            if !confirmed {
                let dangerous = id.starts_with('_')
                    || is_dangerous_command(&id, "")
                    || is_dangerous_command(&id, &command_title(&ctx, &id).await);
                if dangerous {
                    let sec = &ctx.services.security;
                    let mut entry = NewAudit::new(sec.user(), "vscode.execute", id.clone(), AuditOutcome::Denied);
                    entry.details = json!({ "reason": "confirmation required" });
                    sec.audit.record(entry).await;
                    return Err(approval_required(&id));
                }
            }
            let args = p.args.unwrap_or_default();
            if serde_json::to_vec(&args).map_or(true, |b| b.len() > 256 * 1024) {
                return Err(RpcError::bad_request("command arguments are too large"));
            }
            let details = json!({ "args": args.len(), "confirmed": confirmed });
            run_bridge(&ctx, &id, "execute", json!({ "command": id, "args": args }), wait_of(p.wait_ms), details).await
        })
        .method::<m::OpenView, _, _>(|ctx: RpcContext, p: VsCodeOpenViewParams| async move {
            let ext_id = p.extension_id.trim().to_ascii_lowercase();
            let ext = ctx
                .services
                .vscode
                .catalog_with(None, Some(&ext_id), false)
                .await
                .into_iter()
                .next()
                .ok_or_else(|| RpcError::not_found(format!("extension {ext_id} is not installed in VS Code")))?;
            let (command, args) = if p.settings.unwrap_or(false) {
                ("workbench.action.openSettings".to_owned(), vec![json!(format!("@ext:{}", ext.id))])
            } else {
                let container = match p.container_id.as_deref() {
                    Some(c) => ext.view_containers.iter().find(|v| v.id == c),
                    None => ext.view_containers.first(),
                };
                match container {
                    Some(v) => (v.command.clone(), Vec::new()),
                    // No view container: show the extension's page instead.
                    None => ("extension.open".to_owned(), vec![json!(ext.id)]),
                }
            };
            let details = json!({ "extensionId": ext.id });
            run_bridge(
                &ctx,
                &command,
                "execute",
                json!({ "command": command, "args": args }),
                wait_of(p.wait_ms),
                details,
            )
            .await
        })
        .method::<m::OpenFile, _, _>(|ctx: RpcContext, p: VsCodeOpenFileParams| async move {
            let path = std::path::Path::new(&p.path);
            if !path.is_absolute() || !path.is_file() {
                return Err(RpcError::bad_request("the file does not exist (absolute path required)"));
            }
            let params = json!({ "path": p.path, "line": p.line, "column": p.column });
            run_bridge(
                &ctx,
                "openFile",
                "openFile",
                params,
                wait_of(p.wait_ms),
                json!({ "path": p.path, "line": p.line }),
            )
            .await
        })
        .method::<m::Tasks, _, _>(|ctx: RpcContext, p: VsCodeTasksParams| async move {
            let value = ctx
                .services
                .vscode
                .bridge_call("listTasks", Value::Null, wait_of(p.wait_ms), CALL_TIMEOUT)
                .await
                .map_err(host_to_rpc)?;
            let tasks: Vec<VsCodeTaskInfo> = serde_json::from_value(value).unwrap_or_default();
            Ok(VsCodeTasksResult { tasks })
        })
        .method::<m::RunTask, _, _>(|ctx: RpcContext, p: VsCodeRunTaskParams| async move {
            let label = format!("runTask:{}", p.name);
            // A task runs a shell command: always confirmed by the user.
            if !p.confirmed.unwrap_or(false) {
                return Err(approval_required(&label));
            }
            let params = json!({ "name": p.name, "source": p.source });
            run_bridge(&ctx, &label, "runTask", params.clone(), wait_of(p.wait_ms), params).await
        });
}

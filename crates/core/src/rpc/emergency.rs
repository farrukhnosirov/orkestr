//! `emergency.*` handlers (§52). Purely local: no provider/network calls.
//! (`emergency.lockVault` is registered by `rpc::secrets`.)

use orkestr_protocol::methods::emergency::{
    self as m, EmergencyFlagsView, EmergencyStatusResult, EmergencyStopResult, StopAgentParams, StopProjectParams,
    ToggleParams,
};
use orkestr_protocol::RpcError;
use orkestr_security::{EmergencyFlags, EmergencyOutcome};

use super::{RpcContext, RpcRouter};
use crate::error::security_to_rpc;
use crate::Services;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::StopAll, _, _>(|ctx: RpcContext, _| async move { stop_all(&ctx.services).await })
        .method::<m::StopProject, _, _>(|ctx: RpcContext, p: StopProjectParams| async move {
            let sec = &ctx.services.security;
            let out = sec.stop_project(p.project_id, sec.user()).await.map_err(security_to_rpc)?;
            let terminals = ctx.services.terminals.stop_project_agents(p.project_id).await;
            let terminals = terminals + ctx.services.quality.cancel_project(p.project_id);
            let terminals = terminals + ctx.services.claude.manager.stop_project(p.project_id);
            Ok(stop_result(&ctx, out, terminals))
        })
        .method::<m::StopAgent, _, _>(|ctx: RpcContext, p: StopAgentParams| async move {
            let sec = &ctx.services.security;
            let out = sec.stop_agent(p.agent_id, sec.user()).await.map_err(security_to_rpc)?;
            let terminals = ctx.services.terminals.stop_agent(p.agent_id).await;
            Ok(stop_result(&ctx, out, terminals))
        })
        .method::<m::DisableFullAuto, _, _>(|ctx: RpcContext, p: ToggleParams| async move {
            let sec = &ctx.services.security;
            sec.disable_full_auto(p.disabled.unwrap_or(true), sec.user()).await.map_err(security_to_rpc)?;
            Ok(status(&ctx))
        })
        .method::<m::DisableNetworkTools, _, _>(|ctx: RpcContext, p: ToggleParams| async move {
            let sec = &ctx.services.security;
            sec.disable_network_tools(p.disabled.unwrap_or(true), sec.user()).await.map_err(security_to_rpc)?;
            Ok(status(&ctx))
        })
        .method::<m::Status, _, _>(|ctx: RpcContext, _| async move { Ok::<_, RpcError>(status(&ctx)) })
        .method::<m::Reset, _, _>(|ctx: RpcContext, _| async move {
            let sec = &ctx.services.security;
            sec.reset_emergency(sec.user()).await.map_err(security_to_rpc)?;
            Ok(status(&ctx))
        });
}

/// Stop All AI (§52): every agent, agent terminal and quality run. The
/// `emergency.stopAll` handler and embedding hosts (the desktop tray's native
/// *Stop All AI*, which must work without the webview) share this path.
pub async fn stop_all(services: &Services) -> Result<EmergencyStopResult, RpcError> {
    let sec = &services.security;
    let out = sec.stop_all(sec.user()).await.map_err(security_to_rpc)?;
    let terminals = services.terminals.stop_all_agents().await;
    let terminals = terminals + services.quality.cancel_all();
    // Claude Code chat sessions (their tokens are children of the emergency root too).
    let terminals = terminals + services.claude.manager.stop_all();
    // Phase 7: plugin calls (kill switch), deployments, Bug Detective runs.
    let killed = services.plugins.kill(None, "emergency stop").await;
    let terminals = terminals + killed.calls_interrupted as usize;
    let terminals = terminals + services.deploy.cancel_all().await;
    let terminals = terminals + services.detective.cancel_all().await;
    // Computer assistant: running pipelines, pending confirmations, timers.
    let stopped = services.computer.stop().await;
    let terminals = terminals + stopped.runs_stopped as usize;
    // The VS Code server runs (untrusted) extension code: Stop All stops it too.
    let vscode = usize::from(services.vscode.status().running.is_some());
    if let Err(e) = services.vscode.stop().await {
        tracing::error!(error = %e, "Stop All AI could not stop the VS Code server");
    }
    Ok(stop_result_for(services, out, terminals + vscode))
}

/// Wire form of the flags.
#[must_use]
pub fn flags_view(f: EmergencyFlags) -> EmergencyFlagsView {
    EmergencyFlagsView {
        all_stopped: f.all_stopped,
        full_auto_disabled: f.full_auto_disabled,
        network_tools_disabled: f.network_tools_disabled,
        vault_locked: f.vault_locked,
    }
}

fn status(ctx: &RpcContext) -> EmergencyStatusResult {
    let e = &ctx.services.security.emergency;
    EmergencyStatusResult {
        flags: flags_view(e.flags()),
        active_agents: u32::try_from(e.active_agents()).unwrap_or(u32::MAX),
    }
}

/// `terminals` = agent commands / agent terminals stopped by the terminal service.
fn stop_result(ctx: &RpcContext, out: EmergencyOutcome, terminals: usize) -> EmergencyStopResult {
    stop_result_for(&ctx.services, out, terminals)
}

fn stop_result_for(services: &Services, out: EmergencyOutcome, terminals: usize) -> EmergencyStopResult {
    EmergencyStopResult {
        agents_stopped: out.report.agents_stopped,
        other_stopped: out.report.other_stopped.saturating_add(u32::try_from(terminals).unwrap_or(u32::MAX)),
        approvals_cancelled: out.approvals_cancelled,
        flags: flags_view(services.security.emergency.flags()),
    }
}

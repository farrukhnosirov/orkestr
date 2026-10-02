//! `terminal.*` handlers (§15, ADR-0014) and the event bridge.
//!
//! * [`EventTerminalSink`] publishes the terminal service's output as
//!   `TerminalOutput` (ephemeral, routed only to `terminal:<id>`) and
//!   `TerminalExited` events.
//! * Project-scoped requests are confined to the project's root folder.
//! * Requests from RPC clients act as the user, except `agentId`-scoped
//!   `terminal.open`/`terminal.run` and `terminal.execLine`, which act as the
//!   owning agent and therefore go through the command gate.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::ProjectId;
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, TerminalExitedPayload, TerminalOutputPayload};
use orkestr_protocol::methods::terminal::{
    self as m, ExecLineParams, ListShellsResult, ListTerminalsParams, ListTerminalsResult, OpenTerminalParams,
    RenameTerminalParams, ResizeTerminalParams, RunCommandParams, RunCommandResult, ShellDescriptor, TerminalEnvVar,
    TerminalHistoryParams, TerminalHistoryResult, TerminalIdParams, TerminalOwnerInfo, TerminalSession,
    TerminalSessionKind, TerminalSessionResult, TerminalSessionStatus, TerminalShellKind, WriteTerminalParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError, RpcErrorCode};
use orkestr_terminal::{
    EnvVar, OpenRequest, RunRequest, SessionKind, ShellKind, ShellSpec, SinkMeta, TerminalError, TerminalInfo,
    TerminalOwner, TerminalSink, TerminalStatus,
};
use tokio_util::sync::CancellationToken;

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Shells, _, _>(|ctx: RpcContext, p| async move {
            let shells = ctx.services.terminals.shells(p.refresh.unwrap_or(false));
            Ok(ListShellsResult {
                shells: shells
                    .into_iter()
                    .map(|s| ShellDescriptor {
                        id: s.id,
                        kind: shell_kind(s.kind),
                        name: s.name,
                        path: s.program.to_string_lossy().into_owned(),
                        args: s.args,
                        is_default: s.is_default,
                    })
                    .collect(),
            })
        })
        .method::<m::Open, _, _>(open)
        .method::<m::Write, _, _>(|ctx: RpcContext, p: WriteTerminalParams| async move {
            if p.data.len() > 1024 * 1024 {
                return Err(RpcError::bad_request("input larger than 1 MiB"));
            }
            ctx.services.terminals.write(p.terminal_id, TerminalOwner::User, &p.data).await.map_err(to_rpc)?;
            Ok(Empty {})
        })
        .method::<m::ExecLine, _, _>(|ctx: RpcContext, p: ExecLineParams| async move {
            ctx.services.terminals.exec_line(p.terminal_id, &p.line).await.map_err(to_rpc)?;
            Ok(Empty {})
        })
        .method::<m::Resize, _, _>(|ctx: RpcContext, p: ResizeTerminalParams| async move {
            ctx.services.terminals.resize(p.terminal_id, p.cols, p.rows).map_err(to_rpc)?;
            Ok(Empty {})
        })
        .method::<m::Rename, _, _>(|ctx: RpcContext, p: RenameTerminalParams| async move {
            let info = ctx.services.terminals.rename(p.terminal_id, &p.title).map_err(to_rpc)?;
            Ok(TerminalSessionResult { terminal: session_dto(info) })
        })
        .method::<m::Close, _, _>(|ctx: RpcContext, p: TerminalIdParams| async move {
            ctx.services.terminals.close(p.terminal_id, TerminalOwner::User).await.map_err(to_rpc)?;
            Ok(Empty {})
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListTerminalsParams| async move {
            let terminals = ctx.services.terminals.list(p.project_id).into_iter().map(session_dto).collect();
            Ok(ListTerminalsResult { terminals })
        })
        .method::<m::History, _, _>(|ctx: RpcContext, p: TerminalHistoryParams| async move {
            let h = ctx.services.terminals.history(p.terminal_id, p.max_bytes.map(|b| b as usize)).map_err(to_rpc)?;
            Ok(TerminalHistoryResult {
                terminal_id: p.terminal_id,
                data: h.data,
                start_offset: h.start_offset,
                end_offset: h.end_offset,
                truncated: h.truncated,
            })
        })
        .method::<m::Run, _, _>(run);
}

async fn project_root(ctx: &RpcContext, project_id: Option<ProjectId>) -> Result<Option<PathBuf>, RpcError> {
    let Some(pid) = project_id else { return Ok(None) };
    let project = ctx
        .services
        .projects
        .get(pid)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {pid} not found")))?;
    let root = project
        .root_path
        .ok_or_else(|| RpcError::bad_request("the project has no root folder; set one before opening terminals"))?;
    Ok(Some(PathBuf::from(root)))
}

fn shell_spec(shell_id: Option<String>, custom: Option<m::CustomShell>) -> ShellSpec {
    match (custom, shell_id) {
        (Some(c), _) => ShellSpec::Custom { program: PathBuf::from(c.program), args: c.args },
        (None, Some(id)) if !id.is_empty() => ShellSpec::Id(id),
        _ => ShellSpec::Default,
    }
}

fn env_vars(env: Option<Vec<TerminalEnvVar>>) -> Vec<EnvVar> {
    env.unwrap_or_default().into_iter().map(|v| EnvVar::new(v.name, v.value)).collect()
}

async fn open(ctx: RpcContext, p: OpenTerminalParams) -> Result<TerminalSessionResult, RpcError> {
    let owner = p.agent_id.map_or(TerminalOwner::User, |agent_id| TerminalOwner::Agent { agent_id });
    if owner.is_agent() && p.project_id.is_none() {
        return Err(RpcError::bad_request("agent terminals need a projectId"));
    }
    let root = project_root(&ctx, p.project_id).await?;
    let req = OpenRequest {
        project_id: p.project_id,
        root,
        cwd: p.cwd.filter(|c| !c.trim().is_empty()).map(PathBuf::from),
        owner,
        shell: shell_spec(p.shell_id, p.custom_shell),
        env: env_vars(p.env),
        cols: p.cols.unwrap_or(120),
        rows: p.rows.unwrap_or(30),
        title: p.title,
    };
    let info = ctx.services.terminals.open(req).await.map_err(to_rpc)?;
    Ok(TerminalSessionResult { terminal: session_dto(info) })
}

async fn run(ctx: RpcContext, p: RunCommandParams) -> Result<RunCommandResult, RpcError> {
    let owner = p.agent_id.map_or(TerminalOwner::User, |agent_id| TerminalOwner::Agent { agent_id });
    let root = project_root(&ctx, p.project_id).await?;
    let req = RunRequest {
        project_id: p.project_id,
        task_id: p.task_id,
        root,
        cwd: p.cwd.filter(|c| !c.trim().is_empty()).map(PathBuf::from),
        owner,
        shell: shell_spec(p.shell_id, None),
        command: p.command,
        env: env_vars(p.env),
        timeout: p.timeout_ms.map(Duration::from_millis),
        max_output: p.max_output_bytes.map(|b| b as usize),
        stream_to_view: p.stream,
    };
    let out = ctx.services.terminals.run(req, CancellationToken::new()).await.map_err(to_rpc)?;
    Ok(RunCommandResult {
        exit_code: out.exit_code,
        stdout: out.stdout,
        stderr: out.stderr,
        truncated: out.truncated,
        timed_out: out.timed_out,
        duration_ms: u64::try_from(out.duration.as_millis()).unwrap_or(u64::MAX),
        terminal_id: out.view_terminal_id,
    })
}

fn shell_kind(k: ShellKind) -> TerminalShellKind {
    match k {
        ShellKind::PowerShell7 => TerminalShellKind::PowerShell7,
        ShellKind::WindowsPowerShell => TerminalShellKind::WindowsPowerShell,
        ShellKind::Cmd => TerminalShellKind::Cmd,
        ShellKind::GitBash => TerminalShellKind::GitBash,
        ShellKind::Wsl => TerminalShellKind::Wsl,
        ShellKind::Bash => TerminalShellKind::Bash,
        ShellKind::Zsh => TerminalShellKind::Zsh,
        ShellKind::Fish => TerminalShellKind::Fish,
        ShellKind::Sh => TerminalShellKind::Sh,
        ShellKind::Custom => TerminalShellKind::Custom,
    }
}

fn session_dto(info: TerminalInfo) -> TerminalSession {
    let (status, exit_code) = match info.status {
        TerminalStatus::Running => (TerminalSessionStatus::Running, None),
        TerminalStatus::Exited { exit_code } => (TerminalSessionStatus::Exited, exit_code),
    };
    let kind = match info.kind {
        SessionKind::Pty => TerminalSessionKind::Pty,
        SessionKind::CommandView => TerminalSessionKind::CommandView,
    };
    let owner = match info.owner {
        TerminalOwner::User => TerminalOwnerInfo::User,
        TerminalOwner::Agent { agent_id } => TerminalOwnerInfo::Agent { agent_id },
    };
    TerminalSession {
        terminal_id: info.id,
        project_id: info.project_id,
        read_only: info.owner.is_agent() || info.kind == SessionKind::CommandView,
        owner,
        kind,
        shell_id: info.shell_id,
        shell_kind: shell_kind(info.shell_kind),
        title: info.title,
        cwd: info.cwd.to_string_lossy().into_owned(),
        cols: info.cols,
        rows: info.rows,
        pid: info.pid,
        status,
        exit_code,
        created_at: info.created_at,
        output_bytes: info.output_bytes,
    }
}

/// Maps terminal errors to RPC errors (internal details are logged, not leaked).
pub fn to_rpc(err: TerminalError) -> RpcError {
    match err {
        TerminalError::NotFound(id) => RpcError::not_found(format!("terminal {id} not found")),
        e @ (TerminalError::UnknownShell(_)
        | TerminalError::NoShell(_)
        | TerminalError::Invalid(_)
        | TerminalError::InvalidCwd(_)) => RpcError::bad_request(e.to_string()),
        e @ (TerminalError::CwdOutsideRoot { .. } | TerminalError::Forbidden(_) | TerminalError::Denied(_)) => {
            RpcError::forbidden(e.to_string())
        }
        e @ (TerminalError::LimitReached(_)
        | TerminalError::Exited(_)
        | TerminalError::Cancelled
        | TerminalError::ShuttingDown) => RpcError::conflict(e.to_string()),
        TerminalError::ApprovalRequired { reason, approval_id } => {
            RpcError::new(RpcErrorCode::ApprovalRequired, format!("command requires approval: {reason}"))
                .with_data(serde_json::json!({ "approvalId": approval_id }))
        }
        e @ (TerminalError::Pty(_) | TerminalError::Io(_)) => {
            tracing::warn!(error = %e, "terminal operation failed");
            RpcError::internal(format!("terminal operation failed: {e}"))
        }
    }
}

/// Publishes terminal output and exits on the event bus.
#[derive(Debug, Clone)]
pub struct EventTerminalSink {
    bus: EventBus,
}

impl EventTerminalSink {
    /// New sink over `bus`.
    #[must_use]
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    fn scoped(kind: EventKind, meta: SinkMeta) -> NewEvent {
        let mut ev = NewEvent::new(kind);
        if let Some(p) = meta.project_id {
            ev = ev.project(p);
        }
        if let Some(a) = meta.agent_id {
            ev = ev.agent(a);
        }
        ev
    }
}

#[async_trait]
impl TerminalSink for EventTerminalSink {
    async fn output(&self, meta: SinkMeta, offset: u64, data: String) {
        let kind = EventKind::TerminalOutput(TerminalOutputPayload {
            terminal_id: meta.terminal_id,
            data,
            offset: Some(offset),
        });
        if let Err(e) = self.bus.publish(Self::scoped(kind, meta)).await {
            tracing::warn!(terminal_id = %meta.terminal_id, error = %e, "failed to publish terminal output");
        }
    }

    async fn exited(&self, meta: SinkMeta, exit_code: Option<i32>) {
        let kind = EventKind::TerminalExited(TerminalExitedPayload { terminal_id: meta.terminal_id, exit_code });
        if let Err(e) = self.bus.publish(Self::scoped(kind, meta)).await {
            tracing::warn!(terminal_id = %meta.terminal_id, error = %e, "failed to publish terminal exit");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert_eq!(to_rpc(TerminalError::Denied("x".into())).code, RpcErrorCode::Forbidden);
        assert_eq!(to_rpc(TerminalError::LimitReached(3)).code, RpcErrorCode::Conflict);
        let e = to_rpc(TerminalError::ApprovalRequired { reason: "r".into(), approval_id: Some("a1".into()) });
        assert_eq!(e.code, RpcErrorCode::ApprovalRequired);
        assert_eq!(e.data.unwrap()["approvalId"], "a1");
        assert_eq!(to_rpc(TerminalError::Invalid("x".into())).code, RpcErrorCode::BadRequest);
    }

    #[test]
    fn shell_spec_precedence() {
        assert_eq!(shell_spec(None, None), ShellSpec::Default);
        assert_eq!(shell_spec(Some(String::new()), None), ShellSpec::Default);
        assert_eq!(shell_spec(Some("cmd".into()), None), ShellSpec::Id("cmd".into()));
        assert!(matches!(
            shell_spec(Some("cmd".into()), Some(m::CustomShell { program: "x".into(), args: vec![] })),
            ShellSpec::Custom { .. }
        ));
    }
}

//! Cross-area adapters wired in the composition root.
//!
//! [`SecurityCommandGate`] connects the terminal's [`CommandGate`] seam to the
//! security area: `PermissionEngine::check` (rules, approval modes, emergency
//! flags) + `DangerousActionGuard` (classification) + `ApprovalService`
//! (approval requests). Contract §6: dangerous commands always ask, even in
//! Full Auto; forbidden commands are always denied.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{Actor, ApprovalId, ApprovalMode, ApprovalStatus, Capability, ProjectId};
use orkestr_security::{ApprovalRequest, CheckContext, Decision, PermissionEngine, Resource, Security, ShellKind};
use orkestr_storage::{ProjectRepository, SqliteMetadataStore};
use orkestr_terminal::{CommandGate, GateDecision, GateMode, GateRequest, TerminalOwner};
use parking_lot::Mutex;

/// Maps the terminal's shell family onto the guard's grammar family.
#[must_use]
pub fn guard_shell(kind: orkestr_terminal::ShellKind) -> ShellKind {
    use orkestr_terminal::ShellKind as T;
    match kind {
        T::PowerShell7 | T::WindowsPowerShell => ShellKind::PowerShell,
        T::Cmd => ShellKind::Cmd,
        T::GitBash | T::Wsl | T::Bash | T::Zsh | T::Custom => ShellKind::Bash,
        T::Fish | T::Sh => ShellKind::Sh,
    }
}

/// Identity of a gated command: an approval is bound to exactly this
/// (owner, project, shell, command, cwd) and is consumed by one execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CommandKey {
    owner: TerminalOwner,
    project: Option<ProjectId>,
    shell: ShellKind,
    command: String,
    cwd: String,
}

/// Terminal gate backed by the security subsystem.
///
/// When a command needs approval the gate files an approval request and
/// answers `Ask { approval_id }`; the caller re-issues the same command once
/// the user approved it, and the gate then lets it through **once**.
/// A rejected approval turns the retry into a `Deny`; an expired/cancelled
/// one files a fresh request.
pub struct SecurityCommandGate {
    security: Arc<Security>,
    projects: Arc<SqliteMetadataStore>,
    pending: Mutex<HashMap<CommandKey, ApprovalId>>,
}

impl std::fmt::Debug for SecurityCommandGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityCommandGate").field("pending", &self.pending.lock().len()).finish_non_exhaustive()
    }
}

impl SecurityCommandGate {
    /// Creates the gate.
    #[must_use]
    pub fn new(security: Arc<Security>, projects: Arc<SqliteMetadataStore>) -> Self {
        Self { security, projects, pending: Mutex::new(HashMap::new()) }
    }

    async fn approval_mode(
        &self,
        project: Option<ProjectId>,
        actor_agent: Option<orkestr_domain::AgentId>,
    ) -> ApprovalMode {
        let project_mode = match project {
            Some(id) => match self.projects.get(id).await {
                Ok(Some(p)) => p.approval_mode,
                _ => ApprovalMode::default(),
            },
            None => ApprovalMode::default(),
        };
        self.security
            .permissions
            .effective_mode(project_mode, actor_agent)
            .await
            .unwrap_or_else(|_| self.security.engine.effective_mode(project_mode))
    }

    /// Outcome of an earlier approval for the same command, if any.
    async fn previous_approval(&self, key: &CommandKey) -> Option<GateDecision> {
        let id = *self.pending.lock().get(key)?;
        let approval = match self.security.approvals.get(id).await {
            Ok(Some(a)) => a,
            _ => {
                self.pending.lock().remove(key);
                return None;
            }
        };
        match approval.status {
            ApprovalStatus::Approved => {
                self.pending.lock().remove(key);
                Some(GateDecision::Allow)
            }
            ApprovalStatus::Pending => Some(GateDecision::Ask {
                reason: format!("waiting for approval: {}", approval.reason),
                approval_id: Some(id.to_string()),
            }),
            ApprovalStatus::Rejected => {
                self.pending.lock().remove(key);
                Some(GateDecision::Deny { reason: "the command was rejected by the user".into() })
            }
            ApprovalStatus::Expired | ApprovalStatus::Cancelled => {
                self.pending.lock().remove(key);
                None
            }
        }
    }
}

#[async_trait]
impl CommandGate for SecurityCommandGate {
    async fn check(&self, request: &GateRequest) -> GateDecision {
        let agent = request.agent_id();
        let actor = match agent {
            Some(a) => Actor::Agent(a),
            None => self.security.user(),
        };
        let shell = guard_shell(request.shell);
        let cwd = request.cwd.to_string_lossy().into_owned();
        let key = CommandKey {
            owner: request.owner,
            project: request.project_id,
            shell,
            command: request.command.clone(),
            cwd: cwd.clone(),
        };

        let mode = self.approval_mode(request.project_id, agent).await;
        let ctx = CheckContext {
            project_id: request.project_id,
            agent_id: agent,
            task_id: request.task_id,
            approval_mode: mode,
            // The terminal already confines agent commands to their root;
            // the cwd is the narrowest root the guard can reason about.
            worktree_root: Some(request.cwd.clone()),
            protected_environment: false,
        };
        let resource = Resource::Command { command: request.command.clone(), shell, cwd: Some(".".into()) };
        let decision = self.security.engine.check(&actor, Capability::Terminal, &resource, &ctx);

        match decision {
            Decision::Allow => GateDecision::Allow,
            Decision::Deny(r) => {
                self.pending.lock().remove(&key);
                GateDecision::Deny { reason: r.message }
            }
            Decision::Ask(reason) => {
                if let Some(previous) = self.previous_approval(&key).await {
                    return previous;
                }
                let mut req = ApprovalRequest::new(
                    Capability::Terminal,
                    format!(
                        "Run command{}",
                        if request.mode == GateMode::InteractiveLine { " (agent terminal)" } else { "" }
                    ),
                    reason.message.clone(),
                    reason.risk,
                );
                req.project_id = request.project_id;
                req.task_id = request.task_id;
                req.agent_id = agent;
                req.command_preview = Some(request.command.clone());
                req.danger = reason.danger;
                match self.security.approvals.request(req).await {
                    Ok(a) => {
                        self.pending.lock().insert(key, a.id);
                        GateDecision::Ask { reason: reason.message, approval_id: Some(a.id.to_string()) }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "could not file an approval request; refusing the command");
                        GateDecision::Deny { reason: format!("approval could not be requested: {e}") }
                    }
                }
            }
        }
    }
}

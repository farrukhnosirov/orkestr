//! Event envelope and kinds (§49, contract §4).
//!
//! Wire shape (camelCase, `kind` + `payload` adjacently tagged and flattened
//! into the envelope):
//!
//! ```json
//! { "seq": 42, "id": "…", "ts": "2026-10-02T10:00:00Z",
//!   "projectId": "…", "taskId": null, "agentId": null, "nodeId": null,
//!   "correlationId": null,
//!   "kind": "ProjectCreated", "payload": { "project": { … } } }
//! ```
//!
//! **Append-only:** new kinds may be added; existing kinds are never renamed
//! or removed, and payloads only gain optional fields.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    Actor, AgentId, AgentInstance, AgentState, Approval, ApprovalId, ApprovalStatus, AuthState, CorrelationId,
    Decision, Deployment, EventId, MemoryItem, ModelRef, Node, NodeId, Notification, Project, ProjectId,
    ProviderHealth, ProviderId, ReviewState, SnapshotId, Task, TaskId, TaskStatus, TerminalId, Timestamp, ToolCall,
    ToolCallId, ToolCallStatus, WorkspaceSnapshot,
};

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// `ProjectCreated` / `ProjectUpdated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectPayload {
    /// Full project after the change.
    pub project: Project,
}

/// `ProjectDeleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectDeletedPayload {
    /// Deleted project.
    pub project_id: ProjectId,
}

/// `TaskCreated` / `TaskUpdated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskPayload {
    /// Full task after the change.
    pub task: Task,
}

/// `TaskStatusChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskStatusChangedPayload {
    /// Task.
    pub task_id: TaskId,
    /// Previous status.
    pub from: TaskStatus,
    /// New status.
    pub to: TaskStatus,
    /// Optional reason ("dependency failed", "crash recovery", …).
    pub reason: Option<String>,
}

/// `AgentSpawned`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentSpawnedPayload {
    /// The new agent.
    pub agent: AgentInstance,
}

/// `AgentStarted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStartedPayload {
    /// Agent.
    pub agent_id: AgentId,
    /// Task it started working on.
    pub task_id: Option<TaskId>,
}

/// `AgentStateChanged` — drives floating robot animations (§30).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStateChangedPayload {
    /// Agent.
    pub agent_id: AgentId,
    /// Previous state.
    pub from: AgentState,
    /// New state.
    pub to: AgentState,
    /// Short human-readable activity ("Running cargo test").
    pub activity: Option<String>,
}

/// Why an agent stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum StopReason {
    /// Finished its work.
    Completed,
    /// Stopped by a user.
    UserRequested,
    /// Budget/turn/time limit hit.
    LimitReached,
    /// Emergency stop.
    EmergencyStop,
    /// Unrecoverable error.
    Error,
    /// Core shutdown.
    Shutdown,
}

/// `AgentStopped`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStoppedPayload {
    /// Agent.
    pub agent_id: AgentId,
    /// Reason.
    pub reason: StopReason,
    /// Optional detail (redacted).
    pub detail: Option<String>,
}

/// Author of a streamed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MessageRole {
    /// The user.
    User,
    /// The assistant/agent.
    Assistant,
    /// System/status line.
    System,
}

/// `AgentMessage` / `ManagerMessage` — streamed output delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageDeltaPayload {
    /// Agent (absent for the Manager).
    pub agent_id: Option<AgentId>,
    /// Stable id of the message being streamed.
    pub message_id: String,
    /// Role.
    pub role: MessageRole,
    /// Text delta (append to previous deltas of the same message).
    pub delta: String,
    /// True on the last delta.
    pub done: bool,
}

/// `ToolCallStarted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolCallStartedPayload {
    /// Tool call (input already redacted).
    pub tool_call: ToolCall,
}

/// `ToolCallFinished`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolCallFinishedPayload {
    /// Tool call.
    pub tool_call_id: ToolCallId,
    /// Final status.
    pub status: ToolCallStatus,
    /// Short output summary.
    pub output_summary: Option<String>,
    /// Duration in ms.
    pub duration_ms: u64,
}

/// File change kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum FileChangeKind {
    /// Created.
    Created,
    /// Modified.
    Modified,
    /// Deleted.
    Deleted,
    /// Renamed (see `oldPath`).
    Renamed,
}

/// `FileChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileChangedPayload {
    /// Path relative to the workspace root.
    pub path: String,
    /// Previous path for renames.
    pub old_path: Option<String>,
    /// Kind.
    pub change: FileChangeKind,
}

/// `BuildStarted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BuildStartedPayload {
    /// Build run id.
    pub build_id: String,
    /// Command line (redacted).
    pub command: String,
}

/// `BuildSucceeded` / `BuildFailed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BuildFinishedPayload {
    /// Build run id.
    pub build_id: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Duration in ms.
    pub duration_ms: u64,
    /// Summary (first errors for failures).
    pub summary: String,
}

/// `TestRunFinished`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TestRunFinishedPayload {
    /// Test run id.
    pub run_id: String,
    /// Passed tests.
    pub passed: u32,
    /// Failed tests.
    pub failed: u32,
    /// Skipped tests.
    pub skipped: u32,
    /// Duration in ms.
    pub duration_ms: u64,
    /// Summary.
    pub summary: String,
}

/// `ReviewFinished`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReviewFinishedPayload {
    /// Reviewer agent.
    pub reviewer_agent_id: Option<AgentId>,
    /// Verdict.
    pub verdict: ReviewState,
    /// Number of findings.
    pub findings: u32,
    /// Summary.
    pub summary: String,
}

/// `ApprovalRequested`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApprovalRequestedPayload {
    /// The approval.
    pub approval: Approval,
}

/// `ApprovalResolved`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApprovalResolvedPayload {
    /// Approval.
    pub approval_id: ApprovalId,
    /// Final status.
    pub status: ApprovalStatus,
    /// Who resolved it.
    pub resolved_by: Option<Actor>,
}

/// `ProviderStatusChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderStatusChangedPayload {
    /// Provider.
    pub provider_id: ProviderId,
    /// Health.
    pub health: ProviderHealth,
    /// Auth state.
    pub auth_state: AuthState,
    /// Detail (redacted).
    pub detail: Option<String>,
}

/// `ProviderRateLimited`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderRateLimitedPayload {
    /// Provider.
    pub provider_id: ProviderId,
    /// Model.
    pub model: Option<String>,
    /// Seconds until retry is allowed.
    pub retry_after_secs: Option<u64>,
}

/// `ProviderFallback` — explicit user-visible model switch (§7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderFallbackPayload {
    /// Previous model.
    pub from: ModelRef,
    /// New model.
    pub to: ModelRef,
    /// Why ("rate limited", "quota exceeded", …).
    pub reason: String,
}

/// `BudgetThreshold`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetThresholdPayload {
    /// Budget owner description ("project", "task", "provider", …).
    pub scope: String,
    /// Amount spent in USD.
    pub spent_usd: f64,
    /// Limit in USD.
    pub limit_usd: f64,
    /// spent / limit.
    pub fraction: f64,
}

/// `SnapshotCreated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotCreatedPayload {
    /// The snapshot.
    pub snapshot: WorkspaceSnapshot,
}

/// What a rollback restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RollbackScope {
    /// Only the changes of one task.
    Task,
    /// The whole workspace.
    Workspace,
}

/// `RollbackPerformed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackPerformedPayload {
    /// Snapshot restored.
    pub snapshot_id: SnapshotId,
    /// Scope.
    pub scope: RollbackScope,
    /// Files restored.
    pub files_restored: u32,
    /// Safety snapshot taken right before the rollback.
    pub pre_rollback_snapshot_id: Option<SnapshotId>,
}

/// `TerminalOutput`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalOutputPayload {
    /// Terminal session.
    pub terminal_id: TerminalId,
    /// UTF-8 (lossy) output chunk including ANSI sequences.
    pub data: String,
    /// Absolute byte offset of `data` in the session's output stream (for
    /// de-duplicating against `terminal.history`). Added in Phase 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub offset: Option<u64>,
}

/// `TerminalExited`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalExitedPayload {
    /// Terminal session.
    pub terminal_id: TerminalId,
    /// Exit code if known.
    pub exit_code: Option<i32>,
}

/// `NodeStatus`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeStatusPayload {
    /// Node snapshot.
    pub node: Node,
}

/// `NodeOffline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeOfflinePayload {
    /// Node.
    pub node_id: NodeId,
    /// Last heartbeat.
    pub last_seen_at: Option<Timestamp>,
}

/// `SyncCompleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncCompletedPayload {
    /// Backend name (`local-only`, `self-hosted`, `cloud`).
    pub backend: String,
    /// Items synced.
    pub items: u64,
    /// Bytes transferred.
    pub bytes: u64,
    /// Conflicts detected.
    pub conflicts: u32,
}

/// `DeploymentFinished`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeploymentFinishedPayload {
    /// Deployment.
    pub deployment: Deployment,
}

/// `MemoryUpdated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryUpdatedPayload {
    /// The memory item after the change.
    pub item: MemoryItem,
    /// True when the item was deleted.
    pub deleted: bool,
}

/// `DecisionRecorded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionRecordedPayload {
    /// The decision.
    pub decision: Decision,
}

/// Scope of an emergency stop (§52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "scope", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum EmergencyScope {
    /// Stop all AI activity.
    All,
    /// Stop one project.
    Project {
        /// Project.
        project_id: ProjectId,
    },
    /// Stop one agent.
    Agent {
        /// Agent.
        agent_id: AgentId,
    },
}

/// `EmergencyStop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmergencyStopPayload {
    /// Scope.
    pub scope: EmergencyScope,
    /// Agents stopped.
    pub agents_stopped: u32,
    /// Who triggered it.
    pub triggered_by: Actor,
}

/// `Notification`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotificationPayload {
    /// The notification.
    pub notification: Notification,
}

/// Log level for `Log` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LogLevel {
    /// Trace.
    Trace,
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warn.
    Warn,
    /// Error.
    Error,
}

/// `Log` — structured, already redacted log line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogPayload {
    /// Level.
    pub level: LogLevel,
    /// Source module/target.
    pub target: String,
    /// Message.
    pub message: String,
    /// Structured fields.
    pub fields: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Kinds
// ---------------------------------------------------------------------------

macro_rules! event_kinds {
    ($( $(#[$m:meta])* $variant:ident($payload:ty) ),* $(,)?) => {
        /// All event kinds. Adjacently tagged: `{"kind": "...", "payload": {...}}`.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
        #[serde(tag = "kind", content = "payload")]
        #[ts(export)]
        pub enum EventKind {
            $( $(#[$m])* $variant($payload), )*
        }

        impl EventKind {
            /// Names of all kinds, in declaration order.
            pub const NAMES: &'static [&'static str] = &[ $( stringify!($variant) ),* ];

            /// High-volume streaming kinds that are broadcast but **not** persisted
            /// in the event log (terminal output and message deltas have their own
            /// history stores). Their `seq` numbers therefore appear as gaps in
            /// `events.replay`; clients must not treat such gaps as data loss.
            #[must_use]
            pub fn is_ephemeral(&self) -> bool {
                matches!(self, EventKind::TerminalOutput(_) | EventKind::AgentMessage(_) | EventKind::ManagerMessage(_))
            }

            /// The `kind` string of this event.
            #[must_use]
            pub fn name(&self) -> &'static str {
                match self {
                    $( EventKind::$variant(_) => stringify!($variant), )*
                }
            }
        }
    };
}

event_kinds! {
    /// A project was created.
    ProjectCreated(ProjectPayload),
    /// A project was updated (including opened/archived).
    ProjectUpdated(ProjectPayload),
    /// A project was deleted.
    ProjectDeleted(ProjectDeletedPayload),
    /// A task was created.
    TaskCreated(TaskPayload),
    /// A task was updated.
    TaskUpdated(TaskPayload),
    /// A task changed status.
    TaskStatusChanged(TaskStatusChangedPayload),
    /// An agent was spawned.
    AgentSpawned(AgentSpawnedPayload),
    /// An agent started working.
    AgentStarted(AgentStartedPayload),
    /// An agent changed state.
    AgentStateChanged(AgentStateChangedPayload),
    /// An agent stopped.
    AgentStopped(AgentStoppedPayload),
    /// Streamed agent output delta.
    AgentMessage(MessageDeltaPayload),
    /// Tool call started.
    ToolCallStarted(ToolCallStartedPayload),
    /// Tool call finished.
    ToolCallFinished(ToolCallFinishedPayload),
    /// A file changed in a workspace.
    FileChanged(FileChangedPayload),
    /// A build started.
    BuildStarted(BuildStartedPayload),
    /// A build failed.
    BuildFailed(BuildFinishedPayload),
    /// A build succeeded.
    BuildSucceeded(BuildFinishedPayload),
    /// A test run finished.
    TestRunFinished(TestRunFinishedPayload),
    /// A review finished.
    ReviewFinished(ReviewFinishedPayload),
    /// An approval is requested.
    ApprovalRequested(ApprovalRequestedPayload),
    /// An approval was resolved.
    ApprovalResolved(ApprovalResolvedPayload),
    /// Provider health/auth changed.
    ProviderStatusChanged(ProviderStatusChangedPayload),
    /// Provider rate limited.
    ProviderRateLimited(ProviderRateLimitedPayload),
    /// Model fallback happened.
    ProviderFallback(ProviderFallbackPayload),
    /// A budget threshold was crossed.
    BudgetThreshold(BudgetThresholdPayload),
    /// A snapshot was created.
    SnapshotCreated(SnapshotCreatedPayload),
    /// A rollback was performed.
    RollbackPerformed(RollbackPerformedPayload),
    /// Terminal output chunk.
    TerminalOutput(TerminalOutputPayload),
    /// Terminal exited.
    TerminalExited(TerminalExitedPayload),
    /// Node status heartbeat.
    NodeStatus(NodeStatusPayload),
    /// Node went offline.
    NodeOffline(NodeOfflinePayload),
    /// Sync finished.
    SyncCompleted(SyncCompletedPayload),
    /// Deployment finished.
    DeploymentFinished(DeploymentFinishedPayload),
    /// Memory changed.
    MemoryUpdated(MemoryUpdatedPayload),
    /// Decision recorded.
    DecisionRecorded(DecisionRecordedPayload),
    /// Streamed Manager output delta.
    ManagerMessage(MessageDeltaPayload),
    /// Emergency stop executed.
    EmergencyStop(EmergencyStopPayload),
    /// User notification.
    Notification(NotificationPayload),
    /// Structured log line.
    Log(LogPayload),
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// A published event (persisted in the event log, streamed to subscribers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Event {
    /// Monotonic sequence number assigned by the event bus (starts at 1).
    pub seq: u64,
    /// Unique id.
    pub id: EventId,
    /// Publish time.
    pub ts: Timestamp,
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Task scope.
    pub task_id: Option<TaskId>,
    /// Agent scope.
    pub agent_id: Option<AgentId>,
    /// Node scope.
    pub node_id: Option<NodeId>,
    /// Correlation id (§50).
    pub correlation_id: Option<CorrelationId>,
    /// Kind + payload.
    #[serde(flatten)]
    pub kind: EventKind,
}

/// An event before the bus assigned `seq`, `id` and `ts`.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    /// Kind + payload.
    pub kind: EventKind,
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Task scope.
    pub task_id: Option<TaskId>,
    /// Agent scope.
    pub agent_id: Option<AgentId>,
    /// Node scope.
    pub node_id: Option<NodeId>,
    /// Correlation id; when `None` the bus uses the ambient one (if any).
    pub correlation_id: Option<CorrelationId>,
}

impl NewEvent {
    /// New event with scopes inferred from the payload where obvious.
    #[must_use]
    pub fn new(kind: EventKind) -> Self {
        let mut ev =
            Self { kind, project_id: None, task_id: None, agent_id: None, node_id: None, correlation_id: None };
        ev.infer_scopes();
        ev
    }

    /// Sets the project scope.
    #[must_use]
    pub fn project(mut self, id: ProjectId) -> Self {
        self.project_id = Some(id);
        self
    }

    /// Sets the task scope.
    #[must_use]
    pub fn task(mut self, id: TaskId) -> Self {
        self.task_id = Some(id);
        self
    }

    /// Sets the agent scope.
    #[must_use]
    pub fn agent(mut self, id: AgentId) -> Self {
        self.agent_id = Some(id);
        self
    }

    /// Sets the node scope.
    #[must_use]
    pub fn node(mut self, id: NodeId) -> Self {
        self.node_id = Some(id);
        self
    }

    /// Sets the correlation id.
    #[must_use]
    pub fn correlation(mut self, id: CorrelationId) -> Self {
        self.correlation_id = Some(id);
        self
    }

    /// Fills scope ids that are unambiguous from the payload.
    fn infer_scopes(&mut self) {
        match &self.kind {
            EventKind::ProjectCreated(p) | EventKind::ProjectUpdated(p) => self.project_id = Some(p.project.id),
            EventKind::ProjectDeleted(p) => self.project_id = Some(p.project_id),
            EventKind::TaskCreated(t) | EventKind::TaskUpdated(t) => {
                self.project_id = Some(t.task.project_id);
                self.task_id = Some(t.task.id);
            }
            EventKind::TaskStatusChanged(t) => self.task_id = Some(t.task_id),
            EventKind::AgentSpawned(a) => {
                self.agent_id = Some(a.agent.id);
                self.project_id = Some(a.agent.project_id);
                self.task_id = a.agent.task_id;
            }
            EventKind::AgentStarted(a) => {
                self.agent_id = Some(a.agent_id);
                self.task_id = a.task_id;
            }
            EventKind::AgentStateChanged(a) => self.agent_id = Some(a.agent_id),
            EventKind::AgentStopped(a) => self.agent_id = Some(a.agent_id),
            EventKind::AgentMessage(m) => self.agent_id = m.agent_id,
            EventKind::ToolCallStarted(t) => {
                self.agent_id = Some(t.tool_call.agent_id);
                self.task_id = t.tool_call.task_id;
            }
            EventKind::ApprovalRequested(a) => {
                self.project_id = a.approval.project_id;
                self.task_id = a.approval.task_id;
                self.agent_id = a.approval.agent_id;
            }
            EventKind::SnapshotCreated(s) => {
                self.project_id = Some(s.snapshot.project_id);
                self.task_id = s.snapshot.task_id;
            }
            EventKind::NodeStatus(n) => self.node_id = Some(n.node.id),
            EventKind::NodeOffline(n) => self.node_id = Some(n.node_id),
            EventKind::DeploymentFinished(d) => {
                self.project_id = Some(d.deployment.project_id);
                self.task_id = d.deployment.task_id;
            }
            EventKind::DecisionRecorded(d) => self.project_id = Some(d.decision.project_id),
            EventKind::Notification(n) => {
                self.project_id = n.notification.project_id;
                self.task_id = n.notification.task_id;
                self.agent_id = n.notification.agent_id;
            }
            EventKind::EmergencyStop(e) => match e.scope {
                EmergencyScope::Project { project_id } => self.project_id = Some(project_id),
                EmergencyScope::Agent { agent_id } => self.agent_id = Some(agent_id),
                EmergencyScope::All => {}
            },
            _ => {}
        }
    }

    /// Turns the draft into a published event.
    #[must_use]
    pub fn into_event(self, seq: u64, ts: Timestamp) -> Event {
        Event {
            seq,
            id: EventId::new(),
            ts,
            project_id: self.project_id,
            task_id: self.task_id,
            agent_id: self.agent_id,
            node_id: self.node_id,
            correlation_id: self.correlation_id,
            kind: self.kind,
        }
    }
}

impl Event {
    /// The `kind` string.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        self.kind.name()
    }

    /// Terminal id for terminal events.
    #[must_use]
    pub fn terminal_id(&self) -> Option<TerminalId> {
        match &self.kind {
            EventKind::TerminalOutput(p) => Some(p.terminal_id),
            EventKind::TerminalExited(p) => Some(p.terminal_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::now;

    #[test]
    fn envelope_is_flat_with_kind_and_payload() {
        let project = Project::new("Demo", now());
        let ev =
            NewEvent::new(EventKind::ProjectCreated(ProjectPayload { project: project.clone() })).into_event(7, now());
        assert_eq!(ev.project_id, Some(project.id));
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["seq"], 7);
        assert_eq!(v["kind"], "ProjectCreated");
        assert_eq!(v["payload"]["project"]["name"], "Demo");
        assert_eq!(v["projectId"], project.id.to_string());
        let back: Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn names_match_serde_tags() {
        let ev = EventKind::Log(LogPayload {
            level: LogLevel::Info,
            target: "t".into(),
            message: "m".into(),
            fields: serde_json::Value::Null,
        });
        assert_eq!(serde_json::to_value(&ev).unwrap()["kind"], ev.name());
        assert!(EventKind::NAMES.contains(&"EmergencyStop"));
    }

    #[test]
    fn contract_minimum_kinds_present() {
        let required = [
            "ProjectCreated",
            "ProjectUpdated",
            "ProjectDeleted",
            "TaskCreated",
            "TaskUpdated",
            "TaskStatusChanged",
            "AgentSpawned",
            "AgentStarted",
            "AgentStateChanged",
            "AgentStopped",
            "AgentMessage",
            "ToolCallStarted",
            "ToolCallFinished",
            "FileChanged",
            "BuildStarted",
            "BuildFailed",
            "BuildSucceeded",
            "TestRunFinished",
            "ReviewFinished",
            "ApprovalRequested",
            "ApprovalResolved",
            "ProviderStatusChanged",
            "ProviderRateLimited",
            "ProviderFallback",
            "BudgetThreshold",
            "SnapshotCreated",
            "RollbackPerformed",
            "TerminalOutput",
            "TerminalExited",
            "NodeStatus",
            "NodeOffline",
            "SyncCompleted",
            "DeploymentFinished",
            "MemoryUpdated",
            "DecisionRecorded",
            "ManagerMessage",
            "EmergencyStop",
            "Notification",
            "Log",
        ];
        for k in required {
            assert!(EventKind::NAMES.contains(&k), "missing event kind {k}");
        }
    }
}

//! User notifications (§33, §37).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::Timestamp;
use crate::ids::{AgentId, NotificationId, ProjectId, TaskId};

/// Severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NotificationLevel {
    /// Informational.
    Info,
    /// Success.
    Success,
    /// Warning.
    Warning,
    /// Error.
    Error,
    /// Critical (security, data loss risk).
    Critical,
}

/// Category (drives push routing and sounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NotificationCategory {
    /// Task completed.
    TaskCompleted,
    /// Task blocked.
    TaskBlocked,
    /// Approval needed.
    ApprovalRequired,
    /// Build failed.
    BuildFailed,
    /// Tests failed.
    TestFailed,
    /// Security issue found.
    SecurityIssue,
    /// Node offline.
    NodeOffline,
    /// Deployment finished.
    Deployment,
    /// Budget threshold.
    BudgetThreshold,
    /// System message.
    System,
}

/// Action button attached to a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotificationAction {
    /// Button label.
    pub label: String,
    /// Client command id to run (e.g. `approvals.open:<id>`).
    pub command: String,
}

/// A notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notification {
    /// Id.
    pub id: NotificationId,
    /// Time.
    pub ts: Timestamp,
    /// Level.
    pub level: NotificationLevel,
    /// Category.
    pub category: NotificationCategory,
    /// Title.
    pub title: String,
    /// Body.
    pub body: String,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Agent.
    pub agent_id: Option<AgentId>,
    /// Read flag.
    pub read: bool,
    /// Optional actions.
    pub actions: Vec<NotificationAction>,
}

//! Event delivery to plugins (`subscribes`, ADR-0034 §12).
//!
//! * Only kinds in [`SUBSCRIBABLE`] may be subscribed (non-sensitive kinds:
//!   no terminal / LSP / chat streams, no remote-session or device auth, no
//!   settings, no plugin lifecycle or `Log` events, which would loop).
//! * Delivery needs a `Read` grant with scope [`event_scope`]
//!   (`event/<Kind>`) requested in the manifest's `permissions`; without it
//!   the event is not delivered (counted as `denied`).
//! * Each plugin has a bounded queue ([`DEFAULT_QUEUE_CAPACITY`]); a full queue
//!   drops the oldest event and counts it. The bus subscriber only pushes into
//!   queues and never waits on plugin code.
//! * One worker per plugin drains its queue, calling the optional guest
//!   export `orkestr_on_event` with `{"event": <redacted envelope>}` under the
//!   manifest limits (fuel + timeout per delivery). Kill switch, disable and
//!   safe mode clear the queue and stop delivery; running deliveries are
//!   interrupted like tool calls.
//! * Each drained batch writes one audit summary (`plugins.events.deliver`).

use std::collections::VecDeque;
use std::sync::Arc;

use orkestr_protocol::Event;

/// Event kinds plugins may subscribe to.
pub const SUBSCRIBABLE: &[&str] = &[
    "ProjectCreated",
    "ProjectUpdated",
    "ProjectDeleted",
    "TaskCreated",
    "TaskUpdated",
    "TaskStatusChanged",
    "TaskDeleted",
    "AgentSpawned",
    "AgentStarted",
    "AgentStateChanged",
    "AgentStopped",
    "BuildStarted",
    "BuildFailed",
    "BuildSucceeded",
    "TestRunFinished",
    "ReviewFinished",
    "ApprovalRequested",
    "ApprovalResolved",
    "ProviderStatusChanged",
    "ProviderRateLimited",
    "BudgetThreshold",
    "SnapshotCreated",
    "RollbackPerformed",
    "NodeOffline",
    "SyncCompleted",
    "DeploymentStarted",
    "DeploymentFinished",
    "DecisionRecorded",
    "EmergencyStop",
    "MergeProposalUpdated",
    "MergeRiskDetected",
    "PipelineUpdated",
    "PipelineStageChanged",
    "HealthCheckChanged",
    "AlertRaised",
    "AlertResolved",
    "IncidentOpened",
    "IncidentStageChanged",
    "DashboardChanged",
];

/// Default per-plugin queue capacity.
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;

/// Whether plugins may subscribe to `kind`.
#[must_use]
pub fn subscribable(kind: &str) -> bool {
    SUBSCRIBABLE.contains(&kind)
}

/// Grant scope (capability `Read`) that allows delivery of `kind`.
#[must_use]
pub fn event_scope(kind: &str) -> String {
    format!("event/{kind}")
}

/// Delivery counters of one plugin (since the core started).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventStats {
    /// Delivered (guest returned).
    pub delivered: u64,
    /// Guest failed (trap / error result).
    pub failed: u64,
    /// Not delivered: the `event/<Kind>` grant is missing or denied.
    pub denied: u64,
    /// Dropped: queue full (oldest dropped) or the bus subscriber lagged.
    pub dropped: u64,
    /// Currently queued.
    pub queued: u64,
}

/// Bounded drop-oldest queue of one plugin.
#[derive(Debug, Default)]
pub(crate) struct EventQueue {
    pub(crate) items: VecDeque<Arc<Event>>,
    pub(crate) stats: EventStats,
    pub(crate) worker: bool,
}

impl EventQueue {
    /// Pushes `ev`, dropping the oldest event when `cap` is reached.
    pub(crate) fn push(&mut self, ev: Arc<Event>, cap: usize) {
        while self.items.len() >= cap.max(1) {
            self.items.pop_front();
            self.stats.dropped += 1;
        }
        self.items.push_back(ev);
    }

    /// Drops every queued event (kill / disable / safe mode); not counted as dropped.
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    pub(crate) fn snapshot(&self) -> EventStats {
        EventStats { queued: self.items.len() as u64, ..self.stats }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::event::{EventKind, LogLevel, LogPayload};
    use orkestr_protocol::NewEvent;

    fn ev(i: u64) -> Arc<Event> {
        Arc::new(
            NewEvent::new(EventKind::Log(LogPayload {
                level: LogLevel::Info,
                target: "t".into(),
                message: i.to_string(),
                fields: serde_json::Value::Null,
            }))
            .into_event(i, orkestr_domain::now()),
        )
    }

    #[test]
    fn queue_drops_oldest() {
        let mut q = EventQueue::default();
        for i in 0..10 {
            q.push(ev(i), 4);
        }
        let s = q.snapshot();
        assert_eq!((s.queued, s.dropped), (4, 6));
        assert_eq!(q.items.front().unwrap().seq, 6);
        q.clear();
        assert_eq!(q.snapshot().queued, 0);
    }

    #[test]
    fn sensitive_kinds_are_not_subscribable() {
        assert!(subscribable("TaskCreated"));
        for k in ["TerminalOutput", "LspMessage", "ClaudeCodeEvent", "Log", "PluginCallDenied", "SettingsChanged"] {
            assert!(!subscribable(k), "{k}");
        }
        for k in SUBSCRIBABLE {
            assert!(orkestr_protocol::event::EventKind::NAMES.contains(k), "{k} is not an event kind");
        }
        assert_eq!(event_scope("TaskCreated"), "event/TaskCreated");
    }
}

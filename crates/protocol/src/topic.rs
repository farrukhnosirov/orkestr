//! Subscription topics (contract §4).
//!
//! Topics: `global`, `project:<id>`, `task:<id>`, `agent:<id>`,
//! `terminal:<id>`, `node:<id>`, plus the wildcard `*` (every event; intended
//! for the Logs panel and diagnostics).
//!
//! Routing rules ([`event_topics`]):
//! * every event is published on each scope topic it carries
//!   (`project:`, `task:`, `agent:`, `node:`, `terminal:`);
//! * events **without** a project scope, and project lifecycle events
//!   (`ProjectCreated/Updated/Deleted`, so project lists stay fresh), are also
//!   published on `global` — except node heartbeats (`NodeStatus` with a
//!   node id), which only go to `node:<id>`;
//! * `*` matches everything.

use crate::event::{Event, EventKind};

/// Wildcard topic matching every event.
pub const ALL: &str = "*";
/// System-wide events.
pub const GLOBAL: &str = "global";

/// `project:<id>`.
#[must_use]
pub fn project(id: impl std::fmt::Display) -> String {
    format!("project:{id}")
}
/// `task:<id>`.
#[must_use]
pub fn task(id: impl std::fmt::Display) -> String {
    format!("task:{id}")
}
/// `agent:<id>`.
#[must_use]
pub fn agent(id: impl std::fmt::Display) -> String {
    format!("agent:{id}")
}
/// `terminal:<id>`.
#[must_use]
pub fn terminal(id: impl std::fmt::Display) -> String {
    format!("terminal:{id}")
}
/// `node:<id>`.
#[must_use]
pub fn node(id: impl std::fmt::Display) -> String {
    format!("node:{id}")
}

/// Validates a topic string sent by a client.
#[must_use]
pub fn is_valid(topic: &str) -> bool {
    if topic == ALL || topic == GLOBAL {
        return true;
    }
    let Some((prefix, id)) = topic.split_once(':') else {
        return false;
    };
    matches!(prefix, "project" | "task" | "agent" | "terminal" | "node") && uuid::Uuid::parse_str(id).is_ok()
}

/// Topics an event is published on (see module docs).
#[must_use]
pub fn event_topics(event: &Event) -> Vec<String> {
    let mut topics = Vec::with_capacity(4);
    // High-volume terminal output goes only to its terminal topic (and `*`).
    if let EventKind::TerminalOutput(p) = &event.kind {
        topics.push(terminal(p.terminal_id));
        return topics;
    }
    let lifecycle = matches!(
        event.kind,
        EventKind::ProjectCreated(_) | EventKind::ProjectUpdated(_) | EventKind::ProjectDeleted(_)
    );
    // Node heartbeats (every 2 s) only go to `node:<id>`, never `global`.
    let heartbeat = matches!(event.kind, EventKind::NodeStatus(_)) && event.node_id.is_some();
    if (event.project_id.is_none() && !heartbeat) || lifecycle {
        topics.push(GLOBAL.to_owned());
    }
    if let Some(id) = event.project_id {
        topics.push(project(id));
    }
    if let Some(id) = event.task_id {
        topics.push(task(id));
    }
    if let Some(id) = event.agent_id {
        topics.push(agent(id));
    }
    if let Some(id) = event.node_id {
        topics.push(node(id));
    }
    if let Some(id) = event.terminal_id() {
        topics.push(terminal(id));
    }
    topics
}

/// Whether an event matches any of the subscribed topics.
#[must_use]
pub fn matches<S: AsRef<str>>(subscribed: &[S], event: &Event) -> bool {
    if subscribed.iter().any(|t| t.as_ref() == ALL) {
        return true;
    }
    let topics = event_topics(event);
    subscribed.iter().any(|s| topics.iter().any(|t| t == s.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{LogLevel, LogPayload, NewEvent, ProjectDeletedPayload, TaskStatusChangedPayload};
    use orkestr_domain::{now, ProjectId, TaskId, TaskStatus};

    fn log() -> EventKind {
        EventKind::Log(LogPayload {
            level: LogLevel::Info,
            target: "x".into(),
            message: "y".into(),
            fields: serde_json::Value::Null,
        })
    }

    #[test]
    fn global_event_routes_to_global_only() {
        let ev = NewEvent::new(log()).into_event(1, now());
        assert_eq!(event_topics(&ev), vec!["global".to_string()]);
        assert!(matches(&["global"], &ev));
        assert!(matches(&["*"], &ev));
        assert!(!matches(&[project(ProjectId::new())], &ev));
    }

    #[test]
    fn terminal_output_routes_to_terminal_topic_only() {
        use crate::event::TerminalOutputPayload;
        use orkestr_domain::TerminalId;
        let tid = TerminalId::new();
        let ev = NewEvent::new(EventKind::TerminalOutput(TerminalOutputPayload {
            terminal_id: tid,
            data: "x".into(),
            offset: Some(0),
        }))
        .project(ProjectId::new())
        .into_event(3, now());
        assert_eq!(event_topics(&ev), vec![terminal(tid)]);
        assert!(!matches(&["global"], &ev));
        assert!(matches(&["*"], &ev));
    }

    #[test]
    fn project_scoped_event_skips_global() {
        let pid = ProjectId::new();
        let tid = TaskId::new();
        let ev = NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload {
            task_id: tid,
            from: TaskStatus::Ready,
            to: TaskStatus::Running,
            reason: None,
        }))
        .project(pid)
        .into_event(2, now());
        let t = event_topics(&ev);
        assert!(!t.contains(&GLOBAL.to_string()));
        assert!(t.contains(&project(pid)));
        assert!(t.contains(&task(tid)));
        assert!(matches(&[task(tid)], &ev));
    }

    #[test]
    fn project_lifecycle_also_global() {
        let pid = ProjectId::new();
        let ev =
            NewEvent::new(EventKind::ProjectDeleted(ProjectDeletedPayload { project_id: pid })).into_event(3, now());
        let t = event_topics(&ev);
        assert!(t.contains(&GLOBAL.to_string()) && t.contains(&project(pid)));
    }

    #[test]
    fn node_heartbeat_is_node_topic_only() {
        let node = orkestr_domain::Node {
            id: orkestr_domain::NodeId::new(),
            name: "n".into(),
            hostname: "n".into(),
            os: orkestr_domain::OsKind::Windows,
            os_version: "11".into(),
            arch: orkestr_domain::Arch::X64,
            core_version: "0.1.0".into(),
            status: orkestr_domain::NodeStatus::Online,
            resources: orkestr_domain::NodeResources {
                cpu_cores: 1,
                cpu_usage_percent: 0.0,
                memory_total_bytes: 1,
                memory_used_bytes: 0,
                disk_total_bytes: 1,
                disk_free_bytes: 1,
                gpus: Vec::new(),
            },
            tools: Vec::new(),
            local_models: Vec::new(),
            is_local: true,
            last_heartbeat_at: None,
        };
        let id = node.id;
        let ev = NewEvent::new(EventKind::NodeStatus(crate::event::NodeStatusPayload { node, sample: None }))
            .into_event(4, now());
        assert_eq!(event_topics(&ev), vec![super::node(id)]);
        assert!(ev.kind.is_ephemeral());
    }

    #[test]
    fn topic_validation() {
        assert!(is_valid("*"));
        assert!(is_valid("global"));
        assert!(is_valid(&project(ProjectId::new())));
        assert!(!is_valid("project:nope"));
        assert!(!is_valid("bogus:00000000-0000-0000-0000-000000000000"));
        assert!(!is_valid("projects"));
    }
}

//! Crash recovery (§62.13, contract §5).
//!
//! A `session.lock` marker (`{pid, startedAt}`) is written at start and removed
//! on clean shutdown. If a marker of a dead process is found at start, the
//! previous session crashed: every registered [`RecoveryHook`] runs (e.g. the
//! task engine turns `Running/Waiting/Testing/Review` tasks into
//! `Interrupted`) and a `Log` event records what happened.

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_platform::fs::write_private_file;
use orkestr_platform::{Paths, ProcessControl};
use orkestr_protocol::event::{EventKind, LogLevel, LogPayload};
use orkestr_protocol::NewEvent;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::services::Services;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMarker {
    pid: u32,
    started_at: String,
}

/// What [`begin_session`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviousSession {
    /// The previous session did not shut down cleanly.
    pub crashed: bool,
    /// Its pid, if known.
    pub pid: Option<u32>,
    /// Its start time, if known.
    pub started_at: Option<String>,
}

/// Records the start of a session; detects an unclean previous one.
pub fn begin_session(paths: &Paths, process: &dyn ProcessControl) -> Result<PreviousSession, CoreError> {
    let marker_path = paths.session_marker();
    let mut previous = PreviousSession::default();
    if let Ok(bytes) = std::fs::read(&marker_path) {
        match serde_json::from_slice::<SessionMarker>(&bytes) {
            Ok(m) if m.pid != std::process::id() && process.is_alive(m.pid) => {
                // A live process owns the marker but did not answer discovery:
                // be conservative and refuse to share the database.
                return Err(CoreError::AlreadyRunning { url: "<unknown>".into(), pid: m.pid });
            }
            Ok(m) => {
                previous = PreviousSession { crashed: true, pid: Some(m.pid), started_at: Some(m.started_at) };
            }
            Err(_) => previous.crashed = true,
        }
    }
    let marker = SessionMarker { pid: std::process::id(), started_at: chrono::Utc::now().to_rfc3339() };
    let json = serde_json::to_vec(&marker).map_err(std::io::Error::other)?;
    write_private_file(&marker_path, &json)?;
    Ok(previous)
}

/// Marks the session as cleanly finished.
pub fn end_session(paths: &Paths) {
    let path = paths.session_marker();
    let ours = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<SessionMarker>(&b).ok())
        .is_some_and(|m| m.pid == std::process::id());
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

/// A recovery step contributed by an engine.
#[async_trait]
pub trait RecoveryHook: Send + Sync {
    /// Name for logs.
    fn name(&self) -> &'static str;
    /// Repairs state after a crash; returns a short human summary.
    async fn recover(&self, services: &Services) -> Result<String, String>;
}

/// Task engine: `Running/Waiting/Testing/Review` → `Interrupted` through the
/// privileged `SystemRecovery` edges (contract §5, ADR-0022); emits
/// `TaskStatusChanged` + `TaskUpdated` per task.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskRecoveryHook;

#[async_trait]
impl RecoveryHook for TaskRecoveryHook {
    fn name(&self) -> &'static str {
        "task-engine"
    }

    async fn recover(&self, services: &Services) -> Result<String, String> {
        let report = services.tasks.recover_interrupted("crash_recovery").await.map_err(|e| e.to_string())?;
        Ok(format!("{} task(s) marked Interrupted", report.interrupted.len()))
    }
}

/// Hooks run after a crash, in order. Engines append theirs as they land
/// (Phase 1: task engine → Interrupted; agent runtime → Stopped; terminal → Exited).
#[must_use]
pub fn default_hooks() -> Vec<Arc<dyn RecoveryHook>> {
    vec![Arc::new(TaskRecoveryHook)]
}

/// Runs recovery hooks if the previous session crashed and records the outcome.
pub async fn run(services: &Services, previous: &PreviousSession, hooks: &[Arc<dyn RecoveryHook>]) {
    if !previous.crashed {
        return;
    }
    tracing::warn!(pid = ?previous.pid, started_at = ?previous.started_at, "previous core session ended uncleanly; running recovery");
    let mut results = Vec::new();
    for hook in hooks {
        let outcome = hook.recover(services).await;
        match &outcome {
            Ok(summary) => tracing::info!(hook = hook.name(), %summary, "recovery hook finished"),
            Err(error) => tracing::error!(hook = hook.name(), %error, "recovery hook failed"),
        }
        results.push(serde_json::json!({ "hook": hook.name(), "ok": outcome.is_ok(), "summary": match outcome { Ok(s) | Err(s) => s } }));
    }
    let event = NewEvent::new(EventKind::Log(LogPayload {
        level: LogLevel::Warn,
        target: "orkestr_core::recovery".into(),
        message: "Recovered from an unclean shutdown of the previous session".into(),
        fields: serde_json::json!({ "previousPid": previous.pid, "previousStartedAt": previous.started_at, "hooks": results }),
    }));
    if let Err(e) = services.events.publish(event).await {
        tracing::error!(error = %e, "failed to record recovery event");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_platform::NativeProcessControl;

    #[test]
    fn detects_crashed_session() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(dir.path());
        let first = begin_session(&paths, &NativeProcessControl).unwrap();
        assert!(!first.crashed);
        // Simulate a crash of a different (dead) process.
        let stale = SessionMarker { pid: u32::MAX - 11, started_at: "2026-01-01T00:00:00Z".into() };
        std::fs::write(paths.session_marker(), serde_json::to_vec(&stale).unwrap()).unwrap();
        let second = begin_session(&paths, &NativeProcessControl).unwrap();
        assert!(second.crashed);
        assert_eq!(second.pid, Some(u32::MAX - 11));
        end_session(&paths);
        assert!(!paths.session_marker().exists());
        let third = begin_session(&paths, &NativeProcessControl).unwrap();
        assert!(!third.crashed);
    }
}

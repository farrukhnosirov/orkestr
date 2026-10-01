//! The task state machine (contract §5). Anything not listed is an
//! [`InvalidTransition`].
//!
//! ```text
//! Planned    → Ready | Blocked | Cancelled
//! Ready      → Running | Blocked | Cancelled | Planned
//! Running    → Waiting | Blocked | Review | Testing | Completed | Failed | Cancelled | Interrupted
//! Waiting    → Running | Blocked | Cancelled | Failed
//! Blocked    → Ready | Cancelled | Failed
//! Review     → Testing | Running | Completed | Failed | Cancelled      (Running = changes requested)
//! Testing    → Review | Running | Completed | Failed | Cancelled       (Running = fix loop)
//! Failed     → Ready | Recovering | Cancelled                         (retry)
//! Interrupted→ Recovering | Cancelled
//! Recovering → Ready | Running | Failed | Cancelled
//! Completed, Cancelled: terminal
//! ```

use orkestr_domain::{Task, TaskStatus, Timestamp};

use TaskStatus::{
    Blocked, Cancelled, Completed, Failed, Interrupted, Planned, Ready, Recovering, Review, Running, Testing, Waiting,
};

/// Error for an illegal transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid task transition {from:?} → {to:?}")]
pub struct InvalidTransition {
    /// Current status.
    pub from: TaskStatus,
    /// Requested status.
    pub to: TaskStatus,
}

/// Statuses reachable from `from`.
#[must_use]
pub fn allowed_transitions(from: TaskStatus) -> &'static [TaskStatus] {
    match from {
        Planned => &[Ready, Blocked, Cancelled],
        Ready => &[Running, Blocked, Cancelled, Planned],
        Running => &[Waiting, Blocked, Review, Testing, Completed, Failed, Cancelled, Interrupted],
        Waiting => &[Running, Blocked, Cancelled, Failed],
        Blocked => &[Ready, Cancelled, Failed],
        Review => &[Testing, Running, Completed, Failed, Cancelled],
        Testing => &[Review, Running, Completed, Failed, Cancelled],
        Failed => &[Ready, Recovering, Cancelled],
        Interrupted => &[Recovering, Cancelled],
        Recovering => &[Ready, Running, Failed, Cancelled],
        Completed | Cancelled => &[],
    }
}

/// Whether `from → to` is allowed.
#[must_use]
pub fn can_transition(from: TaskStatus, to: TaskStatus) -> bool {
    allowed_transitions(from).contains(&to)
}

/// Applies a transition to a task, maintaining timestamps and attempt count.
pub fn transition(task: &mut Task, to: TaskStatus, now: Timestamp) -> Result<TaskStatus, InvalidTransition> {
    let from = task.status;
    if !can_transition(from, to) {
        return Err(InvalidTransition { from, to });
    }
    task.status = to;
    task.updated_at = now;
    if to == Running {
        if task.started_at.is_none() {
            task.started_at = Some(now);
        }
        if matches!(from, Ready | Recovering) {
            task.attempts += 1;
        }
    }
    if to.is_terminal() {
        task.finished_at = Some(now);
    }
    Ok(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{now, ProjectId};

    /// The contract table, written independently as (from, to) pairs.
    #[rustfmt::skip]
    const ALLOWED: &[(TaskStatus, TaskStatus)] = &[
        (Planned, Ready), (Planned, Blocked), (Planned, Cancelled),
        (Ready, Running), (Ready, Blocked), (Ready, Cancelled), (Ready, Planned),
        (Running, Waiting), (Running, Blocked), (Running, Review), (Running, Testing), (Running, Completed),
        (Running, Failed), (Running, Cancelled), (Running, Interrupted),
        (Waiting, Running), (Waiting, Blocked), (Waiting, Cancelled), (Waiting, Failed),
        (Blocked, Ready), (Blocked, Cancelled), (Blocked, Failed),
        (Review, Testing), (Review, Running), (Review, Completed), (Review, Failed), (Review, Cancelled),
        (Testing, Review), (Testing, Running), (Testing, Completed), (Testing, Failed), (Testing, Cancelled),
        (Failed, Ready), (Failed, Recovering), (Failed, Cancelled),
        (Interrupted, Recovering), (Interrupted, Cancelled),
        (Recovering, Ready), (Recovering, Running), (Recovering, Failed), (Recovering, Cancelled),
    ];

    #[test]
    fn exhaustive_matrix_matches_contract() {
        for from in TaskStatus::ALL {
            for to in TaskStatus::ALL {
                let expected = ALLOWED.contains(&(from, to));
                assert_eq!(can_transition(from, to), expected, "{from:?} → {to:?}");
            }
        }
    }

    #[test]
    fn terminal_states_have_no_exits() {
        for s in TaskStatus::ALL {
            assert_eq!(allowed_transitions(s).is_empty(), s.is_terminal(), "{s:?}");
        }
    }

    #[test]
    fn crash_states_can_be_interrupted_or_recovered() {
        // Running can be interrupted directly; Waiting/Review/Testing are moved
        // to Interrupted by crash recovery (a forced, audited reset), not by
        // a normal transition — they must still be recoverable afterwards.
        assert!(can_transition(Running, Interrupted));
        assert!(can_transition(Interrupted, Recovering));
        assert!(can_transition(Recovering, Running));
    }

    #[test]
    fn transition_updates_task() {
        let t0 = now();
        let mut task = Task::new(ProjectId::new(), "x", t0);
        transition(&mut task, Ready, t0).unwrap();
        transition(&mut task, Running, t0).unwrap();
        assert_eq!(task.attempts, 1);
        assert!(task.started_at.is_some());
        let err = transition(&mut task, Planned, t0).unwrap_err();
        assert_eq!(err, InvalidTransition { from: Running, to: Planned });
        assert_eq!(task.status, Running, "failed transition leaves the task unchanged");
        transition(&mut task, Failed, t0).unwrap();
        transition(&mut task, Ready, t0).unwrap();
        transition(&mut task, Running, t0).unwrap();
        assert_eq!(task.attempts, 2);
        transition(&mut task, Completed, t0).unwrap();
        assert!(task.finished_at.is_some());
        assert!(transition(&mut task, Running, t0).is_err());
    }
}

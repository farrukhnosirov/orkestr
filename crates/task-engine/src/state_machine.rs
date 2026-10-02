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
//!
//! Privileged system edges (ADR-0022, CA-2): `{Waiting, Review, Testing} →
//! Interrupted` are allowed **only** for [`TaskActor::SystemRecovery`]
//! (crash/shutdown recovery). They are never reachable through the normal
//! table, i.e. not through `tasks.transition` from a client.

use orkestr_domain::{Task, TaskActor, TaskStatus, Timestamp};

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

/// Whether `from → to` is one of the privileged crash-recovery edges that are
/// not part of the normal table.
#[must_use]
pub fn is_privileged_edge(from: TaskStatus, to: TaskStatus) -> bool {
    to == Interrupted && matches!(from, Waiting | Review | Testing)
}

/// Whether `actor` may move a task `from → to`: the normal table for every
/// actor, plus the privileged edges for [`TaskActor::SystemRecovery`].
#[must_use]
pub fn can_transition_as(from: TaskStatus, to: TaskStatus, actor: TaskActor) -> bool {
    can_transition(from, to) || (actor == TaskActor::SystemRecovery && is_privileged_edge(from, to))
}

/// Outcome of an applied transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// Previous status.
    pub from: TaskStatus,
    /// Whether a privileged edge was used.
    pub forced: bool,
    /// Whether a new execution attempt starts (`Ready|Recovering → Running`).
    pub new_attempt: bool,
}

/// Applies a transition to a task, maintaining timestamps and attempt count.
pub fn transition(task: &mut Task, to: TaskStatus, now: Timestamp) -> Result<TaskStatus, InvalidTransition> {
    transition_as(task, to, TaskActor::User, now).map(|a| a.from)
}

/// Applies a transition requested by `actor` (see [`can_transition_as`]).
///
/// Maintains `updated_at`, `started_at` (first `Running`), `attempts`
/// (`Ready|Recovering → Running`), `ready_since` (set on entering `Ready`,
/// cleared on leaving it) and `finished_at` (terminal states).
pub fn transition_as(
    task: &mut Task,
    to: TaskStatus,
    actor: TaskActor,
    now: Timestamp,
) -> Result<Applied, InvalidTransition> {
    let from = task.status;
    if !can_transition_as(from, to, actor) {
        return Err(InvalidTransition { from, to });
    }
    let forced = !can_transition(from, to);
    let new_attempt = to == Running && matches!(from, Ready | Recovering);
    task.status = to;
    task.ready_since = if to == Ready { Some(now) } else { None };
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
    Ok(Applied { from, forced, new_attempt })
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

    fn actors() -> [TaskActor; 5] {
        [
            TaskActor::User,
            TaskActor::Agent(orkestr_domain::AgentId::new()),
            TaskActor::Manager,
            TaskActor::System,
            TaskActor::SystemRecovery,
        ]
    }

    /// All 144 (from, to) pairs for every actor: only `SystemRecovery` gains
    /// exactly the three privileged edges; nothing else differs from the table.
    #[test]
    fn exhaustive_matrix_per_actor_with_privileged_edges() {
        let privileged = [(Waiting, Interrupted), (Review, Interrupted), (Testing, Interrupted)];
        let mut checked = 0;
        for actor in actors() {
            for from in TaskStatus::ALL {
                for to in TaskStatus::ALL {
                    let table = ALLOWED.contains(&(from, to));
                    let expected = table || (actor == TaskActor::SystemRecovery && privileged.contains(&(from, to)));
                    assert_eq!(can_transition_as(from, to, actor), expected, "{actor:?}: {from:?} → {to:?}");
                    assert_eq!(is_privileged_edge(from, to), privileged.contains(&(from, to)));
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 5 * 144);
        assert_eq!(ALLOWED.len(), 41);
    }

    #[test]
    fn privileged_transition_is_marked_forced() {
        let t0 = now();
        let mut task = Task::new(ProjectId::new(), "x", t0);
        for s in [Ready, Running, Testing] {
            transition(&mut task, s, t0).unwrap();
        }
        assert!(transition_as(&mut task, Interrupted, TaskActor::User, t0).is_err());
        assert!(transition_as(&mut task, Interrupted, TaskActor::Manager, t0).is_err());
        let applied = transition_as(&mut task, Interrupted, TaskActor::SystemRecovery, t0).unwrap();
        assert_eq!(applied, Applied { from: Testing, forced: true, new_attempt: false });
        // Running → Interrupted is a normal edge, even for the recovery actor.
        let mut t2 = Task::new(ProjectId::new(), "y", t0);
        transition(&mut t2, Ready, t0).unwrap();
        assert!(t2.ready_since.is_some());
        let a = transition_as(&mut t2, Running, TaskActor::System, t0).unwrap();
        assert!(a.new_attempt);
        assert!(t2.ready_since.is_none());
        let a = transition_as(&mut t2, Interrupted, TaskActor::SystemRecovery, t0).unwrap();
        assert!(!a.forced);
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

//! Agent instance state machine (contract §5, agent-runtime-spec §3).
//!
//! ```text
//! Idle      → Listening | Stopped
//! Listening → Thinking | Waiting | Error | Stopped
//! Thinking  → Coding | Testing | Reviewing | Waiting | Completed | Error | Stopped
//! Coding    → Thinking | Waiting | Error | Stopped
//! Testing   → Thinking | Waiting | Error | Stopped
//! Reviewing → Thinking | Waiting | Error | Stopped
//! Waiting   → Thinking | Coding | Testing | Reviewing | Error | Stopped
//! Error     → Thinking | Idle | Stopped
//! Completed → Idle | Stopped
//! Stopped   → (terminal)
//! ```
//!
//! A same-state "transition" is a no-op (no event). Everything else not in the
//! table is an [`InvalidAgentTransition`].

use orkestr_domain::AgentState;

use AgentState::{Coding, Completed, Error, Idle, Listening, Reviewing, Stopped, Testing, Thinking, Waiting};

/// Every agent state in a stable order.
pub const ALL_STATES: [AgentState; 10] =
    [Idle, Listening, Thinking, Coding, Testing, Reviewing, Waiting, Error, Completed, Stopped];

/// Rejected transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid agent state transition {from:?} → {to:?}")]
pub struct InvalidAgentTransition {
    /// Current state.
    pub from: AgentState,
    /// Requested state.
    pub to: AgentState,
}

/// States reachable from `from`.
#[must_use]
pub fn allowed_transitions(from: AgentState) -> &'static [AgentState] {
    match from {
        Idle => &[Listening, Stopped],
        Listening => &[Thinking, Waiting, Error, Stopped],
        Thinking => &[Coding, Testing, Reviewing, Waiting, Completed, Error, Stopped],
        Coding | Testing | Reviewing => &[Thinking, Waiting, Error, Stopped],
        Waiting => &[Thinking, Coding, Testing, Reviewing, Error, Stopped],
        Error => &[Thinking, Idle, Stopped],
        Completed => &[Idle, Stopped],
        Stopped => &[],
    }
}

/// Whether `from → to` is allowed (same state counts as allowed: a no-op).
#[must_use]
pub fn can_transition(from: AgentState, to: AgentState) -> bool {
    from == to || allowed_transitions(from).contains(&to)
}

/// Checks a transition.
pub fn check_transition(from: AgentState, to: AgentState) -> Result<(), InvalidAgentTransition> {
    if can_transition(from, to) {
        Ok(())
    } else {
        Err(InvalidAgentTransition { from, to })
    }
}

/// States in which the agent's run has ended.
#[must_use]
pub fn is_final(state: AgentState) -> bool {
    matches!(state, Completed | Error | Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full expected matrix, written out independently of the table.
    fn expected(from: AgentState, to: AgentState) -> bool {
        if from == to {
            return true;
        }
        let pairs: &[(AgentState, AgentState)] = &[
            (Idle, Listening),
            (Idle, Stopped),
            (Listening, Thinking),
            (Listening, Waiting),
            (Listening, Error),
            (Listening, Stopped),
            (Thinking, Coding),
            (Thinking, Testing),
            (Thinking, Reviewing),
            (Thinking, Waiting),
            (Thinking, Completed),
            (Thinking, Error),
            (Thinking, Stopped),
            (Coding, Thinking),
            (Coding, Waiting),
            (Coding, Error),
            (Coding, Stopped),
            (Testing, Thinking),
            (Testing, Waiting),
            (Testing, Error),
            (Testing, Stopped),
            (Reviewing, Thinking),
            (Reviewing, Waiting),
            (Reviewing, Error),
            (Reviewing, Stopped),
            (Waiting, Thinking),
            (Waiting, Coding),
            (Waiting, Testing),
            (Waiting, Reviewing),
            (Waiting, Error),
            (Waiting, Stopped),
            (Error, Thinking),
            (Error, Idle),
            (Error, Stopped),
            (Completed, Idle),
            (Completed, Stopped),
        ];
        pairs.contains(&(from, to))
    }

    #[test]
    fn every_pair_matches_the_spec_matrix() {
        let mut allowed = 0;
        for from in ALL_STATES {
            for to in ALL_STATES {
                assert_eq!(can_transition(from, to), expected(from, to), "{from:?} → {to:?}");
                if from != to && can_transition(from, to) {
                    allowed += 1;
                }
                let checked = check_transition(from, to);
                assert_eq!(checked.is_ok(), expected(from, to));
                if let Err(e) = checked {
                    assert_eq!((e.from, e.to), (from, to));
                    assert!(e.to_string().contains("invalid agent state transition"));
                }
            }
        }
        assert_eq!(allowed, 36);
    }

    #[test]
    fn stopped_is_terminal_and_finals() {
        assert!(allowed_transitions(Stopped).is_empty());
        for s in ALL_STATES {
            assert_eq!(is_final(s), matches!(s, Completed | Error | Stopped), "{s:?}");
        }
        assert!(!can_transition(Completed, Thinking), "a finished agent must be re-assigned via Idle");
        assert!(!can_transition(Idle, Thinking), "work starts by listening to an assignment");
    }
}

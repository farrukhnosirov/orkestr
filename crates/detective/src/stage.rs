//! Incident stage machine (ADR-0042 §5).
//!
//! `Opened → Collecting → Hypothesizing → Reproducing → AwaitingFixApproval →
//! Fixing → Testing → Reviewing → AwaitingMergeApproval → Resolved`, with
//! `Failed` and `Cancelled` reachable from every non-terminal stage. A failed
//! fix test may loop back `Testing → Fixing` (bounded retry).

use orkestr_protocol::methods::detective::IncidentStage;

use IncidentStage as S;

/// The happy-path successor of `stage` (`None` for gates' targets that need
/// an explicit decision and for terminal stages).
#[must_use]
pub fn next(stage: IncidentStage) -> Option<IncidentStage> {
    Some(match stage {
        S::Opened => S::Collecting,
        S::Collecting => S::Hypothesizing,
        S::Hypothesizing => S::Reproducing,
        S::Reproducing => S::AwaitingFixApproval,
        S::AwaitingFixApproval => S::Fixing,
        S::Fixing => S::Testing,
        S::Testing => S::Reviewing,
        S::Reviewing => S::AwaitingMergeApproval,
        S::AwaitingMergeApproval => S::Resolved,
        S::Resolved | S::Failed | S::Cancelled => return None,
    })
}

/// Whether `from -> to` is allowed.
#[must_use]
pub fn can_transition(from: IncidentStage, to: IncidentStage) -> bool {
    if from.is_terminal() {
        return false;
    }
    if matches!(to, S::Failed | S::Cancelled) {
        return true;
    }
    next(from) == Some(to) || (from == S::Testing && to == S::Fixing)
}

/// Human gate stages (an approval is pending).
#[must_use]
pub fn is_gate(stage: IncidentStage) -> bool {
    matches!(stage, S::AwaitingFixApproval | S::AwaitingMergeApproval)
}

/// Stage order index (timeline rendering, tests).
#[must_use]
pub fn ordinal(stage: IncidentStage) -> u8 {
    match stage {
        S::Opened => 0,
        S::Collecting => 1,
        S::Hypothesizing => 2,
        S::Reproducing => 3,
        S::AwaitingFixApproval => 4,
        S::Fixing => 5,
        S::Testing => 6,
        S::Reviewing => 7,
        S::AwaitingMergeApproval => 8,
        S::Resolved => 9,
        S::Failed => 10,
        S::Cancelled => 11,
    }
}

/// Every stage.
pub const ALL: [IncidentStage; 12] = [
    S::Opened,
    S::Collecting,
    S::Hypothesizing,
    S::Reproducing,
    S::AwaitingFixApproval,
    S::Fixing,
    S::Testing,
    S::Reviewing,
    S::AwaitingMergeApproval,
    S::Resolved,
    S::Failed,
    S::Cancelled,
];

/// Stable name (storage column, audit).
#[must_use]
pub fn name(stage: IncidentStage) -> &'static str {
    match stage {
        S::Opened => "Opened",
        S::Collecting => "Collecting",
        S::Hypothesizing => "Hypothesizing",
        S::Reproducing => "Reproducing",
        S::AwaitingFixApproval => "AwaitingFixApproval",
        S::Fixing => "Fixing",
        S::Testing => "Testing",
        S::Reviewing => "Reviewing",
        S::AwaitingMergeApproval => "AwaitingMergeApproval",
        S::Resolved => "Resolved",
        S::Failed => "Failed",
        S::Cancelled => "Cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_and_escape_hatches() {
        let mut s = S::Opened;
        let mut n = 0;
        while let Some(to) = next(s) {
            assert!(can_transition(s, to), "{s:?} -> {to:?}");
            s = to;
            n += 1;
        }
        assert_eq!((s, n), (S::Resolved, 9));
        for s in ALL {
            assert_eq!(can_transition(s, S::Cancelled), !s.is_terminal());
            assert_eq!(can_transition(s, S::Failed), !s.is_terminal());
            assert_eq!(ALL[ordinal(s) as usize], s);
        }
        assert!(can_transition(S::Testing, S::Fixing));
        assert!(!can_transition(S::Opened, S::Fixing));
        assert!(!can_transition(S::AwaitingFixApproval, S::Resolved));
        assert!(!can_transition(S::Resolved, S::Cancelled));
    }
}

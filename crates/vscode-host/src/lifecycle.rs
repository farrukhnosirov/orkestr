//! Lifecycle state machine of the VS Code server and the crash-restart
//! back-off.
//!
//! ```text
//! NotInstalled ─install→ Installing ─ok→ Stopped ─start→ Starting ─ready→ Running
//!      ↑                    │fail/cancel         ↑            │fail          │exit (crash)
//!      └──uninstall── Stopped/Error ←────────────┘            ↓              ↓
//!                                      Error ←─gave up── Restarting ←────────┘
//! Running/Starting/Restarting ─stop→ Stopping ─exited→ Stopped
//! ```

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use orkestr_protocol::methods::vscode::VsCodeState;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Install / update started.
    InstallStarted,
    /// Install finished and verified.
    InstallFinished,
    /// Install failed or was cancelled; `installed` = a previous version is still usable.
    InstallAborted {
        /// A usable installation remains.
        installed: bool,
        /// Failed (vs cancelled).
        failed: bool,
    },
    /// Package removed.
    Uninstalled,
    /// Start (or a restart attempt) begins.
    StartRequested,
    /// The server announced its port and answered the health check.
    Ready,
    /// The server could not be started.
    StartFailed,
    /// The process exited without a stop request; `restart` = a retry is scheduled.
    Crashed {
        /// A restart is scheduled.
        restart: bool,
    },
    /// Stop requested.
    StopRequested,
    /// The process is gone after a stop.
    Stopped,
}

/// Invalid transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot {trigger:?} while the VS Code server is {from:?}")]
pub struct InvalidTransition {
    /// State.
    pub from: VsCodeState,
    /// Trigger.
    pub trigger: Trigger,
}

/// The state after `trigger` in `from`.
pub fn next_state(from: VsCodeState, trigger: Trigger) -> Result<VsCodeState, InvalidTransition> {
    use Trigger as T;
    use VsCodeState as S;
    let to = match (from, trigger) {
        (S::NotInstalled | S::Stopped | S::Error, T::InstallStarted) => S::Installing,
        (S::Installing, T::InstallFinished) => S::Stopped,
        (S::Installing, T::InstallAborted { installed: true, .. }) => S::Stopped,
        (S::Installing, T::InstallAborted { installed: false, failed: true }) => S::Error,
        (S::Installing, T::InstallAborted { installed: false, failed: false }) => S::NotInstalled,
        (S::NotInstalled | S::Stopped | S::Error, T::Uninstalled) => S::NotInstalled,
        (S::Stopped | S::Error | S::Restarting, T::StartRequested) => S::Starting,
        (S::Starting, T::Ready) => S::Running,
        (S::Starting, T::StartFailed) => S::Error,
        (S::Starting | S::Running, T::Crashed { restart: true }) => S::Restarting,
        (S::Starting | S::Running | S::Restarting, T::Crashed { restart: false }) => S::Error,
        (S::Starting | S::Running | S::Restarting, T::StopRequested) => S::Stopping,
        (S::Stopping, T::Stopped | T::Crashed { .. }) => S::Stopped,
        (S::Stopped, T::StopRequested | T::Stopped) => S::Stopped,
        (from, trigger) => return Err(InvalidTransition { from, trigger }),
    };
    Ok(to)
}

/// Exponential restart back-off with a crash budget: at most `max_attempts`
/// restarts within `window`; then the supervisor gives up (state `Error`).
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    max_attempts: usize,
    window: Duration,
    attempts: VecDeque<Instant>,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(30), 5, Duration::from_secs(600))
    }
}

impl Backoff {
    /// Custom back-off.
    #[must_use]
    pub fn new(base: Duration, max: Duration, max_attempts: usize, window: Duration) -> Self {
        Self { base, max, max_attempts, window, attempts: VecDeque::new() }
    }

    /// Delay before the next restart, or `None` when the crash budget is spent.
    pub fn next_delay(&mut self, now: Instant) -> Option<Duration> {
        while let Some(first) = self.attempts.front() {
            if now.saturating_duration_since(*first) > self.window {
                self.attempts.pop_front();
            } else {
                break;
            }
        }
        if self.attempts.len() >= self.max_attempts {
            return None;
        }
        let exp = u32::try_from(self.attempts.len()).unwrap_or(u32::MAX).min(16);
        let delay = self.base.saturating_mul(2u32.saturating_pow(exp)).min(self.max);
        self.attempts.push_back(now);
        Some(delay)
    }

    /// Restarts counted in the current window.
    #[must_use]
    pub fn attempts(&self) -> usize {
        self.attempts.len()
    }

    /// Forget past crashes (after a user start).
    pub fn reset(&mut self) {
        self.attempts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Trigger as T;
    use VsCodeState as S;

    #[test]
    fn happy_path() {
        let mut s = S::NotInstalled;
        for (t, want) in [
            (T::InstallStarted, S::Installing),
            (T::InstallFinished, S::Stopped),
            (T::StartRequested, S::Starting),
            (T::Ready, S::Running),
            (T::StopRequested, S::Stopping),
            (T::Stopped, S::Stopped),
            (T::Uninstalled, S::NotInstalled),
        ] {
            s = next_state(s, t).unwrap();
            assert_eq!(s, want, "after {t:?}");
        }
    }

    #[test]
    fn crash_restart_and_give_up() {
        let s = next_state(S::Running, T::Crashed { restart: true }).unwrap();
        assert_eq!(s, S::Restarting);
        let s = next_state(s, T::StartRequested).unwrap();
        assert_eq!(s, S::Starting);
        assert_eq!(next_state(S::Restarting, T::Crashed { restart: false }).unwrap(), S::Error);
        assert_eq!(next_state(S::Running, T::Crashed { restart: false }).unwrap(), S::Error);
        // A stop during the back-off wins.
        assert_eq!(next_state(S::Restarting, T::StopRequested).unwrap(), S::Stopping);
        assert_eq!(next_state(S::Stopping, T::Crashed { restart: true }).unwrap(), S::Stopped);
    }

    #[test]
    fn install_outcomes() {
        assert_eq!(next_state(S::Installing, T::InstallAborted { installed: false, failed: true }).unwrap(), S::Error);
        assert_eq!(
            next_state(S::Installing, T::InstallAborted { installed: false, failed: false }).unwrap(),
            S::NotInstalled
        );
        assert_eq!(next_state(S::Installing, T::InstallAborted { installed: true, failed: true }).unwrap(), S::Stopped);
        // Update while stopped.
        assert_eq!(next_state(S::Stopped, T::InstallStarted).unwrap(), S::Installing);
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        for (from, t) in [
            (S::NotInstalled, T::StartRequested),
            (S::Installing, T::StartRequested),
            (S::Running, T::InstallStarted),
            (S::Running, T::Uninstalled),
            (S::Installing, T::Uninstalled),
            (S::Stopped, T::Ready),
            (S::Starting, T::StartRequested),
        ] {
            assert!(next_state(from, t).is_err(), "{from:?} + {t:?}");
        }
    }

    #[test]
    fn backoff_grows_caps_and_gives_up() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(5), 4, Duration::from_secs(60));
        let t0 = Instant::now();
        let delays: Vec<_> = (0..4).map(|_| b.next_delay(t0).unwrap()).collect();
        assert_eq!(delays, [1, 2, 4, 5].map(Duration::from_secs));
        assert_eq!(b.next_delay(t0), None, "crash budget spent");
        // Old crashes leave the window.
        assert_eq!(b.next_delay(t0 + Duration::from_secs(61)), Some(Duration::from_secs(1)));
        b.reset();
        assert_eq!(b.attempts(), 0);
    }
}

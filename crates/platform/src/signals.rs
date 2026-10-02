//! Process shutdown signals for headless services (`orkestrd`, §12).
//!
//! [`shutdown_signal`] resolves on the first request to terminate:
//!
//! * every OS: Ctrl-C;
//! * Unix: `SIGTERM` (service managers, `kill`) and `SIGHUP` (terminal closed);
//! * Windows: console close, logoff and system shutdown events.
//!
//! All `#[cfg]` handling lives here so binaries stay platform-agnostic
//! (contract §2).

/// Which signal asked the process to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    /// Ctrl-C / `SIGINT`.
    Interrupt,
    /// `SIGTERM`.
    Terminate,
    /// `SIGHUP`.
    Hangup,
    /// Windows console window closed.
    ConsoleClose,
    /// Windows user logoff.
    Logoff,
    /// Windows system shutdown.
    SystemShutdown,
}

impl std::fmt::Display for ShutdownSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Hangup => "hangup",
            Self::ConsoleClose => "console-close",
            Self::Logoff => "logoff",
            Self::SystemShutdown => "system-shutdown",
        })
    }
}

/// Waits for a shutdown request. Handlers that cannot be installed are
/// skipped (logged), never fatal.
pub async fn shutdown_signal() -> ShutdownSignal {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => ShutdownSignal::Interrupt,
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for Ctrl-C");
                std::future::pending().await
            }
        }
    };
    tokio::select! {
        s = ctrl_c => s,
        s = os_signal() => s,
    }
}

#[cfg(unix)]
async fn os_signal() -> ShutdownSignal {
    use tokio::signal::unix::{signal, SignalKind};
    async fn wait(kind: SignalKind, out: ShutdownSignal) -> ShutdownSignal {
        match signal(kind) {
            Ok(mut s) => {
                s.recv().await;
                out
            }
            Err(e) => {
                tracing::warn!(error = %e, signal = %out, "cannot install signal handler");
                std::future::pending().await
            }
        }
    }
    tokio::select! {
        s = wait(SignalKind::terminate(), ShutdownSignal::Terminate) => s,
        s = wait(SignalKind::hangup(), ShutdownSignal::Hangup) => s,
    }
}

#[cfg(windows)]
async fn os_signal() -> ShutdownSignal {
    use tokio::signal::windows;
    async fn wait<F, S>(make: F, out: ShutdownSignal) -> ShutdownSignal
    where
        F: FnOnce() -> std::io::Result<S>,
        S: WindowsSignal,
    {
        match make() {
            Ok(mut s) => {
                s.recv_signal().await;
                out
            }
            Err(e) => {
                tracing::debug!(error = %e, signal = %out, "cannot install console handler (no console?)");
                std::future::pending().await
            }
        }
    }
    tokio::select! {
        s = wait(windows::ctrl_close, ShutdownSignal::ConsoleClose) => s,
        s = wait(windows::ctrl_logoff, ShutdownSignal::Logoff) => s,
        s = wait(windows::ctrl_shutdown, ShutdownSignal::SystemShutdown) => s,
    }
}

/// Uniform `recv` over tokio's distinct Windows signal stream types.
#[cfg(windows)]
trait WindowsSignal {
    async fn recv_signal(&mut self);
}

#[cfg(windows)]
impl WindowsSignal for tokio::signal::windows::CtrlClose {
    async fn recv_signal(&mut self) {
        self.recv().await;
    }
}

#[cfg(windows)]
impl WindowsSignal for tokio::signal::windows::CtrlLogoff {
    async fn recv_signal(&mut self) {
        self.recv().await;
    }
}

#[cfg(windows)]
impl WindowsSignal for tokio::signal::windows::CtrlShutdown {
    async fn recv_signal(&mut self) {
        self.recv().await;
    }
}

#[cfg(not(any(unix, windows)))]
async fn os_signal() -> ShutdownSignal {
    std::future::pending().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn does_not_fire_spontaneously() {
        let r = tokio::time::timeout(std::time::Duration::from_millis(100), shutdown_signal()).await;
        assert!(r.is_err(), "no signal was sent, so the future must still be pending");
        assert_eq!(ShutdownSignal::SystemShutdown.to_string(), "system-shutdown");
    }
}

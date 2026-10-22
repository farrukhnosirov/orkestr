//! Process listing / termination (cross-platform via `sysinfo`).

use orkestr_protocol::methods::computer::ProcessInfo;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::error::{ComputerError, ComputerResult};

/// Running processes, aggregated by name (largest memory first).
#[must_use]
pub fn list() -> Vec<ProcessInfo> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_memory());
    let mut out: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            memory_bytes: p.memory(),
            has_window: false,
        })
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(p.memory_bytes));
    out
}

/// Pids whose process name matches `name` (case-insensitive, `.exe` optional).
#[must_use]
pub fn pids_named(name: &str) -> Vec<u32> {
    let want = norm(name);
    if want.is_empty() {
        return Vec::new();
    }
    list().into_iter().filter(|p| norm(&p.name) == want).map(|p| p.pid).collect()
}

/// Lowercase without `.exe`.
#[must_use]
pub fn norm(name: &str) -> String {
    let l = name.trim().to_lowercase();
    l.strip_suffix(".exe").unwrap_or(&l).to_owned()
}

/// Kills `pid`. Refuses this process.
///
/// # Errors
/// `Forbidden` for Orkestr itself, `NotFound`, `Backend`.
pub fn kill(pid: u32) -> ComputerResult<()> {
    if pid == std::process::id() {
        return Err(ComputerError::Forbidden("refusing to kill Orkestr itself".into()));
    }
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
    let proc_ = sys.process(p).ok_or_else(|| ComputerError::NotFound(format!("process {pid}")))?;
    if proc_.kill() {
        Ok(())
    } else {
        Err(ComputerError::Backend(format!("could not terminate process {pid}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_this_process_and_refuses_to_kill_it() {
        let me = std::process::id();
        assert!(list().iter().any(|p| p.pid == me));
        assert!(matches!(kill(me), Err(ComputerError::Forbidden(_))));
        assert_eq!(norm("Notepad.EXE"), "notepad");
    }
}

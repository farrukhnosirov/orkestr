//! Process-tree termination that survives spawn races.
//!
//! Killing a tree is racy without OS containment (Job Objects / cgroups, which
//! the platform layer will provide per ADR-0022):
//!
//! * `taskkill /T` enumerates once — a child spawned meanwhile survives;
//! * killing a *child* first lets its parent script continue with the next
//!   command (`ping … & echo x > file` would still write the file).
//!
//! So the terminal kills **top-down**: it snapshots the tree (parent links)
//! before touching anything, kills level by level starting at the root (a dead
//! parent cannot start new commands), then re-scans a few times for survivors
//! and late-spawned orphans (orphans keep their dead parent's pid on Windows).
//! Processes that started before the root are ignored (pid-reuse guard).

use std::collections::HashSet;
use std::time::Duration;

use orkestr_platform::ProcessControl;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Re-scan rounds after the first pass.
const ROUNDS: usize = 4;
/// Pause between rounds.
const ROUND_PAUSE: Duration = Duration::from_millis(80);

fn refresh(sys: &mut System) {
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
}

/// Extends `tree` with every live descendant (transitively) of its members.
fn grow(sys: &System, tree: &mut HashSet<Pid>, root_start: u64) {
    loop {
        let added: Vec<Pid> = sys
            .processes()
            .iter()
            .filter(|(pid, p)| {
                !tree.contains(pid)
                    && p.start_time() >= root_start
                    && p.parent().is_some_and(|parent| tree.contains(&parent))
            })
            .map(|(pid, _)| *pid)
            .collect();
        if added.is_empty() {
            return;
        }
        tree.extend(added);
    }
}

/// Live members of `tree`, grouped top-down: a process comes after its live parent.
fn levels(sys: &System, tree: &HashSet<Pid>) -> Vec<Vec<Pid>> {
    let alive: HashSet<Pid> = tree.iter().copied().filter(|p| sys.process(*p).is_some()).collect();
    let mut placed: HashSet<Pid> = HashSet::new();
    let mut out = Vec::new();
    while placed.len() < alive.len() {
        let level: Vec<Pid> = alive
            .iter()
            .copied()
            .filter(|p| !placed.contains(p))
            .filter(|p| {
                let parent = sys.process(*p).and_then(sysinfo::Process::parent);
                // Top of what's left: parent dead, outside the tree, or already placed.
                parent.map_or(true, |pp| !alive.contains(&pp) || placed.contains(&pp))
            })
            .collect();
        if level.is_empty() {
            // Parent cycle (pid reuse): place the rest together.
            out.push(alive.iter().copied().filter(|p| !placed.contains(p)).collect());
            break;
        }
        placed.extend(level.iter().copied());
        out.push(level);
    }
    out
}

/// Kills `root` and all its descendants (blocking; run on the blocking pool).
/// Returns how many processes of the tree were seen.
pub fn kill_process_tree(root: u32, platform: &dyn ProcessControl) -> usize {
    let root_pid = Pid::from_u32(root);
    let mut sys = System::new();
    refresh(&mut sys);
    let Some(root_start) = sys.process(root_pid).map(sysinfo::Process::start_time) else {
        // Unknown to the snapshot: still ask the platform (it may be a group leader).
        let _ = platform.kill_tree(root);
        return 0;
    };
    let mut tree: HashSet<Pid> = HashSet::from([root_pid]);
    grow(&sys, &mut tree, root_start);

    for round in 0..=ROUNDS {
        if round > 0 {
            std::thread::sleep(ROUND_PAUSE);
            refresh(&mut sys);
            grow(&sys, &mut tree, root_start);
        }
        let order = levels(&sys, &tree);
        if order.is_empty() {
            break;
        }
        if round > 0 {
            tracing::debug!(
                root,
                survivors = order.iter().map(Vec::len).sum::<usize>(),
                "killing surviving descendants"
            );
        }
        for level in &order {
            kill_level(&sys, level);
        }
    }
    // Backstop (process groups on Unix, `taskkill /T` on Windows); errors are
    // expected when everything is already gone.
    if let Err(e) = platform.kill_tree(root) {
        tracing::trace!(root, error = %e, "platform kill_tree after sweep");
    }
    tree.len()
}

#[cfg(windows)]
fn kill_level(_sys: &System, pids: &[Pid]) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if pids.is_empty() {
        return;
    }
    // One taskkill per level; without /T so the order stays top-down.
    let mut cmd = std::process::Command::new("taskkill");
    cmd.arg("/F");
    for pid in pids {
        cmd.arg("/PID").arg(pid.as_u32().to_string());
    }
    cmd.creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Err(e) = cmd.status() {
        tracing::warn!(error = %e, "taskkill failed to start");
    }
}

#[cfg(not(windows))]
fn kill_level(sys: &System, pids: &[Pid]) {
    for pid in pids {
        if let Some(p) = sys.process(*pid) {
            // SIGKILL; a dead/zombie process simply reports failure.
            let _ = p.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_platform::NativeProcessControl;

    #[test]
    fn missing_process_is_a_noop() {
        assert_eq!(kill_process_tree(u32::MAX - 11, &NativeProcessControl), 0);
    }

    #[test]
    fn kills_child_and_grandchild_top_down() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("after-child.txt");
        // The middle shell would write the marker as soon as its child dies.
        #[cfg(windows)]
        let mut child = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("cmd")
                .args(["/d", "/s", "/c"])
                .raw_arg(format!("\"cmd /d /s /c \"ping -n 30 127.0.0.1 >nul & echo x > \"{}\"\"\"", marker.display()))
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        };
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("sh")
            .args(["-c", &format!("sh -c 'sleep 30; echo x > \"{}\"' ; true", marker.display())])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        let mut sys = System::new();
        refresh(&mut sys);
        let mut before = HashSet::from([Pid::from_u32(child.id())]);
        let start = sys.process(Pid::from_u32(child.id())).unwrap().start_time();
        grow(&sys, &mut before, start);
        assert!(before.len() >= 2, "expected descendants: {before:?}");
        let order = levels(&sys, &before);
        assert_eq!(order.first().map(Vec::as_slice), Some([Pid::from_u32(child.id())].as_slice()));

        kill_process_tree(child.id(), &NativeProcessControl);
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(500));
        refresh(&mut sys);
        let alive: Vec<_> = before.iter().filter(|p| sys.process(**p).is_some()).collect();
        assert!(alive.is_empty(), "survivors: {alive:?}");
        assert!(!marker.exists(), "a parent continued after its child was killed");
    }
}

//! Safety classification of steps (static part; the executor adds runtime
//! checks: password fields, windows with unsaved changes). Deletes go
//! through the [`DangerousActionGuard`] as the equivalent PowerShell
//! command, so its hard-denied paths (drive roots, OS dirs, Orkestr's data
//! dir…) are refused here too.

use std::path::Path;

use orkestr_protocol::methods::computer::{ComputerAction, RiskReason, SafetyLevel, SystemOp};
use orkestr_security::{DangerousActionGuard, ShellKind};

use crate::error::{ComputerError, ComputerResult};

/// Processes that are never killed (shell / session / security).
pub const PROTECTED_PROCESSES: &[&str] = &[
    "system",
    "idle",
    "registry",
    "smss",
    "csrss",
    "wininit",
    "winlogon",
    "services",
    "lsass",
    "lsaiso",
    "svchost",
    "dwm",
    "explorer",
    "fontdrvhost",
    "sihost",
    "msmpeng",
    "securityhealthservice",
    "orkestrd",
    "orkestr-desktop",
    "orkestr",
];

fn reason(step: usize, code: &str, detail: impl Into<String>, level: SafetyLevel) -> RiskReason {
    RiskReason { step: u32::try_from(step).unwrap_or(u32::MAX), code: code.into(), detail: detail.into(), level }
}

/// Risk reasons of (slot-resolved) steps. `Err(Forbidden)` for steps that
/// must never run.
///
/// # Errors
/// `Forbidden` (protected process / path).
pub fn classify(steps: &[ComputerAction], guard: Option<&DangerousActionGuard>) -> ComputerResult<Vec<RiskReason>> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    classify_into(steps, guard, &mut idx, &mut out)?;
    Ok(out)
}

fn classify_into(
    steps: &[ComputerAction],
    guard: Option<&DangerousActionGuard>,
    idx: &mut usize,
    out: &mut Vec<RiskReason>,
) -> ComputerResult<()> {
    for s in steps {
        let i = *idx;
        *idx += 1;
        match s {
            ComputerAction::CloseApp { app, discard: true } => {
                out.push(reason(i, "closeDiscard", app.clone(), SafetyLevel::Confirm));
            }
            ComputerAction::KillProcess { name } => {
                let n = crate::procs::norm(name);
                for p in crate::apps::process_names(name) {
                    if PROTECTED_PROCESSES.contains(&crate::procs::norm(&p).as_str()) {
                        return Err(ComputerError::Forbidden(format!("refusing to kill the system process `{p}`")));
                    }
                }
                if PROTECTED_PROCESSES.contains(&n.as_str()) {
                    return Err(ComputerError::Forbidden(format!("refusing to kill the system process `{name}`")));
                }
                out.push(reason(i, "killProcess", name.clone(), SafetyLevel::Dangerous));
            }
            ComputerAction::FileDelete { path } => {
                check_path(path, guard)?;
                out.push(reason(i, "fileDelete", path.clone(), SafetyLevel::Dangerous));
            }
            ComputerAction::FileMove { from, to } => {
                check_path(from, guard)?;
                out.push(reason(i, "fileMove", format!("{from} → {to}"), SafetyLevel::Confirm));
            }
            ComputerAction::System { op } => match op {
                SystemOp::Lock => {}
                SystemOp::Sleep => out.push(reason(i, "power", "sleep", SafetyLevel::Confirm)),
                SystemOp::Shutdown => out.push(reason(i, "power", "shutdown", SafetyLevel::Dangerous)),
                SystemOp::Restart => out.push(reason(i, "power", "restart", SafetyLevel::Dangerous)),
                SystemOp::SignOut => out.push(reason(i, "power", "signOut", SafetyLevel::Dangerous)),
            },
            ComputerAction::If { then, otherwise, .. } => {
                classify_into(then, guard, idx, out)?;
                classify_into(otherwise, guard, idx, out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_path(spoken: &str, guard: Option<&DangerousActionGuard>) -> ComputerResult<()> {
    let p = crate::apps::file_path(spoken);
    if p.parent().is_none() || p.components().count() <= 2 {
        return Err(ComputerError::Forbidden(format!("refusing to touch the root path {}", p.display())));
    }
    if is_profile_root(&p) {
        return Err(ComputerError::Forbidden(format!("refusing to touch {}", p.display())));
    }
    if let Some(g) = guard {
        let cmd = format!("Remove-Item -Recurse -Force -LiteralPath '{}'", p.display());
        let class = g.classify(&cmd, ShellKind::PowerShell);
        if class.is_forbidden() {
            return Err(ComputerError::Forbidden(format!(
                "the Dangerous Action Guard refuses this path: {}",
                class.reason().unwrap_or("protected path")
            )));
        }
    }
    Ok(())
}

fn is_profile_root(p: &Path) -> bool {
    directories::UserDirs::new().is_some_and(|u| {
        let home = u.home_dir();
        p == home
            || [u.document_dir(), u.desktop_dir(), u.download_dir(), u.picture_dir()]
                .into_iter()
                .flatten()
                .any(|d| d == p)
    })
}

/// Effective level: the max of the declared and the steps' levels.
#[must_use]
pub fn effective(declared: SafetyLevel, reasons: &[RiskReason]) -> SafetyLevel {
    reasons.iter().map(|r| r.level).fold(declared, Ord::max)
}

/// Window titles that look like unsaved documents (`*Untitled - Notepad`,
/// `● file.txt - Visual Studio Code`, `Document1 - Word (unsaved)`).
#[must_use]
pub fn title_looks_unsaved(title: &str) -> bool {
    let t = title.trim_start();
    t.starts_with('*') || t.starts_with('●') || t.starts_with('•') || t.contains(" *") || t.contains("(unsaved)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(json: serde_json::Value) -> ComputerAction {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn risky_steps_need_confirmation() {
        let g = DangerousActionGuard::new();
        let steps = vec![
            act(serde_json::json!({"type":"Launch","target":"notepad"})),
            act(serde_json::json!({"type":"CloseApp","app":"notepad","discard":true})),
            act(serde_json::json!({"type":"KillProcess","name":"chrome"})),
            act(serde_json::json!({"type":"System","op":"Shutdown"})),
            act(serde_json::json!({"type":"FileDelete","path":"old/notes.txt"})),
        ];
        let r = classify(&steps, Some(&g)).unwrap();
        let codes: Vec<&str> = r.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, ["closeDiscard", "killProcess", "power", "fileDelete"]);
        assert_eq!(effective(SafetyLevel::Safe, &r), SafetyLevel::Dangerous);
        let safe = classify(&steps[..1], Some(&g)).unwrap();
        assert!(safe.is_empty());
        assert_eq!(effective(SafetyLevel::Safe, &safe), SafetyLevel::Safe);
    }

    #[test]
    fn protected_targets_are_forbidden() {
        let g = DangerousActionGuard::new();
        for name in ["explorer", "lsass.exe", "проводник", "orkestrd"] {
            let s = [act(serde_json::json!({"type":"KillProcess","name":name}))];
            assert!(matches!(classify(&s, Some(&g)), Err(ComputerError::Forbidden(_))), "{name}");
        }
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let s = [act(serde_json::json!({"type":"FileDelete","path":root}))];
        assert!(matches!(classify(&s, Some(&g)), Err(ComputerError::Forbidden(_))));
        let s = [act(serde_json::json!({"type":"FileDelete","path":"documents"}))];
        let home = directories::UserDirs::new().unwrap().home_dir().display().to_string();
        let s2 = [act(serde_json::json!({"type":"FileDelete","path":home}))];
        assert!(matches!(classify(&s2, Some(&g)), Err(ComputerError::Forbidden(_))));
        // "documents" as a relative name is a file inside Documents → allowed but dangerous
        assert_eq!(classify(&s, Some(&g)).unwrap()[0].level, SafetyLevel::Dangerous);
    }

    #[test]
    fn nested_steps_are_classified() {
        let s = [act(serde_json::json!({"type":"If","condition":{"type":"ProcessRunning","name":"x"},
            "then":[{"type":"System","op":"Restart"}],"otherwise":[]}))];
        assert_eq!(classify(&s, None).unwrap()[0].code, "power");
    }

    #[test]
    fn unsaved_titles() {
        assert!(title_looks_unsaved("*Untitled - Notepad"));
        assert!(title_looks_unsaved("● main.rs - orkestr - Visual Studio Code"));
        assert!(!title_looks_unsaved("Untitled - Notepad"));
    }
}

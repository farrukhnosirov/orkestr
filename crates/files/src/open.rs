//! Opening workspace paths outside Orkestr: OS default application, reveal
//! in the file manager, or an allow-listed external IDE.
//!
//! The command is planned by the pure function [`plan_command`] (unit
//! tested for every OS) and launched by [`SystemOpener`] without a shell:
//! the program is resolved on `PATH` and every argument is passed verbatim.
//! OS selection uses `std::env::consts::OS` at runtime (no `#[cfg]` here);
//! `orkestr-platform` may later provide its own [`ExternalOpener`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use orkestr_protocol::methods::files::OpenTarget;

use crate::error::{FilesError, FilesResult};
use crate::path::display_path;

/// IDE commands that may be launched.
pub const IDE_ALLOWLIST: &[&str] = &[
    "code",
    "code-insiders",
    "codium",
    "cursor",
    "windsurf",
    "zed",
    "subl",
    "idea",
    "idea64",
    "webstorm",
    "webstorm64",
    "rider",
    "rider64",
    "pycharm",
    "pycharm64",
    "goland",
    "goland64",
    "clion",
    "clion64",
    "fleet",
];

/// Preference order when no IDE is specified.
pub const IDE_PREFERENCE: &[&str] = &["code", "cursor", "windsurf", "zed", "code-insiders", "codium", "subl", "idea"];

/// VS Code family: `<root> -g <file>` opens the folder and the file.
const VSCODE_FAMILY: &[&str] = &["code", "code-insiders", "codium", "cursor", "windsurf"];

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// Canonical workspace root.
    pub root: PathBuf,
    /// Absolute target inside the root.
    pub target: PathBuf,
    /// Target is a directory.
    pub is_dir: bool,
    /// How to open.
    pub mode: OpenTarget,
    /// Requested IDE command (allow-listed).
    pub ide: Option<String>,
}

/// A planned process launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    /// Program (resolved path or bare name).
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<OsString>,
    /// Short label for results/logs (`explorer`, `code`, …).
    pub label: String,
}

/// Opens paths externally.
pub trait ExternalOpener: Send + Sync {
    /// Launches; returns the program label.
    fn open(&self, req: &OpenRequest) -> FilesResult<String>;
}

/// Plans the command for `os` (`std::env::consts::OS` values).
pub fn plan_command(
    req: &OpenRequest,
    os: &str,
    which: &dyn Fn(&str) -> Option<PathBuf>,
) -> FilesResult<PlannedCommand> {
    let target = display_path(&req.target);
    let root = display_path(&req.root);
    let resolve = |name: &str| which(name).unwrap_or_else(|| PathBuf::from(name));
    match req.mode {
        OpenTarget::Ide => {
            let ide = match req.ide.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(name) => {
                    let name = name.to_ascii_lowercase();
                    if !IDE_ALLOWLIST.contains(&name.as_str()) {
                        return Err(FilesError::Unsupported(format!(
                            "IDE `{name}` is not supported (allowed: {})",
                            IDE_ALLOWLIST.join(", ")
                        )));
                    }
                    let path =
                        which(&name).ok_or_else(|| FilesError::NotFound(format!("IDE command `{name}` on PATH")))?;
                    (name, path)
                }
                None => IDE_PREFERENCE
                    .iter()
                    .find_map(|n| which(n).map(|p| ((*n).to_owned(), p)))
                    .ok_or_else(|| FilesError::NotFound("an external IDE (code, cursor, zed, …) on PATH".into()))?,
            };
            let (name, program) = ide;
            let args: Vec<OsString> = if VSCODE_FAMILY.contains(&name.as_str()) && !req.is_dir {
                vec![root.into_os_string(), "-g".into(), target.into_os_string()]
            } else {
                vec![target.into_os_string()]
            };
            Ok(PlannedCommand { program, args, label: name })
        }
        OpenTarget::Default | OpenTarget::Reveal => {
            let reveal = req.mode == OpenTarget::Reveal && req.target != req.root;
            match os {
                "windows" => {
                    let arg: OsString = if reveal {
                        let mut a = OsString::from("/select,");
                        a.push(target.as_os_str());
                        a
                    } else {
                        target.into_os_string()
                    };
                    Ok(PlannedCommand { program: resolve("explorer"), args: vec![arg], label: "explorer".into() })
                }
                "macos" => {
                    let mut args: Vec<OsString> = Vec::new();
                    if reveal {
                        args.push("-R".into());
                    }
                    args.push(target.into_os_string());
                    Ok(PlannedCommand { program: resolve("open"), args, label: "open".into() })
                }
                _ => {
                    let what =
                        if reveal { target.parent().map_or_else(|| target.clone(), Path::to_path_buf) } else { target };
                    Ok(PlannedCommand {
                        program: resolve("xdg-open"),
                        args: vec![what.into_os_string()],
                        label: "xdg-open".into(),
                    })
                }
            }
        }
    }
}

/// Resolver of program names on `PATH`.
pub type WhichFn = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// Launches real processes.
pub struct SystemOpener {
    which: WhichFn,
}

impl std::fmt::Debug for SystemOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemOpener").finish_non_exhaustive()
    }
}

impl SystemOpener {
    /// New opener with a `which` implementation (e.g. `orkestr_platform::process::which`).
    #[must_use]
    pub fn new(which: WhichFn) -> Self {
        Self { which }
    }
}

impl ExternalOpener for SystemOpener {
    fn open(&self, req: &OpenRequest) -> FilesResult<String> {
        let plan = plan_command(req, std::env::consts::OS, &*self.which)?;
        let mut child = orkestr_platform::process::background_command(&plan.program)
            .args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| FilesError::Io { path: plan.label.clone(), source: e })?;
        // Reap in the background so no zombie is left behind (explorer.exe exits 1 on success).
        let _ = std::thread::Builder::new().name("orkestr-open-reaper".into()).spawn(move || {
            let _ = child.wait();
        });
        tracing::info!(program = %plan.label, mode = ?req.mode, "opened path externally");
        Ok(plan.label)
    }
}

/// Opener that records requests instead of launching (tests, headless).
#[derive(Debug, Default)]
pub struct RecordingOpener {
    /// Requests seen.
    pub requests: parking_lot::Mutex<Vec<OpenRequest>>,
}

impl ExternalOpener for RecordingOpener {
    fn open(&self, req: &OpenRequest) -> FilesResult<String> {
        self.requests.lock().push(req.clone());
        Ok("recorded".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(mode: OpenTarget, target: &str, is_dir: bool, ide: Option<&str>) -> OpenRequest {
        OpenRequest {
            root: PathBuf::from(r"\\?\C:\work\proj"),
            target: PathBuf::from(target),
            is_dir,
            mode,
            ide: ide.map(str::to_owned),
        }
    }

    fn which_some(name: &str) -> Option<PathBuf> {
        matches!(name, "code" | "zed" | "explorer").then(|| PathBuf::from(format!("/bin/{name}")))
    }

    #[test]
    fn windows_default_and_reveal_use_explorer_without_verbatim_prefix() {
        let p = plan_command(&req(OpenTarget::Default, r"\\?\C:\work\proj\a.txt", false, None), "windows", &which_some)
            .unwrap();
        assert_eq!(p.label, "explorer");
        assert_eq!(p.args, vec![OsString::from(r"C:\work\proj\a.txt")]);
        let p = plan_command(&req(OpenTarget::Reveal, r"\\?\C:\work\proj\a.txt", false, None), "windows", &which_some)
            .unwrap();
        assert_eq!(p.args, vec![OsString::from(r"/select,C:\work\proj\a.txt")]);
        // Revealing the root just opens it.
        let p =
            plan_command(&req(OpenTarget::Reveal, r"\\?\C:\work\proj", true, None), "windows", &which_some).unwrap();
        assert_eq!(p.args, vec![OsString::from(r"C:\work\proj")]);
    }

    #[test]
    fn macos_and_linux() {
        let p = plan_command(&req(OpenTarget::Reveal, "/w/a.txt", false, None), "macos", &|_| None).unwrap();
        assert_eq!(p.program, PathBuf::from("open"));
        assert_eq!(p.args, vec![OsString::from("-R"), OsString::from("/w/a.txt")]);
        let p = plan_command(&req(OpenTarget::Reveal, "/w/sub/a.txt", false, None), "linux", &|_| None).unwrap();
        assert_eq!(p.label, "xdg-open");
        assert_eq!(p.args, vec![OsString::from("/w/sub")]);
        let p = plan_command(&req(OpenTarget::Default, "/w/sub/a.txt", false, None), "freebsd", &|_| None).unwrap();
        assert_eq!(p.args, vec![OsString::from("/w/sub/a.txt")]);
    }

    #[test]
    fn ide_selection_allowlist_and_args() {
        let p = plan_command(&req(OpenTarget::Ide, r"\\?\C:\work\proj\src\a.rs", false, None), "windows", &which_some)
            .unwrap();
        assert_eq!(p.label, "code");
        assert_eq!(p.program, PathBuf::from("/bin/code"));
        assert_eq!(
            p.args,
            vec![OsString::from(r"C:\work\proj"), OsString::from("-g"), OsString::from(r"C:\work\proj\src\a.rs")]
        );
        let p = plan_command(&req(OpenTarget::Ide, "/w/src", true, Some("Zed")), "linux", &which_some).unwrap();
        assert_eq!(p.label, "zed");
        assert_eq!(p.args, vec![OsString::from("/w/src")]);
        assert!(matches!(
            plan_command(&req(OpenTarget::Ide, "/w", true, Some("rm")), "linux", &which_some),
            Err(FilesError::Unsupported(_))
        ));
        assert!(matches!(
            plan_command(&req(OpenTarget::Ide, "/w", true, Some("cursor")), "linux", &which_some),
            Err(FilesError::NotFound(_))
        ));
        assert!(matches!(
            plan_command(&req(OpenTarget::Ide, "/w", true, None), "linux", &|_| None),
            Err(FilesError::NotFound(_))
        ));
    }
}

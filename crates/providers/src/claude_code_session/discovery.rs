//! Finds the official Claude Code binary.
//!
//! Order: the path the user pinned → the newest `anthropic.claude-code-*`
//! extension in Orkestr's VS Code extensions directory
//! (`resources/native-binary/claude[.exe]`) → the extension in Orkestr's
//! native extensions directory → `claude` on `PATH`. Only file metadata is
//! read; nothing in the extension is executed except the binary itself.

use std::path::{Path, PathBuf};

use orkestr_protocol::methods::claude_code::ClaudeBinarySource;

/// Extension id of the official Claude Code extension.
pub const EXTENSION_ID: &str = "anthropic.claude-code";

/// A discovered binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// Absolute path.
    pub path: PathBuf,
    /// Where it was found.
    pub source: ClaudeBinarySource,
}

/// Inputs of a discovery run.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryInput {
    /// Path pinned by the user.
    pub override_path: Option<String>,
    /// Extension roots of Orkestr's VS Code (folders containing `publisher.name-version[-target]` dirs).
    pub vscode_extension_roots: Vec<PathBuf>,
    /// Installed native extensions: `(id, folder)`.
    pub native_extensions: Vec<(String, PathBuf)>,
}

/// File name of the binary on this OS.
#[must_use]
pub fn binary_file_name() -> &'static str {
    if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    }
}

/// The binary inside an extension folder, if present.
#[must_use]
pub fn binary_in_extension(dir: &Path) -> Option<PathBuf> {
    let p = dir.join("resources").join("native-binary").join(binary_file_name());
    p.is_file().then_some(p)
}

/// Numeric version of an extension folder name
/// (`anthropic.claude-code-2.1.292-win32-x64` → `[2, 1, 292]`).
#[must_use]
pub fn folder_version(name: &str) -> Option<Vec<u64>> {
    let prefix = format!("{EXTENSION_ID}-");
    if name.len() <= prefix.len() || !name[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        return None;
    }
    let version = name[prefix.len()..].split('-').next()?;
    let parts: Option<Vec<u64>> = version.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// Newest extension binary below one extensions root.
#[must_use]
pub fn newest_in_root(root: &Path) -> Option<(Vec<u64>, PathBuf)> {
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let version = folder_version(&name)?;
            let bin = binary_in_extension(&e.path())?;
            Some((version, bin))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

/// Validates a user-pinned path.
pub fn check_override(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_absolute() {
        return Err("the Claude Code path must be absolute".into());
    }
    if !p.is_file() {
        return Err(format!("{} does not exist or is not a file", p.display()));
    }
    Ok(p)
}

/// Runs discovery. Returns the binary and, when a pinned path was rejected, why.
pub fn discover(
    input: &DiscoveryInput,
    which: &dyn Fn(&str) -> Option<PathBuf>,
) -> (Option<Discovered>, Option<String>) {
    let mut override_error = None;
    if let Some(p) = input.override_path.as_deref().filter(|p| !p.trim().is_empty()) {
        match check_override(p) {
            Ok(path) => return (Some(Discovered { path, source: ClaudeBinarySource::Override }), None),
            Err(e) => override_error = Some(e),
        }
    }
    let vscode = input.vscode_extension_roots.iter().filter_map(|r| newest_in_root(r)).max_by(|a, b| a.0.cmp(&b.0));
    if let Some((_, path)) = vscode {
        return (Some(Discovered { path, source: ClaudeBinarySource::VscodeExtension }), override_error);
    }
    let native = input
        .native_extensions
        .iter()
        .filter(|(id, _)| id.eq_ignore_ascii_case(EXTENSION_ID))
        .find_map(|(_, dir)| binary_in_extension(dir));
    if let Some(path) = native {
        return (Some(Discovered { path, source: ClaudeBinarySource::NativeExtension }), override_error);
    }
    let found = which("claude").map(|path| Discovered { path, source: ClaudeBinarySource::Path });
    (found, override_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_ext(root: &Path, folder: &str) -> PathBuf {
        let dir = root.join(folder).join("resources").join("native-binary");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join(binary_file_name());
        std::fs::write(&bin, b"bin").unwrap();
        bin
    }

    #[test]
    fn versions_from_folder_names() {
        assert_eq!(folder_version("anthropic.claude-code-2.1.292-win32-x64"), Some(vec![2, 1, 292]));
        assert_eq!(folder_version("Anthropic.Claude-Code-1.0.9"), Some(vec![1, 0, 9]));
        assert_eq!(folder_version("anthropic.claude-codex-1.0.0"), None);
        assert_eq!(folder_version("anthropic.claude-code-"), None);
        assert_eq!(folder_version("ms-python.python-2024.1.0"), None);
    }

    #[test]
    fn newest_vscode_extension_wins_then_native_then_path() {
        let tmp = tempfile::tempdir().unwrap();
        let vs = tmp.path().join("vscode-ext");
        fake_ext(&vs, "anthropic.claude-code-2.0.9-win32-x64");
        let newest = fake_ext(&vs, "anthropic.claude-code-2.1.292-win32-x64");
        std::fs::create_dir_all(vs.join("anthropic.claude-code-9.9.9-broken")).unwrap(); // no binary
        let native_dir = tmp.path().join("native").join("anthropic.claude-code");
        let native_bin = fake_ext(&tmp.path().join("native"), "anthropic.claude-code");
        let path_bin = tmp.path().join("on-path-claude");
        let which = |n: &str| (n == "claude").then(|| path_bin.clone());

        let mut input = DiscoveryInput {
            override_path: None,
            vscode_extension_roots: vec![vs.clone(), tmp.path().join("missing")],
            native_extensions: vec![("other.ext".into(), tmp.path().into()), (EXTENSION_ID.into(), native_dir)],
        };
        let (d, err) = discover(&input, &which);
        assert_eq!(d, Some(Discovered { path: newest, source: ClaudeBinarySource::VscodeExtension }));
        assert!(err.is_none());

        input.vscode_extension_roots.clear();
        assert_eq!(
            discover(&input, &which).0,
            Some(Discovered { path: native_bin.clone(), source: ClaudeBinarySource::NativeExtension })
        );

        input.native_extensions.clear();
        assert_eq!(discover(&input, &which).0.unwrap().source, ClaudeBinarySource::Path);
        assert_eq!(discover(&input, &|_| None).0, None);

        // A valid pinned path wins; an invalid one is reported and ignored.
        input.override_path = Some(native_bin.display().to_string());
        assert_eq!(discover(&input, &which).0.unwrap().source, ClaudeBinarySource::Override);
        input.override_path = Some("relative/claude".into());
        let (d, err) = discover(&input, &which);
        assert_eq!(d.unwrap().source, ClaudeBinarySource::Path);
        assert!(err.unwrap().contains("absolute"));
    }
}

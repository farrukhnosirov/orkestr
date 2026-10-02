//! Path confinement (SR-21, contract §6).
//!
//! Every path that reaches the file service is a *workspace-relative* string
//! from the wire. It is first validated **lexically** by [`RelPath::parse`]
//! (no traversal, no absolute/UNC/device/verbatim forms, no alternate data
//! streams, no Windows-reserved names or aliasing trailing dots/spaces), then
//! resolved **physically** by [`WorkspaceRoot::resolve`], which canonicalizes
//! every existing component so symlinks and junctions that point outside the
//! root are rejected, and 8.3 short names are expanded to their long names
//! (so `GIT~1` cannot be used to reach the protected `.git` directory).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{FilesError, FilesResult};

/// Max length of a relative path (bytes).
pub const MAX_PATH_LEN: usize = 4096;
/// Max length of one path component (bytes).
pub const MAX_COMPONENT_LEN: usize = 255;

/// Directory names that the file service never modifies (reads are allowed).
pub const PROTECTED_DIRS: &[&str] = &[".git"];

const FORBIDDEN_CHARS: &[char] = &['<', '>', '"', '|', '?', '*', ':'];

/// A validated, normalized workspace-relative path (`/`-separated, `""` = root).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RelPath {
    components: Vec<String>,
}

impl RelPath {
    /// The workspace root.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    /// Parses and validates a wire path. `""`, `"."` and `"/"`-free forms
    /// are accepted; both `/` and `\` separate components.
    pub fn parse(input: &str) -> FilesResult<Self> {
        if input.len() > MAX_PATH_LEN {
            return Err(FilesError::invalid(input, "path is too long"));
        }
        if input.chars().any(|c| c.is_control()) {
            return Err(FilesError::invalid(input, "control characters are not allowed"));
        }
        if input.starts_with('/') || input.starts_with('\\') {
            return Err(FilesError::invalid(input, "absolute, UNC and device paths are not allowed"));
        }
        if input.contains(':') {
            return Err(FilesError::invalid(input, "drive letters and alternate data streams are not allowed"));
        }
        let mut components = Vec::new();
        for c in input.split(['/', '\\']) {
            if c.is_empty() || c == "." {
                continue;
            }
            if c == ".." {
                return Err(FilesError::invalid(input, "`..` is not allowed"));
            }
            validate_component(c, input)?;
            components.push(c.to_owned());
        }
        Ok(Self { components })
    }

    /// Builds from already-trusted components (e.g. a canonical path below the root).
    fn from_trusted(components: Vec<String>) -> Self {
        Self { components }
    }

    /// Whether this is the root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    /// Components.
    #[must_use]
    pub fn components(&self) -> &[String] {
        &self.components
    }

    /// Last component.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.components.last().map(String::as_str)
    }

    /// Parent (`None` for the root).
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(Self { components: self.components[..self.components.len() - 1].to_vec() })
    }

    /// Appends a validated component.
    pub fn join(&self, name: &str) -> FilesResult<Self> {
        let child = Self::parse(name)?;
        let mut components = self.components.clone();
        components.extend(child.components);
        Ok(Self { components })
    }

    /// Whether `self` equals `other` or lies below it.
    #[must_use]
    pub fn starts_with(&self, other: &Self) -> bool {
        self.components.len() >= other.components.len()
            && self.components.iter().zip(&other.components).all(|(a, b)| a == b)
    }

    /// Whether any component is a protected directory (case-insensitive).
    #[must_use]
    pub fn is_protected(&self) -> bool {
        self.components.iter().any(|c| PROTECTED_DIRS.iter().any(|p| c.eq_ignore_ascii_case(p)))
    }

    /// Wire form.
    #[must_use]
    pub fn as_string(&self) -> String {
        self.components.join("/")
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_string())
    }
}

fn validate_component(c: &str, input: &str) -> FilesResult<()> {
    if c.len() > MAX_COMPONENT_LEN {
        return Err(FilesError::invalid(input, "path component is too long"));
    }
    if c.chars().any(|ch| FORBIDDEN_CHARS.contains(&ch)) {
        return Err(FilesError::invalid(input, "characters < > \" | ? * : are not allowed"));
    }
    // Windows strips trailing dots and spaces, so `secret.txt.` aliases `secret.txt`
    // and `.git.` aliases `.git`.
    if c.ends_with('.') || c.ends_with(' ') {
        return Err(FilesError::invalid(input, "components must not end with a dot or space"));
    }
    if is_reserved_device_name(c) {
        return Err(FilesError::invalid(input, "reserved device names (CON, NUL, COM1, …) are not allowed"));
    }
    Ok(())
}

/// Windows reserved device names — the single implementation lives in the
/// security crate (`orkestr_security::paths`).
pub use orkestr_security::paths::is_reserved_device_name;

/// Strips the Windows verbatim prefix (`\\?\C:\x` → `C:\x`, `\\?\UNC\s\x` →
/// `\\s\x`) so external programs and messages get conventional paths.
#[must_use]
pub fn display_path(path: &Path) -> PathBuf {
    orkestr_security::paths::simplify(path)
}

/// A physical location inside the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Absolute path (below the canonical root).
    pub abs: PathBuf,
    /// Real relative path (canonical case / long names for existing parts).
    pub rel: RelPath,
    /// Whether something exists at `abs` (without following a final symlink when not requested).
    pub exists: bool,
}

/// A canonical, existing workspace root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

impl WorkspaceRoot {
    /// Opens an absolute, existing directory as a root.
    pub fn open(path: impl AsRef<Path>) -> FilesResult<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(FilesError::InvalidRoot("root must be an absolute path".into()));
        }
        let root = fs::canonicalize(path).map_err(|e| FilesError::InvalidRoot(format!("cannot open root: {e}")))?;
        if !root.is_dir() {
            return Err(FilesError::InvalidRoot("root is not a directory".into()));
        }
        Ok(Self { root })
    }

    /// Canonical root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Root folder name.
    #[must_use]
    pub fn name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Resolves `rel` physically.
    ///
    /// Every existing component is canonicalized and must stay below the
    /// root; the first missing component and everything after it are taken
    /// lexically (they cannot be links because they do not exist). With
    /// `follow_last = false` a final symlink is addressed itself (for delete
    /// and rename) instead of its target.
    pub fn resolve(&self, rel: &RelPath, follow_last: bool) -> FilesResult<Resolved> {
        let comps = rel.components();
        let mut cur = self.root.clone();
        let mut real: Vec<String> = Vec::with_capacity(comps.len());
        let mut exists = true;
        for (i, c) in comps.iter().enumerate() {
            let last = i + 1 == comps.len();
            let candidate = cur.join(c);
            match fs::symlink_metadata(&candidate) {
                Ok(meta) => {
                    if last && !follow_last && meta.file_type().is_symlink() {
                        cur = candidate;
                        real.push(c.clone());
                        break;
                    }
                    let canon = match fs::canonicalize(&candidate) {
                        Ok(p) => p,
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {
                            return Err(FilesError::invalid(&rel.as_string(), "dangling symbolic link"));
                        }
                        Err(e) => return Err(FilesError::io(rel.as_string(), e)),
                    };
                    real =
                        self.components_below_root(&canon).ok_or_else(|| FilesError::OutsideRoot(rel.as_string()))?;
                    cur = canon;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    exists = false;
                    for rest in &comps[i..] {
                        cur.push(rest);
                        real.push(rest.clone());
                    }
                    break;
                }
                Err(e) => return Err(FilesError::io(rel.as_string(), e)),
            }
        }
        Ok(Resolved { abs: cur, rel: RelPath::from_trusted(real), exists })
    }

    /// Relative path of an absolute path below the root (for watcher events).
    #[must_use]
    pub fn relative_of(&self, abs: &Path) -> Option<RelPath> {
        self.components_below_root(abs).map(RelPath::from_trusted)
    }

    fn components_below_root(&self, abs: &Path) -> Option<Vec<String>> {
        let rest = abs.strip_prefix(&self.root).ok()?;
        let mut out = Vec::new();
        for c in rest.components() {
            match c {
                Component::Normal(s) => out.push(s.to_str()?.to_owned()),
                Component::CurDir => {}
                _ => return None,
            }
        }
        Some(out)
    }

    /// Re-checks that an existing directory still lies below the root
    /// (narrows TOCTOU windows after creating parents).
    pub fn ensure_inside(&self, abs: &Path, label: &RelPath) -> FilesResult<()> {
        let canon = fs::canonicalize(abs).map_err(|e| FilesError::io(label.as_string(), e))?;
        if canon.starts_with(&self.root) {
            Ok(())
        } else {
            Err(FilesError::OutsideRoot(label.as_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> String {
        RelPath::parse(s).unwrap_or_else(|e| panic!("{s:?} should be accepted: {e}")).as_string()
    }

    fn rejected(s: &str) {
        assert!(RelPath::parse(s).is_err(), "{s:?} must be rejected");
    }

    #[test]
    fn accepts_normal_relative_paths() {
        assert_eq!(ok(""), "");
        assert_eq!(ok("."), "");
        assert_eq!(ok("src/main.rs"), "src/main.rs");
        assert_eq!(ok("src\\lib\\mod.rs"), "src/lib/mod.rs");
        assert_eq!(ok("./a//b/./c"), "a/b/c");
        assert_eq!(ok(".github/workflows/ci.yml"), ".github/workflows/ci.yml");
        assert_eq!(ok("dir/"), "dir");
        assert_eq!(ok("näme with spaces/файл.txt"), "näme with spaces/файл.txt");
        assert_eq!(ok("console.log"), "console.log");
        assert_eq!(ok("com10"), "com10");
        assert_eq!(ok("lpt"), "lpt");
        assert_eq!(ok("a..b"), "a..b");
        assert_eq!(ok("...hidden"), "...hidden");
        assert_eq!(ok("PROGRA~1"), "PROGRA~1"); // resolved physically, see below
    }

    #[test]
    fn rejects_traversal() {
        for s in ["..", "../x", "a/../../b", "a/..", "a\\..\\..\\b", "./../etc/passwd", "src/../../.."] {
            rejected(s);
        }
    }

    #[test]
    fn rejects_absolute_unc_device_and_verbatim_forms() {
        for s in [
            "/etc/passwd",
            "\\Windows\\System32",
            "C:\\Windows",
            "C:/Windows",
            "c:relative",
            "\\\\server\\share\\x",
            "//server/share/x",
            "\\\\?\\C:\\Windows",
            "\\\\.\\PhysicalDrive0",
            "\\\\?\\UNC\\server\\share",
            "//?/C:/x",
        ] {
            rejected(s);
        }
    }

    #[test]
    fn rejects_alternate_data_streams_and_forbidden_chars() {
        for s in ["a.txt:stream", "a.txt::$DATA", "dir/x:y", "a<b", "a>b", "a|b", "a?b", "a*b", "a\"b"] {
            rejected(s);
        }
    }

    #[test]
    fn rejects_reserved_device_names() {
        for s in [
            "CON",
            "con",
            "nul",
            "NUL.txt",
            "aux.tar.gz",
            "PRN",
            "COM1",
            "com9.log",
            "LPT1",
            "lpt3.txt",
            "src/CON",
            "CONIN$",
            "conout$",
            "COM¹",
            "CON .txt",
        ] {
            rejected(s);
        }
    }

    #[test]
    fn rejects_trailing_dots_spaces_control_chars_and_long_paths() {
        for s in ["secret.txt.", ".git.", "dir /x", "a ", "a\0b", "a\nb", "a\tb"] {
            rejected(s);
        }
        rejected(&"a/".repeat(3000));
        rejected(&"x".repeat(300));
    }

    #[test]
    fn protected_detection_is_case_insensitive() {
        assert!(RelPath::parse(".git/config").unwrap().is_protected());
        assert!(RelPath::parse("sub/.GIT/hooks/pre-commit").unwrap().is_protected());
        assert!(!RelPath::parse(".github/x").unwrap().is_protected());
        assert!(!RelPath::parse(".gitignore").unwrap().is_protected());
    }

    #[test]
    fn relpath_helpers() {
        let p = RelPath::parse("a/b/c.txt").unwrap();
        assert_eq!(p.name(), Some("c.txt"));
        assert_eq!(p.parent().unwrap().as_string(), "a/b");
        assert!(p.starts_with(&RelPath::parse("a").unwrap()));
        assert!(!p.starts_with(&RelPath::parse("a/bb").unwrap()));
        assert!(RelPath::root().parent().is_none());
        assert!(p.join("../x").is_err());
        assert_eq!(p.parent().unwrap().join("d").unwrap().as_string(), "a/b/d");
    }

    #[test]
    fn display_path_strips_verbatim_prefixes() {
        assert_eq!(display_path(Path::new(r"\\?\C:\work\x")), PathBuf::from(r"C:\work\x"));
        assert_eq!(display_path(Path::new(r"\\?\UNC\srv\share\x")), PathBuf::from(r"\\srv\share\x"));
        assert_eq!(display_path(Path::new("/home/u/x")), PathBuf::from("/home/u/x"));
    }

    #[test]
    fn root_must_be_absolute_existing_dir() {
        assert!(WorkspaceRoot::open("relative/dir").is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert!(WorkspaceRoot::open(&file).is_err());
        assert!(WorkspaceRoot::open(dir.path().join("missing")).is_err());
        assert!(WorkspaceRoot::open(dir.path()).is_ok());
    }

    #[test]
    fn resolves_existing_and_missing_paths_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/sub")).unwrap();
        fs::write(dir.path().join("src/a.rs"), b"x").unwrap();
        let root = WorkspaceRoot::open(dir.path()).unwrap();
        let r = root.resolve(&RelPath::parse("src/a.rs").unwrap(), true).unwrap();
        assert!(r.exists);
        assert!(r.abs.starts_with(root.path()));
        assert_eq!(r.rel.as_string(), "src/a.rs");
        let r = root.resolve(&RelPath::parse("src/new/deep/file.txt").unwrap(), true).unwrap();
        assert!(!r.exists);
        assert_eq!(r.rel.as_string(), "src/new/deep/file.txt");
        assert!(r.abs.starts_with(root.path()));
        let r = root.resolve(&RelPath::root(), true).unwrap();
        assert_eq!(r.abs, root.path());
        assert!(r.exists);
    }

    #[test]
    fn relative_of_maps_paths_below_root_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = WorkspaceRoot::open(dir.path()).unwrap();
        assert_eq!(root.relative_of(&root.path().join("a").join("b.txt")).unwrap().as_string(), "a/b.txt");
        assert!(root.relative_of(root.path().parent().unwrap()).is_none());
    }

    /// Creates a directory symlink (Unix) or junction-capable symlink (Windows, needs
    /// developer mode); returns false when the OS refuses so the test can skip.
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            // Fall back to a junction via mklink (no privilege needed).
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
    }

    #[test]
    fn symlink_or_junction_escape_is_rejected() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("escape");
        if !try_symlink_dir(outside.path(), &link) {
            eprintln!("skipping: cannot create directory links on this system");
            return;
        }
        let root = WorkspaceRoot::open(dir.path()).unwrap();
        for p in ["escape", "escape/secret.txt", "escape/new-file.txt", "escape/new/dir/x"] {
            let err = root.resolve(&RelPath::parse(p).unwrap(), true).unwrap_err();
            assert!(matches!(err, FilesError::OutsideRoot(_)), "{p}: {err}");
        }
        // The link itself can still be addressed (to delete it) without following it.
        let r = root.resolve(&RelPath::parse("escape").unwrap(), false).unwrap();
        assert!(r.exists);
        assert_eq!(r.rel.as_string(), "escape");
    }

    #[test]
    fn symlink_inside_root_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("real")).unwrap();
        fs::write(dir.path().join("real/f.txt"), b"x").unwrap();
        if !try_symlink_dir(&dir.path().join("real"), &dir.path().join("alias")) {
            eprintln!("skipping: cannot create directory links on this system");
            return;
        }
        let root = WorkspaceRoot::open(dir.path()).unwrap();
        let r = root.resolve(&RelPath::parse("alias/f.txt").unwrap(), true).unwrap();
        assert!(r.exists);
        assert_eq!(r.rel.as_string(), "real/f.txt", "canonical relative path is reported");
    }
}

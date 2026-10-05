//! Project toolchain detection (§17).
//!
//! Reads manifests in the workspace root and nested package roots (bounded
//! breadth-first walk) and proposes checks per root. Monorepo rules:
//!
//! * a Cargo `[workspace]`, a `.sln`, a Maven parent with `<modules>` or a
//!   Gradle `settings.gradle` covers nested projects of the same toolchain —
//!   they are not proposed again;
//! * nested packages of an npm/pnpm/yarn workspace are proposed but disabled
//!   (the workspace root scripts usually run them);
//! * other toolchains (Go modules, Python packages, …) are proposed per root.
//!
//! Detection is read-only and bounded (depth, directory count, manifest size).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use orkestr_protocol::methods::quality::{CheckDef, CheckKind, CheckSource, DetectedRoot, Detection, Toolchain};

/// Detection limits and platform hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectOptions {
    /// Host is Windows (wrapper scripts like `gradlew.bat`).
    pub windows: bool,
    /// Max directory depth below the root (root = 0).
    pub max_depth: usize,
    /// Max directories visited.
    pub max_dirs: usize,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self { windows: false, max_depth: 3, max_dirs: 4000 }
    }
}

/// Directories never descended into.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    "vendor",
    "bin",
    "obj",
    ".idea",
    ".vscode",
    "coverage",
    ".next",
    ".nuxt",
    ".turbo",
    ".gradle",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".orkestr",
    ".claude",
];

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Relative `/`-separated path of `dir` under `root` (`.` for the root).
fn rel(root: &Path, dir: &Path) -> String {
    let r = dir.strip_prefix(root).unwrap_or(dir);
    let s: Vec<String> = r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if s.is_empty() {
        ".".into()
    } else {
        s.join("/")
    }
}

fn join_rel(dir: &str, file: &str) -> String {
    if dir == "." {
        file.to_owned()
    } else {
        format!("{dir}/{file}")
    }
}

/// Whether `child` (relative) is inside `parent` (relative, `.` = root).
fn is_under(child: &str, parent: &str) -> bool {
    parent == "." || child == parent || child.starts_with(&format!("{parent}/"))
}

#[derive(Default)]
struct Ctx {
    checks: Vec<CheckDef>,
    roots: BTreeMap<String, DetectedRoot>,
    notes: Vec<String>,
    /// Roots that cover nested projects, per toolchain family.
    cargo_workspaces: Vec<String>,
    js_workspaces: Vec<String>,
    dotnet_solutions: Vec<String>,
    maven_parents: Vec<String>,
    gradle_roots: Vec<String>,
}

impl Ctx {
    fn root(&mut self, dir: &str, tc: Toolchain, manifest: &str) {
        let entry = self.roots.entry(dir.to_owned()).or_insert_with(|| DetectedRoot {
            path: dir.to_owned(),
            toolchains: Vec::new(),
            manifests: Vec::new(),
        });
        if !entry.toolchains.contains(&tc) {
            entry.toolchains.push(tc);
        }
        let m = join_rel(dir, manifest);
        if !entry.manifests.contains(&m) {
            entry.manifests.push(m);
        }
    }

    fn add(&mut self, dir: &str, tc: Toolchain, kind: CheckKind, command: String, enabled: bool) {
        let base = format!("{}:{}:{}", toolchain_slug(tc), kind.as_str(), dir);
        let mut id = base.clone();
        let mut n = 2;
        while self.checks.iter().any(|c| c.id == id) {
            id = format!("{base}#{n}");
            n += 1;
        }
        let label = if dir == "." { command.clone() } else { format!("{command} ({dir})") };
        self.checks.push(CheckDef {
            id,
            kind,
            toolchain: tc,
            label,
            command,
            cwd: dir.to_owned(),
            enabled,
            timeout_secs: None,
            source: CheckSource::Detected,
        });
    }

    fn has_kind_in(&self, dir: &str, kind: CheckKind) -> bool {
        self.checks.iter().any(|c| c.cwd == dir && c.kind == kind)
    }
}

/// Stable short name of a toolchain (used in check ids).
#[must_use]
pub fn toolchain_slug(tc: Toolchain) -> &'static str {
    match tc {
        Toolchain::Cargo => "cargo",
        Toolchain::Npm => "npm",
        Toolchain::Pnpm => "pnpm",
        Toolchain::Yarn => "yarn",
        Toolchain::Python => "python",
        Toolchain::Go => "go",
        Toolchain::Dotnet => "dotnet",
        Toolchain::Maven => "maven",
        Toolchain::Gradle => "gradle",
        Toolchain::Make => "make",
        Toolchain::Custom => "custom",
    }
}

/// Detects toolchains under `root`.
#[must_use]
pub fn detect(root: &Path, opts: DetectOptions) -> Detection {
    let mut ctx = Ctx::default();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        visited += 1;
        if visited > opts.max_dirs {
            ctx.notes.push(format!("stopped after {} directories", opts.max_dirs));
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            if depth == 0 {
                ctx.notes.push(format!("cannot read {}", dir.display()));
            }
            continue;
        };
        let mut files: Vec<String> = Vec::new();
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if depth < opts.max_depth && !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    subdirs.push(entry.path());
                }
            } else if ft.is_file() {
                files.push(name);
            }
        }
        files.sort();
        subdirs.sort();
        let rdir = rel(root, &dir);
        detect_dir(&mut ctx, &dir, &rdir, &files, opts);
        for s in subdirs {
            queue.push_back((s, depth + 1));
        }
    }
    let mut roots: Vec<DetectedRoot> = ctx.roots.into_values().collect();
    roots.sort_by_key(|r| (r.path != ".", r.path.matches('/').count(), r.path.clone()));
    let mut checks = ctx.checks;
    // Root first, then by kind order (stable within).
    checks.sort_by_key(|c| (c.cwd != ".", c.cwd.matches('/').count(), c.cwd.clone(), c.kind));
    Detection { roots, checks, notes: ctx.notes }
}

fn detect_dir(ctx: &mut Ctx, dir: &Path, rdir: &str, files: &[String], opts: DetectOptions) {
    let has = |n: &str| files.iter().any(|f| f == n);
    if has("Cargo.toml") {
        detect_cargo(ctx, dir, rdir);
    }
    if has("package.json") {
        detect_js(ctx, dir, rdir, files);
    }
    if has("pyproject.toml")
        || has("setup.py")
        || has("setup.cfg")
        || has("requirements.txt")
        || has("Pipfile")
        || has("pytest.ini")
        || has("tox.ini")
    {
        detect_python(ctx, dir, rdir, files);
    }
    if has("go.mod") {
        ctx.root(rdir, Toolchain::Go, "go.mod");
        ctx.add(rdir, Toolchain::Go, CheckKind::Build, "go build ./...".into(), true);
        ctx.add(rdir, Toolchain::Go, CheckKind::Lint, "go vet ./...".into(), true);
        ctx.add(rdir, Toolchain::Go, CheckKind::Test, "go test ./...".into(), true);
    }
    detect_dotnet(ctx, rdir, files);
    if has("pom.xml") {
        detect_maven(ctx, dir, rdir);
    }
    if has("build.gradle") || has("build.gradle.kts") || has("settings.gradle") || has("settings.gradle.kts") {
        detect_gradle(ctx, rdir, files, opts);
    }
    if has("Makefile") || has("makefile") || has("GNUmakefile") {
        let name = ["GNUmakefile", "makefile", "Makefile"].into_iter().find(|n| has(n)).unwrap_or("Makefile");
        detect_make(ctx, &dir.join(name), rdir, name);
    }
}

fn detect_cargo(ctx: &mut Ctx, dir: &Path, rdir: &str) {
    if ctx.cargo_workspaces.iter().any(|w| is_under(rdir, w)) {
        return;
    }
    let Some(text) = read_small(&dir.join("Cargo.toml")) else {
        ctx.notes.push(format!("cannot read {}", join_rel(rdir, "Cargo.toml")));
        return;
    };
    let workspace = text.lines().any(|l| l.trim() == "[workspace]");
    let has_package = text.lines().any(|l| l.trim() == "[package]");
    if !workspace && !has_package {
        return;
    }
    if workspace {
        ctx.cargo_workspaces.push(rdir.to_owned());
    }
    ctx.root(rdir, Toolchain::Cargo, "Cargo.toml");
    let ws = if workspace { " --workspace" } else { "" };
    ctx.add(rdir, Toolchain::Cargo, CheckKind::Build, format!("cargo build{ws}"), true);
    ctx.add(rdir, Toolchain::Cargo, CheckKind::Lint, format!("cargo clippy{ws} --all-targets -- -D warnings"), true);
    ctx.add(rdir, Toolchain::Cargo, CheckKind::Format, "cargo fmt --all -- --check".into(), true);
    ctx.add(rdir, Toolchain::Cargo, CheckKind::Test, format!("cargo test{ws}"), true);
}

/// `package.json` script names per kind, in preference order.
const JS_SCRIPTS: &[(CheckKind, &[&str])] = &[
    (CheckKind::Build, &["build"]),
    (CheckKind::Typecheck, &["typecheck", "type-check", "check-types", "check:types", "types", "tsc"]),
    (CheckKind::Lint, &["lint", "lint:check", "eslint"]),
    (CheckKind::Format, &["format:check", "fmt:check", "check:format", "format-check", "prettier:check"]),
    (CheckKind::Test, &["test", "test:unit", "unit"]),
    (CheckKind::E2e, &["e2e", "test:e2e", "e2e:test", "playwright", "cypress"]),
    (CheckKind::Smoke, &["smoke", "test:smoke"]),
];

fn detect_js(ctx: &mut Ctx, dir: &Path, rdir: &str, files: &[String]) {
    let has = |n: &str| files.iter().any(|f| f == n);
    let Some(text) = read_small(&dir.join("package.json")) else {
        ctx.notes.push(format!("cannot read {}", join_rel(rdir, "package.json")));
        return;
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            ctx.notes.push(format!("invalid {}: {e}", join_rel(rdir, "package.json")));
            return;
        }
    };
    let inside_workspace = ctx.js_workspaces.iter().any(|w| w != rdir && is_under(rdir, w));
    let is_workspace_root = json.get("workspaces").is_some() || has("pnpm-workspace.yaml");
    if is_workspace_root {
        ctx.js_workspaces.push(rdir.to_owned());
    }
    // Package manager: own lockfile, else the workspace root's, else npm.
    let tc = if has("pnpm-lock.yaml") || has("pnpm-workspace.yaml") {
        Toolchain::Pnpm
    } else if has("yarn.lock") {
        Toolchain::Yarn
    } else if inside_workspace {
        ctx.roots
            .values()
            .filter(|r| ctx.js_workspaces.iter().any(|w| w == &r.path) && is_under(rdir, &r.path))
            .flat_map(|r| r.toolchains.iter().copied())
            .find(|t| matches!(t, Toolchain::Npm | Toolchain::Pnpm | Toolchain::Yarn))
            .unwrap_or(Toolchain::Npm)
    } else {
        Toolchain::Npm
    };
    ctx.root(rdir, tc, "package.json");
    let runner = match tc {
        Toolchain::Pnpm => "pnpm run",
        Toolchain::Yarn => "yarn run",
        _ => "npm run",
    };
    let scripts = json.get("scripts").and_then(|s| s.as_object());
    let enabled = !inside_workspace;
    for (kind, names) in JS_SCRIPTS {
        let Some(scripts) = scripts else { break };
        let found = names
            .iter()
            .find(|n| scripts.get(**n).and_then(|v| v.as_str()).is_some_and(|body| !is_placeholder_test_script(body)));
        if let Some(name) = found {
            ctx.add(rdir, tc, *kind, format!("{runner} {name}"), enabled);
        }
    }
    let deps_mention = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|k| json.get(*k).and_then(|d| d.as_object()).is_some_and(|d| d.contains_key(name)))
    };
    if !ctx.has_kind_in(rdir, CheckKind::Typecheck) && has("tsconfig.json") && deps_mention("typescript") {
        ctx.add(rdir, tc, CheckKind::Typecheck, "npx tsc --noEmit".into(), enabled);
    }
}

fn is_placeholder_test_script(body: &str) -> bool {
    body.contains("no test specified")
}

fn detect_python(ctx: &mut Ctx, dir: &Path, rdir: &str, files: &[String]) {
    let has = |n: &str| files.iter().any(|f| f == n);
    let manifest = ["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile", "pytest.ini", "tox.ini"]
        .into_iter()
        .find(|n| has(n))
        .unwrap_or("pyproject.toml");
    let mut text = String::new();
    for f in files {
        let lower = f.to_ascii_lowercase();
        if lower == "pyproject.toml"
            || lower == "setup.cfg"
            || lower == "tox.ini"
            || lower == "pipfile"
            || (lower.starts_with("requirements") && lower.ends_with(".txt"))
        {
            if let Some(t) = read_small(&dir.join(f)) {
                text.push_str(&t);
                text.push('\n');
            }
        }
    }
    let lower = text.to_ascii_lowercase();
    ctx.root(rdir, Toolchain::Python, manifest);
    let tests_dir = dir.join("tests").is_dir() || dir.join("test").is_dir();
    if lower.contains("pytest") || has("pytest.ini") || has("conftest.py") || tests_dir {
        ctx.add(rdir, Toolchain::Python, CheckKind::Test, "python -m pytest".into(), true);
    }
    let ruff = lower.contains("[tool.ruff") || has("ruff.toml") || has(".ruff.toml") || lower.contains("ruff");
    if ruff {
        ctx.add(rdir, Toolchain::Python, CheckKind::Lint, "ruff check .".into(), true);
    } else if has(".flake8") || lower.contains("[flake8]") || lower.contains("flake8") {
        ctx.add(rdir, Toolchain::Python, CheckKind::Lint, "flake8".into(), true);
    }
    if lower.contains("[tool.mypy") || has("mypy.ini") || has(".mypy.ini") || lower.contains("[mypy") {
        ctx.add(rdir, Toolchain::Python, CheckKind::Typecheck, "mypy .".into(), true);
    }
    if lower.contains("[tool.black") {
        ctx.add(rdir, Toolchain::Python, CheckKind::Format, "black --check .".into(), true);
    } else if lower.contains("[tool.ruff.format") {
        ctx.add(rdir, Toolchain::Python, CheckKind::Format, "ruff format --check .".into(), true);
    }
}

fn detect_dotnet(ctx: &mut Ctx, rdir: &str, files: &[String]) {
    let sln = files.iter().find(|f| f.to_ascii_lowercase().ends_with(".sln"));
    let proj = files.iter().find(|f| {
        let l = f.to_ascii_lowercase();
        l.ends_with(".csproj") || l.ends_with(".fsproj") || l.ends_with(".vbproj")
    });
    let Some(manifest) = sln.or(proj) else { return };
    if ctx.dotnet_solutions.iter().any(|s| is_under(rdir, s)) {
        return;
    }
    if sln.is_some() {
        ctx.dotnet_solutions.push(rdir.to_owned());
    }
    let manifest = manifest.clone();
    ctx.root(rdir, Toolchain::Dotnet, &manifest);
    ctx.add(rdir, Toolchain::Dotnet, CheckKind::Build, "dotnet build".into(), true);
    ctx.add(rdir, Toolchain::Dotnet, CheckKind::Format, "dotnet format --verify-no-changes".into(), false);
    ctx.add(rdir, Toolchain::Dotnet, CheckKind::Test, "dotnet test".into(), true);
}

fn detect_maven(ctx: &mut Ctx, dir: &Path, rdir: &str) {
    if ctx.maven_parents.iter().any(|p| p != rdir && is_under(rdir, p)) {
        return;
    }
    let text = read_small(&dir.join("pom.xml")).unwrap_or_default();
    if text.contains("<modules>") {
        ctx.maven_parents.push(rdir.to_owned());
    }
    ctx.root(rdir, Toolchain::Maven, "pom.xml");
    ctx.add(rdir, Toolchain::Maven, CheckKind::Build, "mvn -B -q compile".into(), true);
    ctx.add(rdir, Toolchain::Maven, CheckKind::Test, "mvn -B test".into(), true);
}

fn detect_gradle(ctx: &mut Ctx, rdir: &str, files: &[String], opts: DetectOptions) {
    if ctx.gradle_roots.iter().any(|p| p != rdir && is_under(rdir, p)) {
        return;
    }
    let has = |n: &str| files.iter().any(|f| f == n);
    if has("settings.gradle") || has("settings.gradle.kts") {
        ctx.gradle_roots.push(rdir.to_owned());
    }
    let manifest = ["build.gradle.kts", "build.gradle", "settings.gradle.kts", "settings.gradle"]
        .into_iter()
        .find(|n| has(n))
        .unwrap_or("build.gradle");
    ctx.root(rdir, Toolchain::Gradle, manifest);
    let g = if has("gradlew") || has("gradlew.bat") {
        if opts.windows {
            ".\\gradlew.bat"
        } else {
            "./gradlew"
        }
    } else {
        "gradle"
    };
    ctx.add(rdir, Toolchain::Gradle, CheckKind::Build, format!("{g} assemble"), true);
    ctx.add(rdir, Toolchain::Gradle, CheckKind::Test, format!("{g} test"), true);
}

/// Makefile targets per kind, in preference order.
const MAKE_TARGETS: &[(CheckKind, &[&str])] = &[
    (CheckKind::Build, &["build", "all"]),
    (CheckKind::Typecheck, &["typecheck", "types"]),
    (CheckKind::Lint, &["lint"]),
    (CheckKind::Format, &["fmt-check", "format-check", "check-format", "fmtcheck"]),
    (CheckKind::Test, &["test", "check"]),
    (CheckKind::E2e, &["e2e", "test-e2e"]),
    (CheckKind::Smoke, &["smoke"]),
];

/// Parses rule targets (`name:` at line start, not variable assignments).
#[must_use]
pub fn makefile_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else { continue };
        if rest.starts_with('=') {
            continue; // `X := value`
        }
        for t in head.split_whitespace() {
            if !t.is_empty()
                && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
                && !out.iter().any(|o: &String| o == t)
            {
                out.push(t.to_owned());
            }
        }
    }
    out
}

fn detect_make(ctx: &mut Ctx, path: &Path, rdir: &str, name: &str) {
    let Some(text) = read_small(path) else { return };
    let targets = makefile_targets(&text);
    let mut added = false;
    for (kind, names) in MAKE_TARGETS {
        if ctx.has_kind_in(rdir, *kind) {
            continue;
        }
        if let Some(t) = names.iter().find(|n| targets.iter().any(|x| x == **n)) {
            ctx.add(rdir, Toolchain::Make, *kind, format!("make {t}"), true);
            added = true;
        }
    }
    if added {
        ctx.root(rdir, Toolchain::Make, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn cmds(d: &Detection) -> Vec<(String, CheckKind, String, bool)> {
        d.checks.iter().map(|c| (c.cwd.clone(), c.kind, c.command.clone(), c.enabled)).collect()
    }

    #[test]
    fn cargo_workspace_covers_members() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        write(t.path(), "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
        let d = detect(t.path(), DetectOptions::default());
        let c = cmds(&d);
        assert!(c.contains(&(".".into(), CheckKind::Build, "cargo build --workspace".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Test, "cargo test --workspace".into(), true)));
        assert!(c.contains(&(
            ".".into(),
            CheckKind::Lint,
            "cargo clippy --workspace --all-targets -- -D warnings".into(),
            true
        )));
        assert!(c.iter().all(|x| x.0 == "."), "members are covered by the workspace: {c:?}");
        assert_eq!(d.roots.len(), 1);
    }

    #[test]
    fn single_crate() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        let d = detect(t.path(), DetectOptions::default());
        assert!(cmds(&d).contains(&(".".into(), CheckKind::Test, "cargo test".into(), true)));
    }

    #[test]
    fn npm_scripts_and_monorepo() {
        let t = tempfile::tempdir().unwrap();
        write(
            t.path(),
            "package.json",
            r#"{"workspaces":["apps/*"],"scripts":{"build":"vite build","test":"vitest run","typecheck":"tsc -b","lint":"eslint .","e2e":"playwright test"}}"#,
        );
        write(t.path(), "package-lock.json", "{}");
        write(t.path(), "apps/web/package.json", r#"{"scripts":{"test":"vitest","build":"vite build"}}"#);
        let d = detect(t.path(), DetectOptions::default());
        let c = cmds(&d);
        assert!(c.contains(&(".".into(), CheckKind::Build, "npm run build".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Typecheck, "npm run typecheck".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Lint, "npm run lint".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Test, "npm run test".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::E2e, "npm run e2e".into(), true)));
        // Nested workspace package: proposed but disabled.
        assert!(c.contains(&("apps/web".into(), CheckKind::Test, "npm run test".into(), false)));
        assert_eq!(d.roots.len(), 2);
        assert_eq!(d.roots[0].path, ".");
    }

    #[test]
    fn pnpm_yarn_and_placeholder_test() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "a/package.json", r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#);
        write(t.path(), "a/pnpm-lock.yaml", "");
        write(t.path(), "b/package.json", r#"{"scripts":{"lint":"eslint"},"devDependencies":{"typescript":"5"}}"#);
        write(t.path(), "b/yarn.lock", "");
        write(t.path(), "b/tsconfig.json", "{}");
        let d = detect(t.path(), DetectOptions::default());
        let c = cmds(&d);
        assert!(!c.iter().any(|x| x.0 == "a" && x.1 == CheckKind::Test), "placeholder test skipped");
        assert!(c.contains(&("b".into(), CheckKind::Lint, "yarn run lint".into(), true)));
        assert!(c.contains(&("b".into(), CheckKind::Typecheck, "npx tsc --noEmit".into(), true)));
        let a = d.roots.iter().find(|r| r.path == "a").unwrap();
        assert_eq!(a.toolchains, vec![Toolchain::Pnpm]);
    }

    #[test]
    fn python_go_dotnet_java_make() {
        let t = tempfile::tempdir().unwrap();
        write(
            t.path(),
            "py/pyproject.toml",
            "[tool.ruff]\nline-length=100\n[tool.mypy]\nstrict=true\n[tool.pytest.ini_options]\n",
        );
        write(t.path(), "go/go.mod", "module x\n");
        write(t.path(), "net/App.sln", "");
        write(t.path(), "net/src/App/App.csproj", "<Project/>");
        write(t.path(), "java/pom.xml", "<project><modules><module>core</module></modules></project>");
        write(t.path(), "java/core/pom.xml", "<project/>");
        write(t.path(), "gr/build.gradle.kts", "");
        write(t.path(), "gr/settings.gradle.kts", "");
        write(t.path(), "gr/gradlew", "");
        write(
            t.path(),
            "Makefile",
            "VAR := 1\n.PHONY: test\nall: build\nbuild:\n\techo\ntest lint:\n\techo\nsmoke:\n\techo\n",
        );
        let d = detect(t.path(), DetectOptions { windows: true, ..DetectOptions::default() });
        let c = cmds(&d);
        assert!(c.contains(&("py".into(), CheckKind::Test, "python -m pytest".into(), true)));
        assert!(c.contains(&("py".into(), CheckKind::Lint, "ruff check .".into(), true)));
        assert!(c.contains(&("py".into(), CheckKind::Typecheck, "mypy .".into(), true)));
        assert!(c.contains(&("go".into(), CheckKind::Test, "go test ./...".into(), true)));
        assert!(c.contains(&("go".into(), CheckKind::Lint, "go vet ./...".into(), true)));
        assert!(c.contains(&("net".into(), CheckKind::Build, "dotnet build".into(), true)));
        assert!(!c.iter().any(|x| x.0.starts_with("net/")), "solution covers projects");
        assert!(c.contains(&("java".into(), CheckKind::Test, "mvn -B test".into(), true)));
        assert!(!c.iter().any(|x| x.0 == "java/core"), "maven parent covers modules");
        assert!(c.contains(&("gr".into(), CheckKind::Test, ".\\gradlew.bat test".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Build, "make build".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Test, "make test".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Lint, "make lint".into(), true)));
        assert!(c.contains(&(".".into(), CheckKind::Smoke, "make smoke".into(), true)));
        // ids are unique
        let mut ids: Vec<_> = d.checks.iter().map(|c| c.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), d.checks.len());
    }

    #[test]
    fn make_only_fills_missing_kinds() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "Cargo.toml", "[package]\nname=\"x\"\n");
        write(t.path(), "Makefile", "test:\n\tcargo test\ne2e:\n\t./e2e.sh\n");
        let d = detect(t.path(), DetectOptions::default());
        let c = cmds(&d);
        assert!(!c.iter().any(|x| x.2 == "make test"));
        assert!(c.contains(&(".".into(), CheckKind::E2e, "make e2e".into(), true)));
    }

    #[test]
    fn skips_heavy_dirs_and_handles_empty() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "node_modules/x/package.json", r#"{"scripts":{"test":"x"}}"#);
        write(t.path(), "target/Cargo.toml", "[package]\n");
        let d = detect(t.path(), DetectOptions::default());
        assert!(d.checks.is_empty());
        assert!(d.roots.is_empty());
        let missing = detect(&t.path().join("nope"), DetectOptions::default());
        assert!(!missing.notes.is_empty());
    }

    #[test]
    fn makefile_target_parser() {
        let t = makefile_targets("X = 1\nY := 2\n.PHONY: a\na b: c\n\tcmd\n# c:\nd:: e\n");
        assert_eq!(t, vec!["a", "b", "d"]);
    }
}

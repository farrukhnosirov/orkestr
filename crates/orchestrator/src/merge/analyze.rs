//! Analysis of agent branches: semantic diff (files, hunks, symbols, risk
//! flags), dependency ordering and the recommendation. The pure parts are
//! unit-tested; [`semantic_diff`] reads git through `orkestr-vcs`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use orkestr_context::{extract_symbols, Language, Symbol};
use orkestr_protocol::methods::merge::{
    MergeBlocker, MergeBranch, MergeFileSummary, MergeRecommendation, MergeRisk, MergeRiskFlag, MergeSemanticDiff,
    MergeSymbolChange, SymbolChange,
};
use orkestr_protocol::methods::vcs::{DiffTarget, FileChangeStatus};
use orkestr_vcs::{BlobText, DiffRequest, GitCli, VcsResult};

/// Max files whose symbols are compared per branch.
pub const MAX_SYMBOL_FILES: usize = 40;
/// Max size of a file read for symbol extraction.
pub const MAX_SYMBOL_FILE_BYTES: usize = 512 * 1024;
/// Hunk headers kept per file.
pub const MAX_HUNK_HEADERS: usize = 6;
/// Changed lines above which a branch is a large change.
pub const LARGE_CHANGE_LINES: u32 = 1000;

const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "go.sum",
    "Gemfile.lock",
    "composer.lock",
    "packages.lock.json",
    "flake.lock",
];

const CONFIG_FILES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "go.mod",
    "Gemfile",
    "composer.json",
    "Dockerfile",
    "Makefile",
    "rust-toolchain.toml",
    ".npmrc",
];

const SECURITY_WORDS: &[&str] = &["auth", "security", "secret", "crypto", "permission", "credential", "password", "token"];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Risk flags of one path (without symbol information).
#[must_use]
pub fn path_flags(path: &str) -> Vec<MergeRiskFlag> {
    let mut out = Vec::new();
    let lower = path.to_ascii_lowercase();
    let name = file_name(path);
    let lname = name.to_ascii_lowercase();
    if lower.contains("migration") || lname.ends_with(".sql") {
        out.push(MergeRiskFlag::Migration);
    }
    if LOCKFILES.contains(&name) {
        out.push(MergeRiskFlag::Lockfile);
    }
    if CONFIG_FILES.contains(&name)
        || lname.starts_with("tsconfig")
        || lname.starts_with("docker-compose")
        || lname.starts_with(".env")
        || lname.contains(".config.")
        || lname == "vite.config.ts"
    {
        out.push(MergeRiskFlag::Config);
    }
    if lower.starts_with(".github/workflows/")
        || lower.starts_with(".gitlab-ci")
        || lower.starts_with(".circleci/")
        || lname == "azure-pipelines.yml"
        || lname == "jenkinsfile"
    {
        out.push(MergeRiskFlag::Ci);
    }
    let segments: Vec<&str> = lower.split(['/', '.', '_', '-']).collect();
    if segments.iter().any(|s| SECURITY_WORDS.iter().any(|w| s.starts_with(w))) {
        out.push(MergeRiskFlag::Security);
    }
    out
}

/// Counts hunks and keeps the first headers (`@@ … @@ <context>`).
#[must_use]
pub fn hunk_summary(patch: &str) -> (u32, Vec<String>) {
    let mut n = 0u32;
    let mut headers = Vec::new();
    for line in patch.lines().filter(|l| l.starts_with("@@")) {
        n += 1;
        if headers.len() < MAX_HUNK_HEADERS {
            let h: String = line.chars().take(160).collect();
            headers.push(h);
        }
    }
    (n, headers)
}

/// Heuristic: does the declaration on `line` look public?
#[must_use]
pub fn looks_public(language: Language, name: &str, line: &str) -> bool {
    let t = line.trim_start();
    match language {
        Language::Rust => t.starts_with("pub "),
        Language::TypeScript | Language::JavaScript => t.starts_with("export "),
        Language::Java | Language::CSharp => t.starts_with("public ") || t.contains(" public "),
        Language::Go => name.chars().next().is_some_and(char::is_uppercase),
        Language::Python => !name.starts_with('_'),
        Language::Markdown | Language::Other => false,
    }
}

struct Decl {
    kind: String,
    name: String,
    decl: String,
    body: String,
}

fn decls(language: Language, text: &str) -> Vec<Decl> {
    let lines: Vec<&str> = text.lines().collect();
    let mut syms: Vec<Symbol> = extract_symbols(language, text);
    syms.sort_by_key(|s| s.line);
    let mut out = Vec::with_capacity(syms.len());
    for (i, s) in syms.iter().enumerate() {
        let start = (s.line as usize).saturating_sub(1).min(lines.len());
        let end = syms.get(i + 1).map_or(lines.len(), |n| (n.line as usize).saturating_sub(1)).clamp(start, lines.len());
        let body = lines[start..end].iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
        out.push(Decl {
            kind: format!("{:?}", s.kind),
            name: s.name.clone(),
            decl: lines.get(start).map(|l| l.trim().to_owned()).unwrap_or_default(),
            body,
        });
    }
    out
}

/// Symbols added / changed / removed between two versions of a file.
#[must_use]
pub fn symbol_changes(path: &str, old: Option<&str>, new: Option<&str>) -> Vec<MergeSymbolChange> {
    let language = Language::from_path(path);
    if matches!(language, Language::Other | Language::Markdown) {
        return Vec::new();
    }
    let key = |d: &Decl, n: usize| (d.kind.clone(), d.name.clone(), n);
    let index = |list: Vec<Decl>| {
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        let mut map = BTreeMap::new();
        for d in list {
            let n = seen.entry((d.kind.clone(), d.name.clone())).or_insert(0);
            *n += 1;
            map.insert(key(&d, *n), d);
        }
        map
    };
    let before = index(old.map(|t| decls(language, t)).unwrap_or_default());
    let after = index(new.map(|t| decls(language, t)).unwrap_or_default());
    let mut out = Vec::new();
    for (k, d) in &after {
        match before.get(k) {
            None => out.push(MergeSymbolChange {
                path: path.to_owned(),
                name: d.name.clone(),
                kind: d.kind.clone(),
                change: SymbolChange::Added,
                public: looks_public(language, &d.name, &d.decl),
                signature_changed: false,
            }),
            Some(b) if b.body != d.body => out.push(MergeSymbolChange {
                path: path.to_owned(),
                name: d.name.clone(),
                kind: d.kind.clone(),
                change: SymbolChange::Changed,
                public: looks_public(language, &d.name, &d.decl) || looks_public(language, &b.name, &b.decl),
                signature_changed: b.decl != d.decl,
            }),
            Some(_) => {}
        }
    }
    for (k, b) in &before {
        if !after.contains_key(k) {
            out.push(MergeSymbolChange {
                path: path.to_owned(),
                name: b.name.clone(),
                kind: b.kind.clone(),
                change: SymbolChange::Removed,
                public: looks_public(language, &b.name, &b.decl),
                signature_changed: false,
            });
        }
    }
    out
}

/// Risk list of a diff.
#[must_use]
pub fn risks(files: &[MergeFileSummary], symbols: &[MergeSymbolChange]) -> Vec<MergeRisk> {
    let mut map: BTreeMap<MergeRiskFlag, BTreeSet<String>> = BTreeMap::new();
    let mut total = 0u32;
    for f in files {
        total = total.saturating_add(f.additions).saturating_add(f.deletions);
        for flag in path_flags(&f.path) {
            map.entry(flag).or_default().insert(f.path.clone());
        }
        if f.status == FileChangeStatus::Deleted {
            map.entry(MergeRiskFlag::Deletion).or_default().insert(f.path.clone());
        }
        if f.binary {
            map.entry(MergeRiskFlag::Binary).or_default().insert(f.path.clone());
        }
    }
    for s in symbols.iter().filter(|s| s.public) {
        if s.change == SymbolChange::Removed || s.signature_changed {
            map.entry(MergeRiskFlag::PublicApi).or_default().insert(format!("{}::{}", s.path, s.name));
        }
    }
    if total > LARGE_CHANGE_LINES {
        map.entry(MergeRiskFlag::LargeChange).or_default().insert(total.to_string());
    }
    map.into_iter().map(|(flag, items)| MergeRisk { flag, items: items.into_iter().take(20).collect() }).collect()
}

async fn text_at(git: &GitCli, dir: &Path, rev: &str, path: &str) -> Option<String> {
    match git.blob_at(dir, rev, path, MAX_SYMBOL_FILE_BYTES).await {
        Ok(BlobText::Text(t)) => Some(t),
        _ => None,
    }
}

/// Semantic diff of `commit` against `base` (the merge base with the target).
pub async fn semantic_diff(git: &GitCli, dir: &Path, base: &str, commit: &str) -> VcsResult<MergeSemanticDiff> {
    let req = DiffRequest {
        target: DiffTarget::Range { from: base.to_owned(), to: commit.to_owned() },
        paths: Vec::new(),
        context_lines: 0,
        include_untracked: false,
        max_patch_bytes: 64 * 1024,
    };
    let d = git.diff(dir, &req).await?;
    let mut out = MergeSemanticDiff { additions: d.additions, deletions: d.deletions, ..MergeSemanticDiff::default() };
    let mut analysed = 0usize;
    for f in &d.files {
        let (hunks, hunk_headers) = hunk_summary(&f.patch);
        out.files.push(MergeFileSummary {
            path: f.path.clone(),
            old_path: f.old_path.clone(),
            status: f.status,
            additions: f.additions,
            deletions: f.deletions,
            hunks,
            hunk_headers,
            binary: f.binary,
        });
        let lang = Language::from_path(&f.path);
        if f.binary || matches!(lang, Language::Other | Language::Markdown) {
            continue;
        }
        if analysed >= MAX_SYMBOL_FILES {
            out.truncated = true;
            continue;
        }
        analysed += 1;
        let old_path = f.old_path.as_deref().unwrap_or(&f.path);
        let old = if f.status == FileChangeStatus::Added { None } else { text_at(git, dir, base, old_path).await };
        let new = if f.status == FileChangeStatus::Deleted { None } else { text_at(git, dir, commit, &f.path).await };
        out.symbols.extend(symbol_changes(&f.path, old.as_deref(), new.as_deref()));
    }
    out.symbols.truncate(500);
    out.risks = risks(&out.files, &out.symbols);
    Ok(out)
}

/// Topological order of `branches` (stable: input order breaks ties).
/// `deps[b]` lists branches that must merge before `b` (unknown names are
/// ignored). Errors with the branches of a cycle.
pub fn order_branches(branches: &[String], deps: &BTreeMap<String, Vec<String>>) -> Result<Vec<String>, Vec<String>> {
    let set: BTreeSet<&String> = branches.iter().collect();
    let mut remaining: Vec<&String> = branches.iter().collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(branches.len());
    while !remaining.is_empty() {
        let pos = remaining.iter().position(|b| {
            deps.get(*b).is_none_or(|ds| ds.iter().filter(|d| set.contains(d)).all(|d| done.contains(d)))
        });
        let Some(i) = pos else {
            return Err(remaining.into_iter().cloned().collect());
        };
        let b = remaining.remove(i);
        done.insert(b.clone());
        out.push(b.clone());
    }
    Ok(out)
}

/// Checks that `order` is a permutation of `branches` respecting `deps`.
pub fn validate_order(
    order: &[String],
    branches: &[String],
    deps: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    let want: BTreeSet<&String> = branches.iter().collect();
    let got: BTreeSet<&String> = order.iter().collect();
    if order.len() != branches.len() || want != got {
        return Err("the order must list every branch of the proposal exactly once".into());
    }
    let pos: HashMap<&String, usize> = order.iter().enumerate().map(|(i, b)| (b, i)).collect();
    for (b, ds) in deps {
        let Some(&pb) = pos.get(b) else { continue };
        for d in ds {
            if let Some(&pd) = pos.get(d) {
                if pd > pb {
                    return Err(format!("`{b}` depends on `{d}`, which must merge first"));
                }
            }
        }
    }
    Ok(())
}

/// The recommendation for a set of analysed branches.
#[must_use]
pub fn recommend(branches: &[MergeBranch], blockers: &[MergeBlocker], ordered_conflicts: bool) -> MergeRecommendation {
    if branches.is_empty() || branches.iter().all(|b| b.commits_ahead == 0) {
        return MergeRecommendation::NothingToMerge;
    }
    if !blockers.is_empty() {
        return MergeRecommendation::Blocked;
    }
    if ordered_conflicts {
        return MergeRecommendation::ResolveConflicts;
    }
    if branches.iter().all(|b| b.gates.all_green()) {
        MergeRecommendation::MergeNow
    } else {
        MergeRecommendation::ReviewFirst
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn ordering_follows_dependencies_and_is_stable() {
        let branches = s(&["c", "a", "b", "d"]);
        let mut deps = BTreeMap::new();
        deps.insert("c".to_owned(), s(&["b"]));
        deps.insert("b".to_owned(), s(&["a", "zzz-not-in-proposal"]));
        assert_eq!(order_branches(&branches, &deps).unwrap(), s(&["a", "b", "c", "d"]));
        assert!(validate_order(&s(&["a", "d", "b", "c"]), &branches, &deps).is_ok());
        let e = validate_order(&s(&["b", "a", "c", "d"]), &branches, &deps).unwrap_err();
        assert!(e.contains("`b` depends on `a`"), "{e}");
        assert!(validate_order(&s(&["a", "b", "c"]), &branches, &deps).is_err());
        deps.insert("a".to_owned(), s(&["c"]));
        assert_eq!(order_branches(&branches, &deps).unwrap_err().len(), 3);
    }

    #[test]
    fn path_risks() {
        assert!(path_flags("crates/x/src/migrations/0001.sql").contains(&MergeRiskFlag::Migration));
        assert_eq!(path_flags("Cargo.lock"), vec![MergeRiskFlag::Lockfile]);
        assert!(path_flags("apps/web/package.json").contains(&MergeRiskFlag::Config));
        assert!(path_flags(".github/workflows/ci.yml").contains(&MergeRiskFlag::Ci));
        assert!(path_flags("src/auth/login.rs").contains(&MergeRiskFlag::Security));
        assert!(path_flags("src/lib.rs").is_empty());
        assert!(path_flags("src/authors.md").contains(&MergeRiskFlag::Security)); // conservative
    }

    #[test]
    fn symbols_added_changed_removed_and_public_api() {
        let old = "pub fn keep() -> u32 {\n    1\n}\n\npub fn gone() {}\n\nfn private_change(a: u8) {\n    a;\n}\n";
        let new = "pub fn keep() -> u32 {\n    2\n}\n\nfn private_change(a: u16) {\n    a;\n}\n\npub struct Fresh;\n";
        let ch = symbol_changes("src/lib.rs", Some(old), Some(new));
        let find = |n: &str| ch.iter().find(|c| c.name == n).unwrap();
        assert_eq!(find("keep").change, SymbolChange::Changed);
        assert!(find("keep").public && !find("keep").signature_changed);
        assert_eq!(find("gone").change, SymbolChange::Removed);
        assert!(find("gone").public);
        assert_eq!(find("private_change").change, SymbolChange::Changed);
        assert!(find("private_change").signature_changed && !find("private_change").public);
        assert_eq!(find("Fresh").change, SymbolChange::Added);
        let files = vec![MergeFileSummary {
            path: "src/lib.rs".into(),
            old_path: None,
            status: FileChangeStatus::Modified,
            additions: 3,
            deletions: 3,
            hunks: 2,
            hunk_headers: Vec::new(),
            binary: false,
        }];
        let r = risks(&files, &ch);
        let api = r.iter().find(|r| r.flag == MergeRiskFlag::PublicApi).unwrap();
        assert_eq!(api.items, vec!["src/lib.rs::gone".to_owned()]);
        // TypeScript export.
        let ts = symbol_changes("a.ts", None, Some("export function hello() {}\nfunction local() {}\n"));
        assert!(ts.iter().any(|c| c.name == "hello" && c.public));
        assert!(ts.iter().any(|c| c.name == "local" && !c.public));
        assert!(symbol_changes("notes.txt", None, Some("fn x() {}")).is_empty());
    }

    #[test]
    fn hunks() {
        let p = "diff --git a/x b/x\n@@ -1,2 +1,2 @@ fn main()\n-a\n+b\n@@ -10 +10 @@\n-c\n+d\n";
        let (n, h) = hunk_summary(p);
        assert_eq!(n, 2);
        assert_eq!(h[0], "@@ -1,2 +1,2 @@ fn main()");
    }
}

//! Parsers for git's machine-readable (`-z`, porcelain v2) output formats.
//! Pure functions — unit tested with fixtures (unicode paths, spaces, renames,
//! conflicts).

use orkestr_protocol::methods::vcs::{
    BranchInfo, CommitInfo, ConflictKind, FileChangeStatus, MergeConflict, StatusEntry,
};

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

/// Parsed `git --version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    /// Major.
    pub major: u32,
    /// Minor.
    pub minor: u32,
    /// Patch.
    pub patch: u32,
}

impl GitVersion {
    /// Minimum version with `merge-tree --write-tree`.
    pub const MERGE_TREE_WRITE_TREE: GitVersion = GitVersion { major: 2, minor: 38, patch: 0 };

    /// Whether `self >= other`.
    #[must_use]
    pub fn at_least(&self, other: GitVersion) -> bool {
        *self >= other
    }
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parses `git version 2.45.1.windows.1`.
#[must_use]
pub fn parse_version(s: &str) -> Option<GitVersion> {
    let rest = s.trim().strip_prefix("git version ")?;
    let mut it = rest.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    let major = it.next()?.parse().ok()?;
    let minor = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some(GitVersion { major, minor, patch })
}

// ---------------------------------------------------------------------------
// status --porcelain=v2 -z --branch
// ---------------------------------------------------------------------------

/// Parsed status (branch headers + entries).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedStatus {
    /// `branch.oid` (None for `(initial)`).
    pub oid: Option<String>,
    /// `branch.head` (None for `(detached)`).
    pub head: Option<String>,
    /// HEAD is detached.
    pub detached: bool,
    /// `branch.upstream`.
    pub upstream: Option<String>,
    /// Ahead count.
    pub ahead: u32,
    /// Behind count.
    pub behind: u32,
    /// Entries.
    pub entries: Vec<StatusEntry>,
}

fn status_code(c: char) -> FileChangeStatus {
    match c {
        'M' => FileChangeStatus::Modified,
        'A' => FileChangeStatus::Added,
        'D' => FileChangeStatus::Deleted,
        'R' => FileChangeStatus::Renamed,
        'C' => FileChangeStatus::Copied,
        'T' => FileChangeStatus::TypeChanged,
        'U' => FileChangeStatus::Unmerged,
        _ => FileChangeStatus::Unmodified,
    }
}

fn conflict_kind(xy: &str) -> ConflictKind {
    match xy {
        "DD" => ConflictKind::BothDeleted,
        "AU" => ConflictKind::AddedByUs,
        "UD" => ConflictKind::DeletedByThem,
        "UA" => ConflictKind::AddedByThem,
        "DU" => ConflictKind::DeletedByUs,
        "AA" => ConflictKind::BothAdded,
        _ => ConflictKind::BothModified,
    }
}

fn xy(field: &str) -> (FileChangeStatus, FileChangeStatus) {
    let mut c = field.chars();
    let x = c.next().map_or(FileChangeStatus::Unmodified, status_code);
    let y = c.next().map_or(FileChangeStatus::Unmodified, status_code);
    (x, y)
}

/// Parses `git status --porcelain=v2 -z --branch` output.
#[must_use]
pub fn parse_status_v2(out: &[u8]) -> ParsedStatus {
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0');
    let mut st = ParsedStatus::default();
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, value) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => st.oid = (value != "(initial)").then(|| value.to_owned()),
                "branch.head" => {
                    st.detached = value == "(detached)";
                    st.head = (!st.detached).then(|| value.to_owned());
                }
                "branch.upstream" => st.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(a) = part.strip_prefix('+') {
                            st.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = part.strip_prefix('-') {
                            st.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' => {
                // 1 XY sub mH mI mW hH hI path
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if let [_, code, sub, .., path] = f.as_slice() {
                    if f.len() == 9 {
                        let (index, worktree) = xy(code);
                        st.entries.push(StatusEntry {
                            path: (*path).to_owned(),
                            orig_path: None,
                            index,
                            worktree,
                            conflict: None,
                            submodule: sub.starts_with('S'),
                        });
                    }
                }
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let f: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = records.next().map(str::to_owned);
                if f.len() == 10 {
                    let (index, worktree) = xy(f[1]);
                    st.entries.push(StatusEntry {
                        path: f[9].to_owned(),
                        orig_path: orig,
                        index,
                        worktree,
                        conflict: None,
                        submodule: f[2].starts_with('S'),
                    });
                }
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() == 11 {
                    st.entries.push(StatusEntry {
                        path: f[10].to_owned(),
                        orig_path: None,
                        index: FileChangeStatus::Unmerged,
                        worktree: FileChangeStatus::Unmerged,
                        conflict: Some(conflict_kind(f[1])),
                        submodule: f[2].starts_with('S'),
                    });
                }
            }
            b'?' | b'!' => {
                let path = rec.get(2..).unwrap_or_default().to_owned();
                let s = if kind == b'?' { FileChangeStatus::Untracked } else { FileChangeStatus::Ignored };
                st.entries.push(StatusEntry {
                    path,
                    orig_path: None,
                    index: s,
                    worktree: s,
                    conflict: None,
                    submodule: false,
                });
            }
            _ => {}
        }
    }
    st
}

// ---------------------------------------------------------------------------
// diff --numstat -z / --name-status -z
// ---------------------------------------------------------------------------

/// One `--numstat -z` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumStat {
    /// Added lines (None for binary).
    pub additions: Option<u32>,
    /// Deleted lines (None for binary).
    pub deletions: Option<u32>,
    /// Path (new path for renames).
    pub path: String,
    /// Old path for renames/copies.
    pub old_path: Option<String>,
}

/// Parses `git diff --numstat -z`.
#[must_use]
pub fn parse_numstat_z(out: &[u8]) -> Vec<NumStat> {
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0');
    let mut res = Vec::new();
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let a = a.trim_start_matches('\n');
        let (path, old_path) = if path.is_empty() {
            // rename/copy: the next two records are old and new path
            let old = records.next().unwrap_or_default().to_owned();
            let new = records.next().unwrap_or_default().to_owned();
            (new, Some(old))
        } else {
            (path.to_owned(), None)
        };
        res.push(NumStat { additions: a.parse().ok(), deletions: d.parse().ok(), path, old_path });
    }
    res
}

/// One `--name-status -z` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameStatus {
    /// Status.
    pub status: FileChangeStatus,
    /// Path (new path for renames).
    pub path: String,
    /// Old path for renames/copies.
    pub old_path: Option<String>,
}

/// Parses `git diff --name-status -z`.
#[must_use]
pub fn parse_name_status_z(out: &[u8]) -> Vec<NameStatus> {
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0');
    let mut res = Vec::new();
    while let Some(code) = records.next() {
        let code = code.trim_start_matches('\n');
        let Some(c) = code.chars().next() else { continue };
        let status = status_code(c);
        if matches!(c, 'R' | 'C') {
            let old = records.next().unwrap_or_default().to_owned();
            let new = records.next().unwrap_or_default().to_owned();
            res.push(NameStatus { status, path: new, old_path: Some(old) });
        } else {
            let path = records.next().unwrap_or_default().to_owned();
            res.push(NameStatus { status, path, old_path: None });
        }
    }
    res
}

/// Splits a multi-file unified diff into per-file chunks (each starting with
/// `diff --git `), in output order.
#[must_use]
pub fn split_patch(text: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = Vec::new();
    if text.starts_with("diff --git ") {
        starts.push(0);
    }
    let mut search = 0;
    while let Some(i) = text[search..].find("\ndiff --git ") {
        let at = search + i + 1;
        starts.push(at);
        search = at;
    }
    let mut chunks = Vec::with_capacity(starts.len());
    for (n, &s) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(text.len());
        chunks.push(&text[s..end]);
    }
    chunks
}

/// Truncates `s` to at most `max` bytes on a char boundary.
#[must_use]
pub fn truncate_utf8(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

// ---------------------------------------------------------------------------
// log
// ---------------------------------------------------------------------------

/// `--format` used by [`parse_log`] (fields `\x1f`, records `\x1e`).
pub const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cI%x1f%s%x1f%b%x1f%D%x1e";

/// Parses `git log` output produced with [`LOG_FORMAT`].
#[must_use]
pub fn parse_log(out: &[u8]) -> Vec<CommitInfo> {
    let text = String::from_utf8_lossy(out);
    text.split('\u{1e}')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches(['\n', '\r']);
            if rec.is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.splitn(10, '\u{1f}').collect();
            if f.len() < 10 {
                return None;
            }
            Some(CommitInfo {
                id: f[0].to_owned(),
                short_id: f[1].to_owned(),
                parents: f[2].split_whitespace().map(str::to_owned).collect(),
                author_name: f[3].to_owned(),
                author_email: f[4].to_owned(),
                author_date: f[5].to_owned(),
                committer_date: f[6].to_owned(),
                subject: f[7].to_owned(),
                body: f[8].trim_end().to_owned(),
                refs: f[9].trim().split(", ").filter(|r| !r.is_empty()).map(str::to_owned).collect(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// for-each-ref
// ---------------------------------------------------------------------------

/// `--format` used by [`parse_branches`] (fields NUL-separated, records `\n`).
pub const BRANCH_FORMAT: &str = "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(HEAD)%00%(committerdate:iso-strict)%00%(worktreepath)%00%(contents:subject)";

fn parse_track(track: &str) -> (u32, u32, bool) {
    let mut ahead = 0;
    let mut behind = 0;
    let gone = track.trim() == "gone";
    for part in track.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (ahead, behind, gone)
}

/// Parses `git for-each-ref` output produced with [`BRANCH_FORMAT`].
/// Symbolic `refs/remotes/*/HEAD` entries are skipped. Local branches come
/// first, then remote-tracking ones, each sorted by name.
#[must_use]
pub fn parse_branches(out: &[u8]) -> Vec<BranchInfo> {
    let text = String::from_utf8_lossy(out);
    let mut res: Vec<BranchInfo> = text
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(9, '\0').collect();
            if f.len() < 9 {
                return None;
            }
            let full_ref = f[0];
            if full_ref.starts_with("refs/remotes/") && full_ref.ends_with("/HEAD") {
                return None;
            }
            let (ahead, behind, gone) = parse_track(f[4]);
            Some(BranchInfo {
                name: f[1].to_owned(),
                full_ref: full_ref.to_owned(),
                remote: full_ref.starts_with("refs/remotes/"),
                current: f[5].trim() == "*",
                commit: f[2].to_owned(),
                upstream: (!f[3].is_empty()).then(|| f[3].to_owned()),
                ahead,
                behind,
                gone,
                worktree_path: (!f[7].is_empty()).then(|| f[7].to_owned()),
                subject: f[8].to_owned(),
                committer_date: (!f[6].is_empty()).then(|| f[6].to_owned()),
            })
        })
        .collect();
    res.sort_by(|a, b| a.remote.cmp(&b.remote).then_with(|| a.name.cmp(&b.name)));
    res
}

// ---------------------------------------------------------------------------
// worktree list --porcelain -z
// ---------------------------------------------------------------------------

/// One entry of `git worktree list --porcelain -z`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawWorktree {
    /// Path as printed by git.
    pub path: String,
    /// HEAD.
    pub head: Option<String>,
    /// Full branch ref (`refs/heads/x`).
    pub branch_ref: Option<String>,
    /// Detached.
    pub detached: bool,
    /// Bare.
    pub bare: bool,
    /// Locked.
    pub locked: bool,
    /// Prunable.
    pub prunable: bool,
}

impl RawWorktree {
    /// Short branch name.
    #[must_use]
    pub fn branch(&self) -> Option<String> {
        self.branch_ref.as_deref().map(|r| r.strip_prefix("refs/heads/").unwrap_or(r).to_owned())
    }
}

/// Parses `git worktree list --porcelain -z`.
#[must_use]
pub fn parse_worktrees_z(out: &[u8]) -> Vec<RawWorktree> {
    let text = String::from_utf8_lossy(out);
    let mut res = Vec::new();
    let mut cur: Option<RawWorktree> = None;
    for rec in text.split('\0') {
        if rec.is_empty() {
            if let Some(w) = cur.take() {
                res.push(w);
            }
            continue;
        }
        let (key, value) = rec.split_once(' ').unwrap_or((rec, ""));
        if key == "worktree" {
            if let Some(w) = cur.take() {
                res.push(w);
            }
            cur = Some(RawWorktree { path: value.to_owned(), ..RawWorktree::default() });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match key {
            "HEAD" => w.head = Some(value.to_owned()),
            "branch" => w.branch_ref = Some(value.to_owned()),
            "detached" => w.detached = true,
            "bare" => w.bare = true,
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    if let Some(w) = cur.take() {
        res.push(w);
    }
    res
}

// ---------------------------------------------------------------------------
// merge-tree --write-tree --name-only --messages -z
// ---------------------------------------------------------------------------

/// Parsed merge-tree output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedMergeTree {
    /// Toplevel tree OID.
    pub tree: String,
    /// Conflicting paths with their messages.
    pub conflicts: Vec<MergeConflict>,
    /// All informational messages.
    pub messages: Vec<String>,
}

/// Parses `git merge-tree --write-tree --name-only --messages -z` output.
///
/// Layout: `<tree>\0` + conflicted paths (`<path>\0` each) + `\0` +
/// messages (`<n>\0<path>\0…<type>\0<message>\0` each).
#[must_use]
pub fn parse_merge_tree_z(out: &[u8], conflicted: bool) -> ParsedMergeTree {
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0');
    let mut res = ParsedMergeTree { tree: records.next().unwrap_or_default().trim().to_owned(), ..Default::default() };
    if conflicted {
        for rec in records.by_ref() {
            if rec.is_empty() {
                break;
            }
            if !res.conflicts.iter().any(|c| c.path == rec) {
                res.conflicts.push(MergeConflict { path: rec.to_owned(), kinds: Vec::new(), messages: Vec::new() });
            }
        }
    } else {
        // Clean merges have no conflicted-file section; an empty record
        // separates the (empty) section from the messages.
    }
    while let Some(n) = records.next() {
        if n.is_empty() {
            continue;
        }
        let Ok(count) = n.parse::<usize>() else { break };
        let paths: Vec<String> = records.by_ref().take(count).map(str::to_owned).collect();
        let kind = records.next().unwrap_or_default().to_owned();
        let message = records.next().unwrap_or_default().trim_end().to_owned();
        for p in &paths {
            if let Some(c) = res.conflicts.iter_mut().find(|c| &c.path == p) {
                if !kind.is_empty() && !c.kinds.contains(&kind) {
                    c.kinds.push(kind.clone());
                }
                c.messages.push(message.clone());
            }
        }
        res.messages.push(message);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("git version 2.45.1.windows.1\n"), Some(GitVersion { major: 2, minor: 45, patch: 1 }));
        assert_eq!(parse_version("git version 2.38.0"), Some(GitVersion { major: 2, minor: 38, patch: 0 }));
        assert_eq!(
            parse_version("git version 2.39.3 (Apple Git-146)"),
            Some(GitVersion { major: 2, minor: 39, patch: 3 })
        );
        assert!(parse_version("nope").is_none());
        let v = parse_version("git version 2.37.9").unwrap();
        assert!(!v.at_least(GitVersion::MERGE_TREE_WRITE_TREE));
    }

    #[test]
    fn status_fixture_with_unicode_spaces_renames_conflicts() {
        let mut out = Vec::new();
        for rec in [
            "# branch.oid 1111111111111111111111111111111111111111",
            "# branch.head feature/ünï code",
            "# branch.upstream origin/feature",
            "# branch.ab +3 -1",
            "1 .M N... 100644 100644 100644 aaaa bbbb src/main file.rs",
            "1 A. N... 000000 100644 100644 0000 cccc новый файл.txt",
            "2 R. N... 100644 100644 100644 dddd dddd R100 docs/new name.md",
            "docs/old name.md",
            "u UU N... 100644 100644 100644 100644 e1 e2 e3 conflict both.txt",
            "u DU N... 100644 000000 100644 100644 e1 e2 e3 gone.txt",
            "? 未跟踪 file.txt",
            "! ignored.log",
        ] {
            out.extend_from_slice(rec.as_bytes());
            out.push(0);
        }
        let st = parse_status_v2(&out);
        assert_eq!(st.oid.as_deref(), Some("1111111111111111111111111111111111111111"));
        assert_eq!(st.head.as_deref(), Some("feature/ünï code"));
        assert_eq!(st.upstream.as_deref(), Some("origin/feature"));
        assert_eq!((st.ahead, st.behind), (3, 1));
        assert_eq!(st.entries.len(), 7);
        let e = &st.entries[0];
        assert_eq!(e.path, "src/main file.rs");
        assert_eq!((e.index, e.worktree), (FileChangeStatus::Unmodified, FileChangeStatus::Modified));
        assert!(e.is_unstaged() && !e.is_staged());
        assert_eq!(st.entries[1].path, "новый файл.txt");
        assert_eq!(st.entries[1].index, FileChangeStatus::Added);
        let r = &st.entries[2];
        assert_eq!(r.path, "docs/new name.md");
        assert_eq!(r.orig_path.as_deref(), Some("docs/old name.md"));
        assert_eq!(r.index, FileChangeStatus::Renamed);
        assert_eq!(st.entries[3].conflict, Some(ConflictKind::BothModified));
        assert_eq!(st.entries[3].path, "conflict both.txt");
        assert_eq!(st.entries[4].conflict, Some(ConflictKind::DeletedByUs));
        assert!(st.entries[5].is_untracked());
        assert_eq!(st.entries[5].path, "未跟踪 file.txt");
        assert_eq!(st.entries[6].worktree, FileChangeStatus::Ignored);
    }

    #[test]
    fn status_initial_and_detached() {
        let out = b"# branch.oid (initial)\0# branch.head (detached)\0";
        let st = parse_status_v2(out);
        assert!(st.oid.is_none());
        assert!(st.detached);
        assert!(st.head.is_none());
        assert!(st.entries.is_empty());
    }

    #[test]
    fn numstat_and_name_status() {
        let out = b"3\t1\tsrc/a b.rs\0-\t-\timg.png\0\x30\t0\t\0old n.md\0new n.md\0";
        let ns = parse_numstat_z(out);
        assert_eq!(ns.len(), 3);
        assert_eq!(
            ns[0],
            NumStat { additions: Some(3), deletions: Some(1), path: "src/a b.rs".into(), old_path: None }
        );
        assert_eq!(ns[1].additions, None);
        assert_eq!(ns[2].path, "new n.md");
        assert_eq!(ns[2].old_path.as_deref(), Some("old n.md"));

        let out = b"M\0src/a b.rs\0A\0img.png\0R087\0old n.md\0new n.md\0D\0x\0";
        let st = parse_name_status_z(out);
        assert_eq!(st.len(), 4);
        assert_eq!(st[0].status, FileChangeStatus::Modified);
        assert_eq!(st[1].status, FileChangeStatus::Added);
        assert_eq!(st[2].status, FileChangeStatus::Renamed);
        assert_eq!(st[2].old_path.as_deref(), Some("old n.md"));
        assert_eq!(st[3].status, FileChangeStatus::Deleted);
    }

    #[test]
    fn patch_splitting() {
        let text =
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+diff --git inside\ndiff --git a/b b/b\nnew\n";
        let chunks = split_patch(text);
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert!(chunks[0].contains("+diff --git inside"));
        assert!(chunks[1].starts_with("diff --git a/b b/b"));
        assert!(split_patch("").is_empty());
        let (t, cut) = truncate_utf8("héllo", 2);
        assert_eq!((t, cut), ("h", true));
    }

    #[test]
    fn log_parsing() {
        let rec = |id: &str, parents: &str, refs: &str| {
            format!("{id}\u{1f}{}\u{1f}{parents}\u{1f}Ann\u{1f}a@x\u{1f}2026-01-01T00:00:00+00:00\u{1f}2026-01-01T00:00:00+00:00\u{1f}Subject {id}\u{1f}Body line\n\n\u{1f}{refs}\u{1e}\n", &id[..7])
        };
        let out =
            format!("{}{}", rec("aaaaaaaaaa", "bbbbbbbbbb", "HEAD -> main, origin/main"), rec("bbbbbbbbbb", "", ""));
        let log = parse_log(out.as_bytes());
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents, vec!["bbbbbbbbbb"]);
        assert_eq!(log[0].refs, vec!["HEAD -> main", "origin/main"]);
        assert_eq!(log[0].body, "Body line");
        assert!(log[1].parents.is_empty());
        assert!(log[1].refs.is_empty());
    }

    #[test]
    fn branch_parsing() {
        let out = "refs/heads/main\0main\0aaa\0origin/main\0ahead 2, behind 1\0*\x002026-01-01T00:00:00+00:00\0C:/repo\0Init\n\
                   refs/heads/agent/backend-1234abcd\0agent/backend-1234abcd\0bbb\0\0\0 \x002026-01-01T00:00:00+00:00\0C:/wt/x\0Work\n\
                   refs/remotes/origin/HEAD\0origin\0aaa\0\0\0 \0\0\0\n\
                   refs/remotes/origin/main\0origin/main\0aaa\0\0\0 \0\0\0Init\n\
                   refs/heads/old\0old\0ccc\0origin/old\0gone\0 \0\0\0x\n";
        let b = parse_branches(out.as_bytes());
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].name, "agent/backend-1234abcd");
        assert_eq!(b[0].worktree_path.as_deref(), Some("C:/wt/x"));
        let main = b.iter().find(|x| x.name == "main").unwrap();
        assert!(main.current);
        assert_eq!((main.ahead, main.behind), (2, 1));
        let old = b.iter().find(|x| x.name == "old").unwrap();
        assert!(old.gone);
        assert!(b[3].remote);
        assert_eq!(b[3].name, "origin/main");
    }

    #[test]
    fn worktree_parsing() {
        let out = b"worktree C:/repo\0HEAD aaa\0branch refs/heads/main\0\0worktree C:/data/wt/p/x y\0HEAD bbb\0detached\0locked\0\0worktree /gone\0HEAD ccc\0branch refs/heads/agent/qa-1\0prunable gitdir file points to non-existent location\0\0";
        let w = parse_worktrees_z(out);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].branch().as_deref(), Some("main"));
        assert_eq!(w[1].path, "C:/data/wt/p/x y");
        assert!(w[1].detached && w[1].locked);
        assert!(w[2].prunable);
        assert_eq!(w[2].branch().as_deref(), Some("agent/qa-1"));
    }

    #[test]
    fn merge_tree_parsing() {
        let out = b"abc123\0f.txt\0dir/g h.txt\0\x001\0f.txt\0CONFLICT (contents)\0CONFLICT (content): Merge conflict in f.txt\n\x001\0other.txt\0Auto-merging\0Auto-merging other.txt\n\x00";
        let p = parse_merge_tree_z(out, true);
        assert_eq!(p.tree, "abc123");
        assert_eq!(p.conflicts.len(), 2);
        assert_eq!(p.conflicts[0].path, "f.txt");
        assert_eq!(p.conflicts[0].kinds, vec!["CONFLICT (contents)"]);
        assert_eq!(p.conflicts[1].path, "dir/g h.txt");
        assert_eq!(p.messages.len(), 2);

        let clean = parse_merge_tree_z(b"def456\0\x001\0a\0Auto-merging\0Auto-merging a\n\x00", false);
        assert_eq!(clean.tree, "def456");
        assert!(clean.conflicts.is_empty());
        assert_eq!(clean.messages, vec!["Auto-merging a"]);
    }
}

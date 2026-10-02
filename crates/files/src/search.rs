//! Project content search (parallel, `.gitignore`-aware) and fuzzy file
//! finding (quick open).

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};
use orkestr_protocol::methods::files::{FindFilesResult, FindMatch, SearchFilesResult, SearchMatch};
use parking_lot::Mutex;
use regex::{Regex, RegexBuilder};

use crate::error::{FilesError, FilesResult};
use crate::ops::TEMP_MARKER;
use crate::path::WorkspaceRoot;
use crate::text::{self, utf16_len, Decoded};

/// Max characters of a preview line.
const PREVIEW_CHARS: usize = 240;
/// Characters kept before the match when a preview is shortened.
const PREVIEW_CONTEXT: usize = 60;
/// Max matches recorded per file.
const PER_FILE_CAP: usize = 1000;
/// Max length of a query.
const MAX_QUERY_LEN: usize = 2000;

/// Content search options.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Text or regex.
    pub query: String,
    /// Regex mode.
    pub is_regex: bool,
    /// Case sensitive.
    pub case_sensitive: bool,
    /// Whole words.
    pub whole_word: bool,
    /// Include globs.
    pub include: Vec<String>,
    /// Exclude globs.
    pub exclude: Vec<String>,
    /// Max matches.
    pub max_results: usize,
    /// Search ignored files too.
    pub show_ignored: bool,
    /// Skip files larger than this.
    pub max_file_bytes: u64,
}

/// Builds the matcher for the options.
pub fn build_regex(opts: &SearchOptions) -> FilesResult<Regex> {
    if opts.query.is_empty() {
        return Err(FilesError::InvalidQuery("query must not be empty".into()));
    }
    if opts.query.len() > MAX_QUERY_LEN {
        return Err(FilesError::InvalidQuery("query is too long".into()));
    }
    let base = if opts.is_regex { opts.query.clone() } else { regex::escape(&opts.query) };
    let pattern = if opts.whole_word { format!(r"\b(?:{base})\b") } else { base };
    RegexBuilder::new(&pattern)
        .case_insensitive(!opts.case_sensitive)
        .size_limit(4 << 20)
        .dfa_size_limit(8 << 20)
        .build()
        .map_err(|e| FilesError::InvalidQuery(e.to_string()))
}

fn walk_builder(
    root: &WorkspaceRoot,
    include: &[String],
    exclude: &[String],
    show_ignored: bool,
) -> FilesResult<WalkBuilder> {
    let mut b = WalkBuilder::new(root.path());
    b.hidden(false)
        .follow_links(false)
        .require_git(false)
        .git_global(false)
        .parents(true)
        .ignore(!show_ignored)
        .git_ignore(!show_ignored)
        .git_exclude(!show_ignored)
        .filter_entry(|e| {
            e.file_name().to_str().map_or(true, |n| !n.eq_ignore_ascii_case(".git") && !n.contains(TEMP_MARKER))
        });
    if !include.is_empty() || !exclude.is_empty() {
        let mut ob = OverrideBuilder::new(root.path());
        for g in include.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
            ob.add(g).map_err(|e| FilesError::InvalidQuery(format!("include glob `{g}`: {e}")))?;
        }
        for g in exclude.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
            let g = g.trim_start_matches('!');
            ob.add(&format!("!{g}")).map_err(|e| FilesError::InvalidQuery(format!("exclude glob `{g}`: {e}")))?;
        }
        b.overrides(ob.build().map_err(|e| FilesError::InvalidQuery(e.to_string()))?);
    }
    Ok(b)
}

/// Builds one match record for `line[start..end]` (byte offsets).
#[must_use]
pub fn make_match(path: &str, line_no: usize, line: &str, start: usize, end: usize) -> SearchMatch {
    let column = utf16_len(&line[..start]) + 1;
    let length = utf16_len(&line[start..end]);
    let lead = (line.len() - line.trim_start().len()).min(start);
    // Window start: at most PREVIEW_CONTEXT chars before the match, never before the indentation.
    let ctx_start = line[..start].char_indices().rev().nth(PREVIEW_CONTEXT - 1).map_or(0, |(i, _)| i);
    let win = ctx_start.max(lead);
    let preview: String = line[win..].trim_end().chars().take(PREVIEW_CHARS).collect();
    let preview_start = utf16_len(&line[win..start]);
    let preview_len = utf16_len(&preview);
    SearchMatch {
        path: path.to_owned(),
        line: u32::try_from(line_no).unwrap_or(u32::MAX),
        column,
        length,
        preview_start: preview_start.min(preview_len),
        preview_end: (preview_start + length).min(preview_len),
        preview,
    }
}

/// Searches file contents below the root.
pub fn search(root: &WorkspaceRoot, opts: &SearchOptions) -> FilesResult<SearchFilesResult> {
    let started = Instant::now();
    let regex = Arc::new(build_regex(opts)?);
    let walker = walk_builder(root, &opts.include, &opts.exclude, opts.show_ignored)?.build_parallel();
    let max = opts.max_results.max(1);
    let results: Arc<Mutex<Vec<SearchMatch>>> = Arc::new(Mutex::new(Vec::new()));
    let count = Arc::new(AtomicUsize::new(0));
    let files = Arc::new(AtomicU64::new(0));
    let truncated = Arc::new(AtomicBool::new(false));
    walker.run(|| {
        let regex = Arc::clone(&regex);
        let results = Arc::clone(&results);
        let count = Arc::clone(&count);
        let files = Arc::clone(&files);
        let truncated = Arc::clone(&truncated);
        let max_file_bytes = opts.max_file_bytes;
        Box::new(move |entry| {
            if count.load(Ordering::Relaxed) >= max {
                truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            if entry.metadata().map(|m| m.len() > max_file_bytes).unwrap_or(true) {
                return WalkState::Continue;
            }
            let Some(rel) = root.relative_of(entry.path()) else { return WalkState::Continue };
            let Ok(bytes) = fs::read(entry.path()) else { return WalkState::Continue };
            let Decoded::Text { content, .. } = text::decode(&bytes) else { return WalkState::Continue };
            files.fetch_add(1, Ordering::Relaxed);
            let path = rel.as_string();
            let mut local = Vec::new();
            'lines: for (i, line) in content.lines().enumerate() {
                for m in regex.find_iter(line) {
                    if m.start() == m.end() {
                        continue;
                    }
                    local.push(make_match(&path, i + 1, line, m.start(), m.end()));
                    if local.len() >= PER_FILE_CAP {
                        break 'lines;
                    }
                }
            }
            if !local.is_empty() {
                let mut all = results.lock();
                let room = max.saturating_sub(all.len());
                if local.len() > room {
                    local.truncate(room);
                    truncated.store(true, Ordering::Relaxed);
                }
                all.extend(local);
                count.store(all.len(), Ordering::Relaxed);
                if all.len() >= max {
                    return WalkState::Quit;
                }
            }
            WalkState::Continue
        })
    });
    let mut matches = std::mem::take(&mut *results.lock());
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)).then(a.column.cmp(&b.column)));
    Ok(SearchFilesResult {
        matches,
        files_searched: files.load(Ordering::Relaxed),
        truncated: truncated.load(Ordering::Relaxed),
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

fn is_boundary(prev: Option<char>, cur: char) -> bool {
    match prev {
        None => true,
        Some(p) => matches!(p, '/' | '\\' | '_' | '-' | '.' | ' ') || (p.is_lowercase() && cur.is_uppercase()),
    }
}

/// Fuzzy score of `query` against `path` (higher is better) with matched
/// positions as UTF-16 indices; `None` when `query` is not a subsequence.
///
/// Matching runs right-to-left so the basename is preferred; bonuses for
/// consecutive runs, word boundaries, basename-only matches and basename
/// prefix/equality; a small penalty for long paths and gaps.
#[must_use]
pub fn fuzzy_score(query: &str, path: &str) -> Option<(i64, Vec<u32>)> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    let chars: Vec<char> = path.chars().collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let mut positions = Vec::with_capacity(q.len());
    let mut qi = q.len();
    let mut pi = lower.len();
    while qi > 0 && pi > 0 {
        pi -= 1;
        if lower[pi] == q[qi - 1] {
            positions.push(pi);
            qi -= 1;
        }
    }
    if qi > 0 {
        return None;
    }
    positions.reverse();
    let base_start = chars.iter().rposition(|&c| c == '/' || c == '\\').map_or(0, |i| i + 1);
    let mut score: i64 = 0;
    let mut prev_pos: Option<usize> = None;
    let mut streak: i64 = 0;
    for &p in &positions {
        score += 1;
        if prev_pos.is_some_and(|pp| pp + 1 == p) {
            streak += 1;
            score += 4 + streak * 2;
        } else {
            streak = 0;
            if let Some(pp) = prev_pos {
                score -= i64::try_from((p - pp - 1).min(8)).unwrap_or(8);
            }
        }
        let prev_char = if p == 0 { None } else { Some(chars[p - 1]) };
        if is_boundary(prev_char, chars[p]) {
            score += if matches!(prev_char, None | Some('/' | '\\')) { 10 } else { 7 };
        }
        prev_pos = Some(p);
    }
    if positions.first().is_some_and(|&p| p >= base_start) {
        score += 25;
    }
    let base_alnum: String = lower[base_start..].iter().filter(|c| c.is_alphanumeric()).collect();
    let q_str: String = q.iter().filter(|c| c.is_alphanumeric()).collect();
    if !q_str.is_empty() {
        if base_alnum == q_str {
            score += 60;
        } else if base_alnum.starts_with(&q_str) {
            score += 30;
        }
    }
    score -= i64::try_from(chars.len() / 8).unwrap_or(0);
    // char index → UTF-16 index
    let mut utf16_at = Vec::with_capacity(chars.len());
    let mut acc = 0u32;
    for c in &chars {
        utf16_at.push(acc);
        acc += u32::try_from(c.len_utf16()).unwrap_or(2);
    }
    Some((score, positions.into_iter().map(|p| utf16_at[p]).collect()))
}

/// Fuzzy file-name search.
pub fn find(
    root: &WorkspaceRoot,
    query: &str,
    max_results: usize,
    show_ignored: bool,
    max_scan: usize,
) -> FilesResult<FindFilesResult> {
    if query.len() > MAX_QUERY_LEN {
        return Err(FilesError::InvalidQuery("query is too long".into()));
    }
    let walker = walk_builder(root, &[], &[], show_ignored)?.build_parallel();
    let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let truncated = Arc::new(AtomicBool::new(false));
    walker.run(|| {
        let paths = Arc::clone(&paths);
        let truncated = Arc::clone(&truncated);
        Box::new(move |entry| {
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let Some(rel) = root.relative_of(entry.path()) else { return WalkState::Continue };
            let mut all = paths.lock();
            if all.len() >= max_scan {
                truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            all.push(rel.as_string());
            WalkState::Continue
        })
    });
    let paths = std::mem::take(&mut *paths.lock());
    let scanned = paths.len() as u64;
    let mut scored: Vec<FindMatch> = paths
        .into_iter()
        .filter_map(|p| fuzzy_score(query, &p).map(|(score, positions)| FindMatch { path: p, score, positions }))
        .collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.len().cmp(&b.path.len())).then(a.path.cmp(&b.path)));
    scored.truncate(max_results.max(1));
    Ok(FindFilesResult { matches: scored, files_scanned: scanned, truncated: truncated.load(Ordering::Relaxed) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (tempfile::TempDir, WorkspaceRoot) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::create_dir_all(p.join("src/domain")).unwrap();
        fs::create_dir_all(p.join("target")).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
        fs::write(p.join(".gitignore"), "target/\n").unwrap();
        fs::write(p.join("src/main.rs"), "fn main() {\n    let Foo = foo_bar();\n    println!(\"foo\");\n}\n").unwrap();
        fs::write(p.join("src/domain/remains.rs"), "// FOO here\nstruct Food;\n").unwrap();
        fs::write(p.join("README.md"), "# Foo project\n").unwrap();
        fs::write(p.join("target/out.rs"), "foo in build output\n").unwrap();
        fs::write(p.join(".git/config"), "foo in git\n").unwrap();
        fs::write(p.join("blob.bin"), [0u8, 1, 2, b'f', b'o', b'o']).unwrap();
        let root = WorkspaceRoot::open(p).unwrap();
        (dir, root)
    }

    fn opts(q: &str) -> SearchOptions {
        SearchOptions { query: q.into(), max_results: 100, max_file_bytes: 1 << 20, ..Default::default() }
    }

    #[test]
    fn literal_case_insensitive_search_skips_ignored_git_and_binary() {
        let (_d, root) = tree();
        let res = search(&root, &opts("foo")).unwrap();
        let paths: Vec<_> = res.matches.iter().map(|m| (m.path.as_str(), m.line)).collect();
        assert_eq!(
            paths,
            vec![
                ("README.md", 1),
                ("src/domain/remains.rs", 1),
                ("src/domain/remains.rs", 2),
                ("src/main.rs", 2),
                ("src/main.rs", 2),
                ("src/main.rs", 3)
            ]
        );
        assert!(!res.truncated);
        let m = &res.matches[3];
        assert_eq!((m.column, m.length), (9, 3));
        assert_eq!(m.preview, "let Foo = foo_bar();");
        assert_eq!((m.preview_start, m.preview_end), (4, 7));

        let with_ignored = search(&root, &SearchOptions { show_ignored: true, ..opts("foo") }).unwrap();
        assert!(with_ignored.matches.iter().any(|m| m.path == "target/out.rs"));
        assert!(!with_ignored.matches.iter().any(|m| m.path.starts_with(".git")), ".git is never searched");
    }

    #[test]
    fn case_whole_word_regex_and_globs() {
        let (_d, root) = tree();
        let cs = search(&root, &SearchOptions { case_sensitive: true, ..opts("Foo") }).unwrap();
        assert_eq!(cs.matches.len(), 3); // main.rs "Foo", remains.rs "Food", README "Foo"
        let ww = search(&root, &SearchOptions { whole_word: true, ..opts("foo") }).unwrap();
        assert_eq!(ww.matches.len(), 4, "no Food, no foo_bar");
        let re = search(&root, &SearchOptions { is_regex: true, ..opts(r"fo+_\w+") }).unwrap();
        assert_eq!(re.matches.len(), 1);
        assert_eq!(re.matches[0].length, 7);
        let inc = search(&root, &SearchOptions { include: vec!["*.md".into()], ..opts("foo") }).unwrap();
        assert_eq!(inc.matches.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), vec!["README.md"]);
        let exc = search(&root, &SearchOptions { exclude: vec!["src/**".into()], ..opts("foo") }).unwrap();
        assert_eq!(exc.matches.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), vec!["README.md"]);
        let dir_inc = search(&root, &SearchOptions { include: vec!["src/domain/**".into()], ..opts("foo") }).unwrap();
        assert!(dir_inc.matches.iter().all(|m| m.path.starts_with("src/domain/")));
        assert!(!dir_inc.matches.is_empty());
        assert!(matches!(
            search(&root, &SearchOptions { is_regex: true, ..opts("(") }),
            Err(FilesError::InvalidQuery(_))
        ));
        assert!(matches!(search(&root, &opts("")), Err(FilesError::InvalidQuery(_))));
    }

    #[test]
    fn max_results_truncates() {
        let (_d, root) = tree();
        let res = search(&root, &SearchOptions { max_results: 2, ..opts("foo") }).unwrap();
        assert_eq!(res.matches.len(), 2);
        assert!(res.truncated);
    }

    #[test]
    fn preview_windows_long_lines_in_utf16_units() {
        let line = format!("{}héllo😀 target here", "x".repeat(500));
        let start = line.find("target").unwrap();
        let m = make_match("a", 1, &line, start, start + 6);
        assert!(m.preview.chars().count() <= PREVIEW_CHARS);
        let units: Vec<u16> = m.preview.encode_utf16().collect();
        let picked = String::from_utf16(&units[m.preview_start as usize..m.preview_end as usize]).unwrap();
        assert_eq!(picked, "target");
        assert_eq!(m.column, 500 + 5 + 2 + 1 + 1); // "x"*500 + "héllo" + 😀(2 units) + " " → 1-based
    }

    #[test]
    fn fuzzy_prefers_basename_and_reports_positions() {
        let (a, pa) = fuzzy_score("mainrs", "src/main.rs").unwrap();
        let (b, _) = fuzzy_score("mainrs", "src/domain/remains.rs").unwrap();
        assert!(a > b, "{a} <= {b}");
        assert_eq!(pa, vec![4, 5, 6, 7, 9, 10]);
        assert!(fuzzy_score("xyz", "src/main.rs").is_none());
        let (c, _) = fuzzy_score("ws", "crates/core/src/rpc/ws.rs").unwrap();
        let (d, _) = fuzzy_score("ws", "crates/workspace/src/lib.rs").unwrap();
        assert!(c > d);
        assert_eq!(fuzzy_score("", "x").unwrap().0, 0);
        assert!(fuzzy_score("Main RS", "src/main.rs").is_some(), "spaces ignored, case-insensitive");
    }

    #[test]
    fn find_ranks_and_respects_ignores() {
        let (_d, root) = tree();
        let res = find(&root, "main", 10, false, 1000).unwrap();
        assert_eq!(res.matches.first().map(|m| m.path.as_str()), Some("src/main.rs"));
        assert!(!res.matches.iter().any(|m| m.path.starts_with("target/") || m.path.starts_with(".git/")));
        let all = find(&root, "", 100, false, 1000).unwrap();
        assert_eq!(all.files_scanned, 5); // .gitignore, README.md, blob.bin, main.rs, remains.rs
        let capped = find(&root, "", 100, false, 2).unwrap();
        assert!(capped.truncated);
    }
}

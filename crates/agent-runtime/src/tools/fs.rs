//! Filesystem tools: `read_file`, `write_file`, `edit_file`, `list_dir`,
//! `search_files`. Every path is confined to the agent root with
//! [`orkestr_security::confine`] (traversal, absolute, UNC, device names,
//! ADS, symlink/junction escapes are rejected) right before the operation.

use std::path::{Path, PathBuf};

use orkestr_protocol::event::{EventKind, FileChangeActor, FileChangeKind, FileChangedPayload};
use orkestr_protocol::NewEvent;
use serde_json::Value;

use super::{arg_str, opt_bool, opt_str, opt_u64, touches_git_dir, ToolEnv, ToolError, ToolOutput};

/// Max bytes read from one file.
pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
/// Max bytes written by one call.
pub const MAX_WRITE_BYTES: usize = 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 500;
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;

/// Resolves a path inside the root: `(absolute, relative)`.
pub fn resolve(env: &ToolEnv, path: &str) -> Result<(PathBuf, String), ToolError> {
    let root = env.root()?;
    let abs = orkestr_security::confine(root, path).map_err(|e| ToolError::new(e.to_string()))?;
    let rel = orkestr_security::confine_relative(root, path).map_err(|e| ToolError::new(e.to_string()))?;
    Ok((abs, if rel.is_empty() { ".".into() } else { rel }))
}

async fn read_text(abs: &Path, rel: &str) -> Result<String, ToolError> {
    let meta = tokio::fs::metadata(abs).await.map_err(|e| ToolError::new(format!("cannot read `{rel}`: {e}")))?;
    if meta.is_dir() {
        return Err(ToolError::new(format!("`{rel}` is a directory — use list_dir")));
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(ToolError::new(format!(
            "`{rel}` is {} bytes; larger than the {MAX_READ_BYTES}-byte limit",
            meta.len()
        )));
    }
    let bytes = tokio::fs::read(abs).await.map_err(|e| ToolError::new(format!("cannot read `{rel}`: {e}")))?;
    if bytes.contains(&0) {
        return Err(ToolError::new(format!("`{rel}` looks like a binary file")));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn file_changed(env: &ToolEnv, rel: &str, change: FileChangeKind) {
    let ev = NewEvent::new(EventKind::FileChanged(FileChangedPayload {
        path: rel.to_owned(),
        old_path: None,
        change,
        actor: FileChangeActor::Agent,
        agent_id: Some(env.agent_id),
    }))
    .project(env.project_id)
    .task(env.task_id)
    .agent(env.agent_id);
    env.deps.publish(ev).await;
}

/// `read_file`.
pub async fn read_file(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let (abs, rel) = resolve(env, arg_str(args, "path")?)?;
    let text = read_text(&abs, &rel).await?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let offset = usize::try_from(opt_u64(args, "offset").unwrap_or(1).max(1)).unwrap_or(1);
    let limit = usize::try_from(opt_u64(args, "limit").unwrap_or(400).clamp(1, 2000)).unwrap_or(400);
    let start = offset.saturating_sub(1).min(total);
    let end = start.saturating_add(limit).min(total);
    let mut out = format!("{rel} (lines {}-{} of {total})\n", start + usize::from(total > 0), end);
    for (i, l) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:>5}\t{l}\n", start + i + 1));
    }
    if end < total {
        out.push_str(&format!("… {} more lines (use offset={})\n", total - end, end + 1));
    }
    Ok(ToolOutput::ok(out, format!("read {rel} ({total} lines)")))
}

/// `write_file`.
pub async fn write_file(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let path = arg_str(args, "path")?;
    let content = arg_str(args, "content")?;
    if touches_git_dir(path) {
        return Err(ToolError::new("writing inside `.git` is not allowed — use git_commit"));
    }
    if content.len() > MAX_WRITE_BYTES {
        return Err(ToolError::new(format!("content is larger than {MAX_WRITE_BYTES} bytes")));
    }
    let (abs, rel) = resolve(env, path)?;
    if abs.is_dir() {
        return Err(ToolError::new(format!("`{rel}` is a directory")));
    }
    let existed = abs.exists();
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| ToolError::new(format!("cannot create folder: {e}")))?;
    }
    tokio::fs::write(&abs, content.as_bytes())
        .await
        .map_err(|e| ToolError::new(format!("cannot write `{rel}`: {e}")))?;
    file_changed(env, &rel, if existed { FileChangeKind::Modified } else { FileChangeKind::Created }).await;
    let verb = if existed { "updated" } else { "created" };
    let mut out = ToolOutput::ok(
        format!("{verb} {rel} ({} bytes, {} lines)", content.len(), content.lines().count()),
        format!("{verb} {rel}"),
    );
    out.files_changed.push(rel);
    Ok(out)
}

/// `edit_file`.
pub async fn edit_file(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let path = arg_str(args, "path")?;
    let old = arg_str(args, "old")?;
    let new = arg_str(args, "new")?;
    let all = opt_bool(args, "replace_all").unwrap_or(false);
    if old.is_empty() {
        return Err(ToolError::new("`old` must not be empty"));
    }
    if touches_git_dir(path) {
        return Err(ToolError::new("writing inside `.git` is not allowed — use git_commit"));
    }
    let (abs, rel) = resolve(env, path)?;
    let text = read_text(&abs, &rel).await?;
    let count = text.matches(old).count();
    if count == 0 {
        return Err(ToolError::new(format!("`old` text not found in {rel} — read the file and copy the exact text")));
    }
    if count > 1 && !all {
        return Err(ToolError::new(format!(
            "`old` text matches {count} times in {rel}; add surrounding context or set replace_all"
        )));
    }
    let updated = if all { text.replace(old, new) } else { text.replacen(old, new, 1) };
    if updated.len() > MAX_WRITE_BYTES * 2 {
        return Err(ToolError::new("resulting file is too large"));
    }
    tokio::fs::write(&abs, updated.as_bytes())
        .await
        .map_err(|e| ToolError::new(format!("cannot write `{rel}`: {e}")))?;
    file_changed(env, &rel, FileChangeKind::Modified).await;
    let n = if all { count } else { 1 };
    let mut out =
        ToolOutput::ok(format!("edited {rel}: replaced {n} occurrence(s)"), format!("edited {rel} ({n} change(s))"));
    out.files_changed.push(rel);
    Ok(out)
}

fn glob_matcher(glob: Option<&str>) -> Result<Option<globset::GlobMatcher>, ToolError> {
    glob.filter(|g| !g.trim().is_empty())
        .map(|g| {
            globset::GlobBuilder::new(g.trim())
                .literal_separator(false)
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| ToolError::new(format!("invalid glob: {e}")))
        })
        .transpose()
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// `list_dir`.
pub async fn list_dir(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let (abs, rel) = resolve(env, opt_str(args, "path").unwrap_or("."))?;
    if !abs.is_dir() {
        return Err(ToolError::new(format!("`{rel}` is not a directory")));
    }
    let depth = usize::try_from(opt_u64(args, "depth").unwrap_or(2).clamp(1, 5)).unwrap_or(2);
    let matcher = glob_matcher(opt_str(args, "glob"))?;
    let root = orkestr_security::paths::canonical_root(env.root()?).map_err(|e| ToolError::new(e.to_string()))?;
    let root = orkestr_security::paths::simplify(&root);
    let (lines, truncated) = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let walker = ignore::WalkBuilder::new(&abs)
            .max_depth(Some(depth))
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|e| e.file_name() != ".git")
            .sort_by_file_path(Path::cmp)
            .build();
        let mut truncated = false;
        for entry in walker.flatten() {
            if entry.depth() == 0 {
                continue;
            }
            let path = entry.path();
            let r = rel_of(&root, path);
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if let Some(m) = &matcher {
                if !is_dir && !m.is_match(entry.file_name()) && !m.is_match(&r) {
                    continue;
                }
                if is_dir {
                    continue;
                }
            }
            if out.len() >= MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            if is_dir {
                out.push(format!("{r}/"));
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(format!("{r} ({size} B)"));
            }
        }
        (out, truncated)
    })
    .await
    .map_err(|e| ToolError::new(format!("listing failed: {e}")))?;
    let mut content = if lines.is_empty() { format!("{rel}: (empty)") } else { lines.join("\n") };
    if truncated {
        content.push_str(&format!("\n… truncated at {MAX_LIST_ENTRIES} entries; narrow the path or glob"));
    }
    Ok(ToolOutput::ok(content, format!("listed {rel} ({} entries)", lines.len())))
}

/// `search_files`.
pub async fn search_files(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let pattern = arg_str(args, "pattern")?.to_owned();
    if pattern.is_empty() {
        return Err(ToolError::new("`pattern` must not be empty"));
    }
    let re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(opt_bool(args, "case_insensitive").unwrap_or(false))
        .size_limit(1 << 20)
        .build()
        .map_err(|e| ToolError::new(format!("invalid regular expression: {e}")))?;
    let (abs, rel) = resolve(env, opt_str(args, "path").unwrap_or("."))?;
    let max = usize::try_from(opt_u64(args, "max_results").unwrap_or(50).clamp(1, 200)).unwrap_or(50);
    let matcher = glob_matcher(opt_str(args, "glob"))?;
    let root = orkestr_security::paths::canonical_root(env.root()?).map_err(|e| ToolError::new(e.to_string()))?;
    let root = orkestr_security::paths::simplify(&root);
    let hits = tokio::task::spawn_blocking(move || {
        let mut hits: Vec<String> = Vec::new();
        let walker = ignore::WalkBuilder::new(&abs)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|e| e.file_name() != ".git")
            .sort_by_file_path(Path::cmp)
            .build();
        'files: for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let r = rel_of(&root, entry.path());
            if let Some(m) = &matcher {
                if !m.is_match(entry.file_name()) && !m.is_match(&r) {
                    continue;
                }
            }
            if entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > MAX_SEARCH_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else { continue };
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    let shown: String = line.trim().chars().take(200).collect();
                    hits.push(format!("{r}:{}: {shown}", i + 1));
                    if hits.len() >= max {
                        break 'files;
                    }
                }
            }
        }
        hits
    })
    .await
    .map_err(|e| ToolError::new(format!("search failed: {e}")))?;
    let content = if hits.is_empty() { format!("no matches for /{pattern}/ in {rel}") } else { hits.join("\n") };
    Ok(ToolOutput::ok(content, format!("{} match(es) for /{pattern}/", hits.len())))
}

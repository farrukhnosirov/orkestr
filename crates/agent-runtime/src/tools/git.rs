//! Git tools over `orkestr-vcs` (git CLI, argument vectors only):
//! `git_status`, `git_diff` (read-only, own worktree) and `git_commit`
//! (own agent branch only, never pushes).

use orkestr_protocol::methods::vcs::{CommitTrailer, DiffTarget, FileChangeStatus};
use orkestr_vcs::{DiffRequest, VcsError};
use serde_json::Value;

use super::{arg_str, opt_bool, opt_list, opt_str, touches_git_dir, ToolEnv, ToolError, ToolOutput};

fn vcs_err(e: VcsError) -> ToolError {
    ToolError::new(e.to_string())
}

fn code(s: FileChangeStatus) -> char {
    match s {
        FileChangeStatus::Unmodified => ' ',
        FileChangeStatus::Modified => 'M',
        FileChangeStatus::Added => 'A',
        FileChangeStatus::Deleted => 'D',
        FileChangeStatus::Renamed => 'R',
        FileChangeStatus::Copied => 'C',
        FileChangeStatus::TypeChanged => 'T',
        FileChangeStatus::Unmerged => 'U',
        FileChangeStatus::Untracked => '?',
        FileChangeStatus::Ignored => '!',
    }
}

/// `git_status`.
pub async fn status(env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let root = env.root()?;
    let st = env.deps.vcs.git().status(root).await.map_err(vcs_err)?;
    if !st.is_repo {
        return Err(ToolError::new("the worktree is not a git repository"));
    }
    let mut out = format!(
        "branch: {}{}\n",
        st.branch.as_deref().unwrap_or("(detached)"),
        st.head.as_deref().map(|h| format!(" @ {}", &h[..h.len().min(10)])).unwrap_or_default()
    );
    let entries: Vec<_> = st.entries.iter().filter(|e| e.worktree != FileChangeStatus::Ignored).collect();
    if entries.is_empty() {
        out.push_str("working tree clean\n");
    }
    for e in &entries {
        out.push_str(&format!("{}{} {}\n", code(e.index), code(e.worktree), e.path));
    }
    Ok(ToolOutput::ok(out, format!("{} changed file(s)", entries.len())))
}

/// `git_diff`.
pub async fn diff(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let root = env.root()?;
    let staged = opt_bool(args, "staged").unwrap_or(false);
    let paths: Vec<String> = opt_str(args, "path").map(|p| vec![p.replace('\\', "/")]).unwrap_or_default();
    let req = DiffRequest {
        target: if staged { DiffTarget::Staged } else { DiffTarget::Head },
        paths,
        context_lines: 3,
        include_untracked: !staged,
        max_patch_bytes: 64 * 1024,
    };
    let d = env.deps.vcs.git().diff(root, &req).await.map_err(vcs_err)?;
    if d.files.is_empty() {
        return Ok(ToolOutput::ok("no changes", "no changes"));
    }
    let mut out = format!("{} file(s), +{} -{}\n", d.files.len(), d.additions, d.deletions);
    for f in &d.files {
        if f.binary {
            out.push_str(&format!("binary file {}\n", f.path));
        } else {
            out.push_str(&f.patch);
            if !f.patch.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    Ok(ToolOutput::ok(out, format!("{} file(s), +{} -{}", d.files.len(), d.additions, d.deletions)))
}

/// `git_commit` — only on the agent's own branch.
pub async fn commit(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let root = env.root()?;
    let message = arg_str(args, "message")?.trim().to_owned();
    if message.is_empty() {
        return Err(ToolError::new("commit message must not be empty"));
    }
    let Some(own) = env.own_branch.as_deref() else {
        return Err(ToolError::new(
            "git_commit is only available with a dedicated agent worktree (own branch); this agent shares a checkout",
        ));
    };
    let git = env.deps.vcs.git();
    let current = git.current_branch(root).await.map_err(vcs_err)?;
    if current.as_deref() != Some(own) {
        return Err(ToolError::new(format!(
            "refusing to commit: the worktree is on `{}`, not on the agent branch `{own}`",
            current.as_deref().unwrap_or("(detached)")
        )));
    }
    let paths: Vec<String> = opt_list(args, "paths").into_iter().map(|p| p.replace('\\', "/")).collect();
    for p in &paths {
        if touches_git_dir(p) {
            return Err(ToolError::new("paths inside `.git` cannot be committed"));
        }
        orkestr_security::confine(root, p).map_err(|e| ToolError::new(e.to_string()))?;
    }
    git.stage(root, &paths, paths.is_empty()).await.map_err(vcs_err)?;
    let trailers = [
        CommitTrailer { key: "Orkestr-Agent".into(), value: env.agent_id.to_string() },
        CommitTrailer { key: "Orkestr-Task".into(), value: env.task_id.to_string() },
    ];
    match git.commit(root, &message, false, &trailers).await {
        Ok(c) => Ok(ToolOutput::ok(
            format!("committed {} on {own}: {}", c.short_id, c.subject),
            format!("commit {} on {own}", c.short_id),
        )),
        Err(VcsError::NothingToCommit) => Err(ToolError::new("nothing to commit")),
        Err(e) => Err(vcs_err(e)),
    }
}

//! The tool registry (agent-runtime-spec §5).
//!
//! Tool names follow `^[a-z][a-z0-9_]{0,63}$` (no dots — OpenAI/Gemini name
//! rules). Every call is validated against its JSON schema, mapped to a
//! permission [`Resource`] (paths confined to the agent root) and checked by
//! the `PermissionEngine` with `actor = Agent` **before** it executes — except
//! the control tools `ask_user` / `submit_result` (always allowed, spec §5.2)
//! and `run_command`, whose check happens in the terminal's security gate
//! (same engine, same actor) right before anything is spawned.

pub mod fs;
pub mod git;
pub mod memory;
pub mod schema;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{AgentId, Capability, ExecutionId, ProjectId, SnapshotId, TaskId};
use orkestr_security::Resource;
use serde_json::{json, Value};

use crate::deps::RuntimeDeps;

/// Tool class — drives the agent's visual state (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolClass {
    /// Reads / searches / diffs.
    Inspect,
    /// Changes files, runs mutating commands, commits.
    Mutate,
    /// Build / test / lint commands.
    Verify,
    /// Talks to the user / manager, ends the run, records knowledge.
    Control,
}

/// Static description of a tool.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    /// Name.
    pub name: &'static str,
    /// Description shown to the model.
    pub description: &'static str,
    /// JSON schema of the arguments (`additionalProperties: false`).
    pub input_schema: Value,
    /// Capability checked (None = always allowed control tool).
    pub capability: Option<Capability>,
    /// Class.
    pub class: ToolClass,
    /// Needs a filesystem root (unavailable with worktree policy `None`).
    pub needs_root: bool,
    /// Writes (unavailable with worktree policy `ReadOnlyMain`).
    pub writes: bool,
    /// Execution timeout.
    pub timeout: Duration,
}

/// Every built-in tool name (Phase 1).
pub const TOOL_NAMES: [&str; 14] = [
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "search_files",
    "run_command",
    "git_status",
    "git_diff",
    "git_commit",
    "create_snapshot",
    "ask_user",
    "record_memory",
    "record_decision",
    "submit_result",
];

/// Whether `name` is a known tool.
#[must_use]
pub fn is_known_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// Descriptor of a built-in tool.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn descriptor(name: &str) -> Option<ToolDescriptor> {
    let short = Duration::from_secs(60);
    let d = |name: &'static str,
             description: &'static str,
             input_schema: Value,
             capability: Option<Capability>,
             class: ToolClass,
             needs_root: bool,
             writes: bool| ToolDescriptor {
        name,
        description,
        input_schema,
        capability,
        class,
        needs_root,
        writes,
        timeout: short,
    };
    Some(match name {
        "read_file" => d(
            "read_file",
            "Read a text file of your worktree. Returns numbered lines. Use offset/limit (1-based line, max 2000 lines) for large files.",
            obj(
                json!({
                    "path": {"type": "string", "description": "Path relative to the worktree root"},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line (1-based)"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Number of lines"}
                }),
                &["path"],
            ),
            Some(Capability::Read),
            ToolClass::Inspect,
            true,
            false,
        ),
        "write_file" => d(
            "write_file",
            "Create or overwrite a text file in your worktree (parent folders are created). Prefer edit_file for small changes.",
            obj(
                json!({
                    "path": {"type": "string", "description": "Path relative to the worktree root"},
                    "content": {"type": "string", "description": "Full new file content"}
                }),
                &["path", "content"],
            ),
            Some(Capability::Write),
            ToolClass::Mutate,
            true,
            true,
        ),
        "edit_file" => d(
            "edit_file",
            "Replace an exact text snippet in a file. `old` must match exactly once unless replace_all is true. Include enough surrounding lines to make it unique.",
            obj(
                json!({
                    "path": {"type": "string", "description": "Path relative to the worktree root"},
                    "old": {"type": "string", "description": "Exact text to replace (non-empty)"},
                    "new": {"type": "string", "description": "Replacement text"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence"}
                }),
                &["path", "old", "new"],
            ),
            Some(Capability::Write),
            ToolClass::Mutate,
            true,
            true,
        ),
        "list_dir" => d(
            "list_dir",
            "List files and folders (gitignore-aware). Depth 1..5 (default 2), optional glob filter such as `*.rs`.",
            obj(
                json!({
                    "path": {"type": "string", "description": "Folder relative to the worktree root (default `.`)"},
                    "depth": {"type": "integer", "minimum": 1, "maximum": 5},
                    "glob": {"type": "string"}
                }),
                &[],
            ),
            Some(Capability::Read),
            ToolClass::Inspect,
            true,
            false,
        ),
        "search_files" => d(
            "search_files",
            "Search file contents with a regular expression (gitignore-aware). Returns `path:line: text` matches.",
            obj(
                json!({
                    "pattern": {"type": "string", "description": "Regular expression (Rust syntax)"},
                    "path": {"type": "string", "description": "Folder to search (default `.`)"},
                    "glob": {"type": "string", "description": "File filter, e.g. `*.ts`"},
                    "case_insensitive": {"type": "boolean"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 200}
                }),
                &["pattern"],
            ),
            Some(Capability::Read),
            ToolClass::Inspect,
            true,
            false,
        ),
        "run_command" => ToolDescriptor {
            timeout: Duration::from_secs(30 * 60),
            ..d(
                "run_command",
                "Run a shell command inside your worktree (non-interactive). Dangerous commands need user approval; forbidden ones are refused. Returns exit code, stdout and stderr.",
                obj(
                    json!({
                        "command": {"type": "string", "description": "The command line"},
                        "cwd": {"type": "string", "description": "Working folder relative to the worktree root"},
                        "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 1800}
                    }),
                    &["command"],
                ),
                Some(Capability::Terminal),
                ToolClass::Mutate,
                true,
                true,
            )
        },
        "git_status" => d(
            "git_status",
            "Show the git status of your worktree (branch and changed files).",
            obj(json!({}), &[]),
            Some(Capability::Read),
            ToolClass::Inspect,
            true,
            false,
        ),
        "git_diff" => d(
            "git_diff",
            "Show the diff of uncommitted changes in your worktree (or only staged changes).",
            obj(
                json!({
                    "staged": {"type": "boolean"},
                    "path": {"type": "string", "description": "Limit to one path"}
                }),
                &[],
            ),
            Some(Capability::Read),
            ToolClass::Inspect,
            true,
            false,
        ),
        "git_commit" => d(
            "git_commit",
            "Commit changes on your own agent branch (never pushes). Stages the given paths, or everything when omitted.",
            obj(
                json!({
                    "message": {"type": "string"},
                    "paths": {"type": "array", "items": {"type": "string"}}
                }),
                &["message"],
            ),
            Some(Capability::Git),
            ToolClass::Mutate,
            true,
            true,
        ),
        "create_snapshot" => d(
            "create_snapshot",
            "Create a restorable snapshot (checkpoint) of your worktree, including uncommitted files.",
            obj(json!({ "label": {"type": "string"} }), &[]),
            Some(Capability::Read),
            ToolClass::Control,
            true,
            false,
        ),
        "ask_user" => ToolDescriptor {
            timeout: Duration::from_secs(24 * 3600),
            ..d(
                "ask_user",
                "Ask the user a question and wait for the answer (approve/reject plus an optional note). Use sparingly, only when blocked.",
                obj(
                    json!({
                        "question": {"type": "string"},
                        "options": {"type": "array", "items": {"type": "string"}}
                    }),
                    &["question"],
                ),
                None,
                ToolClass::Control,
                false,
                false,
            )
        },
        "record_memory" => d(
            "record_memory",
            "Store a durable project memory item (convention, architecture fact, known issue, …) for future agents.",
            obj(
                json!({
                    "title": {"type": "string"},
                    "content": {"type": "string"},
                    "kind": {"type": "string", "enum": ["Requirement", "Architecture", "BusinessLogic", "Convention", "Schema", "Api", "Mistake", "Todo", "KnownIssue", "Note"]},
                    "tags": {"type": "array", "items": {"type": "string"}}
                }),
                &["title", "content"],
            ),
            Some(Capability::Write),
            ToolClass::Control,
            false,
            false,
        ),
        "record_decision" => d(
            "record_decision",
            "Record an architecture/design decision in the project decision log. rationale_summary is a short user-facing explanation.",
            obj(
                json!({
                    "title": {"type": "string"},
                    "decision": {"type": "string"},
                    "context": {"type": "string"},
                    "rationale_summary": {"type": "string"},
                    "alternatives": {"type": "array", "items": {"type": "string"}}
                }),
                &["title", "decision"],
            ),
            Some(Capability::Write),
            ToolClass::Control,
            false,
            false,
        ),
        "submit_result" => d(
            "submit_result",
            "Finish your assignment with a structured result. Always call this exactly once when done (or blocked).",
            obj(
                json!({
                    "status": {"type": "string", "enum": ["done", "partial", "blocked", "failed"]},
                    "summary": {"type": "string", "maxLength": 4000},
                    "files_changed": {"type": "array", "items": {"type": "string"}},
                    "tests_run": {"type": "array", "items": {"type": "object", "properties": {
                        "command": {"type": "string"}, "passed": {"type": "boolean"}, "summary": {"type": "string"}
                    }, "required": ["command"]}},
                    "open_issues": {"type": "array", "items": {"type": "string"}}
                }),
                &["summary"],
            ),
            None,
            ToolClass::Control,
            false,
            false,
        ),
        _ => return None,
    })
}

/// Everything a tool sees. Tools never receive the `SecretStore`.
#[derive(Clone)]
pub struct ToolEnv {
    /// Calling agent.
    pub agent_id: AgentId,
    /// Project.
    pub project_id: ProjectId,
    /// Task.
    pub task_id: TaskId,
    /// Execution.
    pub execution_id: ExecutionId,
    /// Confinement root (agent worktree / project root); None = no filesystem.
    pub root: Option<PathBuf>,
    /// The agent's own branch (dedicated worktree policy only).
    pub own_branch: Option<String>,
    /// Services.
    pub deps: Arc<RuntimeDeps>,
}

impl std::fmt::Debug for ToolEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolEnv").field("agent_id", &self.agent_id).field("root", &self.root).finish_non_exhaustive()
    }
}

impl ToolEnv {
    /// The confinement root or an error for root-less agents.
    pub fn root(&self) -> Result<&Path, ToolError> {
        self.root.as_deref().ok_or_else(|| ToolError::new("this agent has no filesystem access (worktree policy None)"))
    }
}

/// A tool's result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolOutput {
    /// Text returned to the model.
    pub content: String,
    /// One-line summary (events, UI).
    pub summary: String,
    /// Worktree-relative files changed by this call.
    pub files_changed: Vec<String>,
    /// The result is an error for the model (e.g. a failing command).
    pub is_error: bool,
    /// Snapshot created by this call.
    pub snapshot_id: Option<SnapshotId>,
    /// `(command, passed, summary)` of a verification command.
    pub test: Option<(String, bool, String)>,
}

impl ToolOutput {
    /// Successful output.
    #[must_use]
    pub fn ok(content: impl Into<String>, summary: impl Into<String>) -> Self {
        Self { content: content.into(), summary: summary.into(), ..Self::default() }
    }
}

/// A tool failure, returned to the model as an error result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    /// Message (safe to show to the model and the user).
    pub message: String,
}

impl ToolError {
    /// New error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

/// String argument.
pub fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| ToolError::new(format!("missing string argument `{key}`")))
}

/// Optional string argument.
#[must_use]
pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// Optional bool argument.
#[must_use]
pub fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

/// Optional unsigned argument.
#[must_use]
pub fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

/// Optional string list argument.
#[must_use]
pub fn opt_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `.git` (any case) anywhere in a relative path — git internals are only
/// reachable through the git tools (spec §6).
#[must_use]
pub fn touches_git_dir(path: &str) -> bool {
    path.split(['/', '\\']).any(|c| c.eq_ignore_ascii_case(".git"))
}

/// The permission check of a call: `(capability, resource)`, or `None` for
/// tools without a capability (control tools) and `run_command` (gated by the
/// terminal). Paths are validated here; escapes become hard denies in the
/// engine (the engine canonicalizes again against the root).
pub fn permission_target(name: &str, args: &Value) -> Result<Option<(Capability, Resource)>, ToolError> {
    let path = |key: &str, default: &str| -> Result<String, ToolError> {
        let p = opt_str(args, key).unwrap_or(default).trim().to_owned();
        if p.is_empty() {
            return Err(ToolError::new(format!("`{key}` must not be empty")));
        }
        Ok(p)
    };
    Ok(match name {
        "read_file" => Some((Capability::Read, Resource::Path { path: path("path", "")?, recursive: false })),
        "list_dir" | "search_files" => {
            Some((Capability::Read, Resource::Path { path: path("path", ".")?, recursive: false }))
        }
        "write_file" | "edit_file" => {
            let p = path("path", "")?;
            if touches_git_dir(&p) {
                return Err(ToolError::new("writing inside `.git` is not allowed — use git_commit"));
            }
            Some((Capability::Write, Resource::Path { path: p, recursive: false }))
        }
        "git_status" | "git_diff" | "create_snapshot" => {
            Some((Capability::Read, Resource::Path { path: ".".into(), recursive: false }))
        }
        "git_commit" => Some((
            Capability::Git,
            Resource::Git { args: vec!["commit".into(), "-m".into(), arg_str(args, "message")?.to_owned()] },
        )),
        "record_memory" => Some((Capability::Write, Resource::Other { value: "memory:project".into() })),
        "record_decision" => Some((Capability::Write, Resource::Other { value: "memory:decision".into() })),
        _ => None,
    })
}

/// Executes a plain (non-control, non-command) tool.
pub async fn execute(name: &str, args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    match name {
        "read_file" => fs::read_file(args, env).await,
        "write_file" => fs::write_file(args, env).await,
        "edit_file" => fs::edit_file(args, env).await,
        "list_dir" => fs::list_dir(args, env).await,
        "search_files" => fs::search_files(args, env).await,
        "git_status" => git::status(env).await,
        "git_diff" => git::diff(args, env).await,
        "git_commit" => git::commit(args, env).await,
        "create_snapshot" => memory::create_snapshot(args, env).await,
        "record_memory" => memory::record_memory(args, env).await,
        "record_decision" => memory::record_decision(args, env).await,
        other => Err(ToolError::new(format!("tool `{other}` is not executable here"))),
    }
}

/// Truncates text to `max` chars (keeping head and tail).
#[must_use]
pub fn truncate_middle(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let head = max * 2 / 3;
    let tail = max.saturating_sub(head).saturating_sub(40);
    let h: String = text.chars().take(head).collect();
    let t: String = text.chars().skip(n - tail).collect();
    format!("{h}\n… [{} characters omitted] …\n{t}", n - head - tail)
}

/// First line, at most `max` chars.
#[must_use]
pub fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_valid_descriptor() {
        let re = regex::Regex::new("^[a-z][a-z0-9_]{0,63}$").unwrap();
        for n in TOOL_NAMES {
            let d = descriptor(n).unwrap_or_else(|| panic!("{n}"));
            assert_eq!(d.name, n);
            assert!(re.is_match(n), "{n}");
            assert_eq!(d.input_schema["type"], "object");
            assert_eq!(d.input_schema["additionalProperties"], false);
            assert!(!d.description.is_empty());
        }
        assert!(descriptor("fs.read").is_none());
        assert!(!is_known_tool("rm_rf"));
    }

    #[test]
    fn permission_targets() {
        let (cap, res) = permission_target("write_file", &json!({"path": "src/a.rs", "content": ""})).unwrap().unwrap();
        assert_eq!(cap, Capability::Write);
        assert_eq!(res, Resource::Path { path: "src/a.rs".into(), recursive: false });
        assert!(permission_target("write_file", &json!({"path": ".git/config", "content": ""})).is_err());
        assert!(permission_target("edit_file", &json!({"path": "a/.GIT/x", "old": "a", "new": "b"})).is_err());
        assert!(permission_target("submit_result", &json!({"summary": "x"})).unwrap().is_none());
        assert!(permission_target("run_command", &json!({"command": "ls"})).unwrap().is_none());
        let (cap, _) = permission_target("git_commit", &json!({"message": "m"})).unwrap().unwrap();
        assert_eq!(cap, Capability::Git);
        assert!(permission_target("read_file", &json!({"path": "  "})).is_err());
    }

    #[test]
    fn truncation_helpers() {
        let long = "x".repeat(1000);
        let t = truncate_middle(&long, 200);
        assert!(t.chars().count() < 260 && t.contains("omitted"));
        assert_eq!(truncate_middle("short", 10), "short");
        assert_eq!(one_line("\n  hello world  \nnext", 5), "hello…");
    }
}

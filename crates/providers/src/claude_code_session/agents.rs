//! Claude Code sub-agents: markdown definitions (`~/.claude/agents/*.md`,
//! `<project>/.claude/agents/*.md` — YAML front matter `name`, `description`,
//! `tools`, `model` + the system prompt as the body) and the CLI's built-in
//! agents (the `agents` list of its `initialize` answer).
//!
//! Only these definition files are read or written here; the CLI's
//! credentials and settings are never touched.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use orkestr_protocol::methods::claude_engine::{ClaudeAgentDef, ClaudeAgentScope};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::control;

/// Largest agent file read (bytes).
pub const MAX_AGENT_FILE: u64 = 256 * 1024;
/// Most agent files read per directory.
pub const MAX_AGENT_FILES: usize = 200;

/// Whether `name` is a valid agent / file name (`[a-z0-9-_]`, 1..=64,
/// starts with a letter or digit).
#[must_use]
pub fn is_valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        let inner = &v[1..v.len() - 1];
        if v.starts_with('"') {
            return inner.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\");
        }
        return inner.replace("''", "'");
    }
    v.to_owned()
}

fn list_value(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches('[').trim_end_matches(']');
    v.split(',').map(unquote).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

/// Parses an agent file (pure; tested). `None` without front matter or a name.
#[must_use]
pub fn parse_agent_file(text: &str, scope: ClaudeAgentScope, path: Option<&Path>) -> Option<ClaudeAgentDef> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let (front, body) = (&rest[..end], &rest[end + 4..]);
    let body = body.strip_prefix('\n').unwrap_or(body);
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut lists: Vec<(String, Vec<String>)> = Vec::new();
    let lines: Vec<&str> = front.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line.starts_with(' ') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().to_owned();
        let value = value.trim();
        if value == "|" || value == ">" || value == "|-" || value == ">-" {
            // Block scalar: indented continuation lines.
            let mut parts = Vec::new();
            while i < lines.len() && (lines[i].starts_with(' ') || lines[i].trim().is_empty()) {
                parts.push(lines[i].trim());
                i += 1;
            }
            let joined = if value.starts_with('|') { parts.join("\n") } else { parts.join(" ") };
            fields.push((key, joined.trim().to_owned()));
        } else if value.is_empty() {
            // YAML list on the following lines.
            let mut items = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with("- ") {
                items.push(unquote(lines[i].trim_start().trim_start_matches("- ")));
                i += 1;
            }
            lists.push((key, items));
        } else {
            fields.push((key, value.to_owned()));
        }
    }
    let field = |k: &str| fields.iter().find(|(key, _)| key == k).map(|(_, v)| unquote(v));
    let name = field("name").filter(|n| !n.trim().is_empty()).or_else(|| {
        path.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned())
    })?;
    let tools = lists
        .iter()
        .find(|(k, _)| k == "tools")
        .map(|(_, v)| v.clone())
        .or_else(|| fields.iter().find(|(k, _)| k == "tools").map(|(_, v)| list_value(v)))
        .unwrap_or_default();
    Some(ClaudeAgentDef {
        name: name.trim().to_owned(),
        description: field("description").unwrap_or_default(),
        tools,
        model: field("model").filter(|m| !m.is_empty()),
        prompt: body.trim().to_owned(),
        scope,
        path: path.map(|p| p.display().to_string()),
        template_id: None,
    })
}

fn yaml_string(s: &str) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("\"{}\"", one_line.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Renders an agent file (pure; tested — round-trips through [`parse_agent_file`]).
#[must_use]
pub fn render_agent_file(name: &str, description: &str, tools: &[String], model: Option<&str>, prompt: &str) -> String {
    let mut s = format!("---\nname: {name}\ndescription: {}\n", yaml_string(description));
    if !tools.is_empty() {
        s.push_str(&format!("tools: {}\n", tools.join(", ")));
    }
    if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
        s.push_str(&format!("model: {m}\n"));
    }
    s.push_str("---\n\n");
    s.push_str(prompt.trim());
    s.push('\n');
    s
}

/// The agents directory of a scope root (`<root>/.claude/agents`).
#[must_use]
pub fn agents_dir(root: &Path) -> PathBuf {
    root.join(".claude").join("agents")
}

/// Reads every `*.md` agent below `dir` (sorted by name; unreadable or
/// oversized files are skipped).
#[must_use]
pub fn read_dir(dir: &Path, scope: ClaudeAgentScope) -> Vec<ClaudeAgentDef> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<ClaudeAgentDef> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("md")))
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_AGENT_FILE))
        .take(MAX_AGENT_FILES)
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            parse_agent_file(&text, scope, Some(&e.path()))
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Built-in agents from an `initialize` answer: `(name, description, model)`.
#[must_use]
pub fn builtin_from_initialize(response: &Value) -> Vec<ClaudeAgentDef> {
    response
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let name = a.get("name").and_then(Value::as_str)?.trim().to_owned();
            if name.is_empty() {
                return None;
            }
            Some(ClaudeAgentDef {
                name,
                description: a.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
                tools: a
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(|t| t.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_default(),
                model: a.get("model").and_then(Value::as_str).map(str::to_owned),
                prompt: String::new(),
                scope: ClaudeAgentScope::BuiltIn,
                path: None,
                template_id: None,
            })
        })
        .collect()
}

/// `--agents` JSON definition of a file agent.
#[must_use]
pub fn cli_definition(def: &ClaudeAgentDef) -> Option<Value> {
    if def.prompt.trim().is_empty() {
        return None;
    }
    let mut v = json!({"description": def.description, "prompt": def.prompt});
    if !def.tools.is_empty() {
        v["tools"] = json!(def.tools);
    }
    if let Some(m) = def.model.as_deref().filter(|m| *m != "inherit") {
        v["model"] = json!(m);
    }
    Some(v)
}

/// Asks the CLI for its `initialize` answer (commands, models, agents) in
/// `cwd` and stops it. No user turn is sent: nothing is spent.
pub async fn probe_initialize(program: &Path, cwd: &Path) -> Result<Value, String> {
    let args: Vec<String> =
        ["--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
    let mut cmd = super::manager::sanitized_command(program, &args);
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("could not run `{}`: {e}", program.display()))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let line = control::control_request("orkestr-probe-1", control::initialize());
    stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())?;
    let read = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
            if v.get("type").and_then(Value::as_str) == Some("control_response")
                && v.pointer("/response/request_id").and_then(Value::as_str) == Some("orkestr-probe-1")
            {
                return v.pointer("/response/response").cloned().ok_or_else(|| {
                    v.pointer("/response/error").and_then(Value::as_str).unwrap_or("initialize failed").to_owned()
                });
            }
        }
        Err("Claude Code ended before answering initialize".to_owned())
    };
    let out = tokio::time::timeout(Duration::from_secs(45), read).await;
    drop(stdin);
    let _ = child.kill().await;
    out.map_err(|_| "Claude Code did not answer initialize in time".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter_variants() {
        let text = "---\nname: code-reviewer\ndescription: \"Reviews code: style, bugs\"\ntools: Read, Grep, Glob\nmodel: sonnet\n---\n\nYou are a careful reviewer.\nBe brief.\n";
        let a = parse_agent_file(text, ClaudeAgentScope::Project, None).unwrap();
        assert_eq!(a.name, "code-reviewer");
        assert_eq!(a.description, "Reviews code: style, bugs");
        assert_eq!(a.tools, vec!["Read", "Grep", "Glob"]);
        assert_eq!(a.model.as_deref(), Some("sonnet"));
        assert_eq!(a.prompt, "You are a careful reviewer.\nBe brief.");

        let list = "---\r\nname: tester\r\ndescription: >\r\n  Runs the tests\r\n  and reports\r\ntools:\r\n  - Bash\r\n  - Read\r\n---\r\nPrompt";
        let b = parse_agent_file(list, ClaudeAgentScope::User, None).unwrap();
        assert_eq!(b.description, "Runs the tests and reports");
        assert_eq!(b.tools, vec!["Bash", "Read"]);
        assert_eq!(b.model, None);
        assert_eq!(b.prompt, "Prompt");

        let no_name = "---\ndescription: x\n---\nbody";
        assert_eq!(
            parse_agent_file(no_name, ClaudeAgentScope::User, Some(Path::new("/a/helper.md"))).unwrap().name,
            "helper"
        );
        assert!(parse_agent_file(no_name, ClaudeAgentScope::User, None).is_none());
        assert!(parse_agent_file("no front matter", ClaudeAgentScope::User, None).is_none());
    }

    #[test]
    fn render_round_trips() {
        let tools = vec!["Read".to_owned(), "Edit".to_owned()];
        let text = render_agent_file("doc-writer", "Writes \"docs\"\nfor APIs", &tools, Some("haiku"), "Write docs.\n\nKeep it short.");
        let a = parse_agent_file(&text, ClaudeAgentScope::Project, None).unwrap();
        assert_eq!(a.name, "doc-writer");
        assert_eq!(a.description, "Writes \"docs\" for APIs");
        assert_eq!(a.tools, tools);
        assert_eq!(a.model.as_deref(), Some("haiku"));
        assert_eq!(a.prompt, "Write docs.\n\nKeep it short.");
        let def = cli_definition(&a).unwrap();
        assert_eq!(def["model"], "haiku");
        assert_eq!(def["tools"][1], "Edit");
    }

    #[test]
    fn names_and_dirs() {
        assert!(is_valid_file_name("code-reviewer"));
        assert!(!is_valid_file_name("../evil"));
        assert!(!is_valid_file_name("Has Space"));
        assert!(!is_valid_file_name("-x"));
        let tmp = tempfile::tempdir().unwrap();
        let dir = agents_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.md"), render_agent_file("b", "B", &[], None, "pb")).unwrap();
        std::fs::write(dir.join("a.md"), render_agent_file("a", "A", &[], None, "pa")).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let all = read_dir(&dir, ClaudeAgentScope::Project);
        assert_eq!(all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(all[0].path.as_deref().unwrap().ends_with("a.md"));
    }

    #[test]
    fn builtins_from_the_recorded_initialize_answer() {
        let fixture = include_str!("../../tests/fixtures/claude_code/initialize.jsonl");
        let resp = fixture
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|v| v["type"] == "control_response")
            .unwrap();
        let agents = builtin_from_initialize(&resp["response"]["response"]);
        assert!(agents.iter().any(|a| a.name == "claude-code-guide" && a.model.as_deref() == Some("haiku")));
        assert!(agents.iter().all(|a| a.scope == ClaudeAgentScope::BuiltIn && a.prompt.is_empty()));
    }
}

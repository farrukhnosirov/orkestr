//! Past conversations of a folder, from the CLI's own per-project session
//! transcripts (`<projects dir>/<encoded cwd>/<session id>.jsonl`).
//!
//! Only these transcript files are read — never the CLI's credentials or
//! settings. The projects directory comes from `claude auth status`
//! (`projectsDirectory`) or the documented default (`~/.claude/projects`,
//! `$CLAUDE_CONFIG_DIR/projects`).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use orkestr_protocol::methods::claude_code::{ClaudeCodeEvent, ClaudeHistoryEntry};
use serde_json::Value;

use super::parse::{Inbound, StreamParser};

/// Newest conversations listed per folder.
pub const MAX_HISTORY: usize = 60;
/// Transcript lines larger than this are skipped (huge tool outputs).
const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
/// Most events returned for one transcript (the newest are kept).
pub const MAX_TRANSCRIPT_EVENTS: usize = 3000;

/// The CLI's folder name for a working directory: every character that is
/// not an ASCII letter or digit becomes `-`.
#[must_use]
pub fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Default projects directory when the CLI did not report one.
#[must_use]
pub fn default_projects_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("projects"));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".claude").join("projects"))
}

/// The transcript folder of `cwd` (case-insensitive match: drive letters
/// are spelled differently by different clients).
#[must_use]
pub fn folder_for(projects_dir: &Path, cwd: &Path) -> Option<PathBuf> {
    let name = encode_cwd(cwd);
    let exact = projects_dir.join(&name);
    if exact.is_dir() {
        return Some(exact);
    }
    std::fs::read_dir(projects_dir).ok()?.filter_map(Result::ok).find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.eq_ignore_ascii_case(&name) && e.path().is_dir()).then(|| e.path())
    })
}

/// Whether `id` looks like a CLI session id (UUID characters only — never a path).
#[must_use]
pub fn is_session_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn shorten(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > max {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    } else {
        one_line
    }
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    let reader = File::open(path).ok().map(BufReader::new);
    reader.into_iter().flat_map(|r| r.split(b'\n')).filter_map(|l| {
        let l = l.ok()?;
        (l.len() <= MAX_LINE_BYTES).then(|| String::from_utf8_lossy(&l).into_owned())
    })
}

fn first_prompt(v: &Value) -> Option<String> {
    let text = match v.pointer("/message/content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("[Request interrupted")).then(|| shorten(t, 160))
}

/// Summary of one transcript file.
#[must_use]
pub fn summarize(path: &Path) -> Option<ClaudeHistoryEntry> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    if !is_session_id(&id) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let updated_at: DateTime<Utc> = meta.modified().ok()?.into();
    let (mut title, mut prompt, mut created, mut messages) = (None, None, None, 0u32);
    for line in lines(path) {
        // Cheap prefilter: only a few line types matter here.
        let wanted = line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
            || line.contains("\"type\":\"ai-title\"")
            || line.contains("\"type\":\"summary\"");
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") => title = v.get("aiTitle").and_then(Value::as_str).map(|t| shorten(t, 120)).or(title),
            Some("summary") if title.is_none() => {
                title = v.get("summary").and_then(Value::as_str).map(|t| shorten(t, 120))
            }
            Some(kind @ ("user" | "assistant")) => {
                if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
                    || v.get("isMeta").and_then(Value::as_bool) == Some(true)
                {
                    continue;
                }
                messages = messages.saturating_add(1);
                if created.is_none() {
                    created = v
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.with_timezone(&Utc));
                }
                if kind == "user" && prompt.is_none() {
                    prompt = first_prompt(&v);
                }
            }
            _ => {}
        }
    }
    (messages > 0).then_some(ClaudeHistoryEntry {
        claude_session_id: id,
        title,
        first_prompt: prompt,
        created_at: created,
        updated_at,
        messages,
    })
}

/// Past conversations in a transcript folder, newest first.
#[must_use]
pub fn list(folder: &Path) -> Vec<ClaudeHistoryEntry> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl") && e.path().is_file())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().take(MAX_HISTORY).filter_map(|(_, p)| summarize(&p)).collect()
}

fn transcript_path(folder: &Path, session_id: &str) -> Result<PathBuf, String> {
    if !is_session_id(session_id) {
        return Err("invalid session id".into());
    }
    let path = folder.join(format!("{session_id}.jsonl"));
    if !path.is_file() {
        return Err(format!("conversation {session_id} not found"));
    }
    Ok(path)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The entries (`uuid`s) on the conversation's active branch: from the last
/// main-chain message back to the root over `parentUuid` (across compaction
/// via `logicalParentUuid`). A rewound conversation (`--resume-session-at`)
/// keeps its abandoned turns in the file; they are not on this chain.
/// `None` = no usable chain (no uuids, or a parent outside the file) — then
/// nothing is filtered.
fn active_chain(path: &Path) -> Option<std::collections::HashSet<String>> {
    let mut parents: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let mut leaf: Option<String> = None;
    for line in lines(path) {
        if !line.contains("\"uuid\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(uuid) = str_field(&v, "uuid") else { continue };
        let parent = str_field(&v, "parentUuid").or_else(|| str_field(&v, "logicalParentUuid"));
        let kind = v.get("type").and_then(Value::as_str);
        if matches!(kind, Some("user" | "assistant")) && v.get("isSidechain").and_then(Value::as_bool) != Some(true) {
            leaf = Some(uuid.clone());
        }
        parents.insert(uuid, parent);
    }
    let mut chain = std::collections::HashSet::new();
    let mut cur = leaf;
    while let Some(id) = cur {
        if !chain.insert(id.clone()) {
            break; // cycle
        }
        // A parent outside this file: no usable chain.
        cur = parents.get(&id)?.clone();
    }
    (!chain.is_empty()).then_some(chain)
}

/// One user message of a conversation's active branch (a rewind target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTurn {
    /// The message's transcript entry.
    pub uuid: String,
    /// The entry before it (`None` = the first message of the conversation).
    pub parent_uuid: Option<String>,
    /// Text as the panel shows it.
    pub text: String,
}

/// The user messages of a conversation's active branch, oldest first —
/// exactly the ones [`transcript`] shows as `UserMessage`.
pub fn user_turns(folder: &Path, session_id: &str) -> Result<Vec<UserTurn>, String> {
    let path = transcript_path(folder, session_id)?;
    let chain = active_chain(&path);
    let mut parser = StreamParser::transcript();
    let mut out = Vec::new();
    for line in lines(&path) {
        if !line.contains("\"type\":\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(uuid) = str_field(&v, "uuid") else { continue };
        if chain.as_ref().is_some_and(|c| !c.contains(&uuid)) {
            continue;
        }
        for item in parser.value(&v) {
            if let Inbound::Event(ClaudeCodeEvent::UserMessage { text }) = item {
                out.push(UserTurn { uuid: uuid.clone(), parent_uuid: str_field(&v, "parentUuid"), text });
            }
        }
    }
    Ok(out)
}

/// The chat items of one conversation (oldest first, capped to the newest
/// [`MAX_TRANSCRIPT_EVENTS`]); `(events, truncated)`. Only the active branch
/// is shown (turns abandoned by a rewind are skipped).
pub fn transcript(folder: &Path, session_id: &str) -> Result<(Vec<ClaudeCodeEvent>, bool), String> {
    let path = transcript_path(folder, session_id)?;
    let chain = active_chain(&path);
    let mut parser = StreamParser::transcript();
    let mut events = std::collections::VecDeque::new();
    let mut truncated = false;
    for line in lines(&path) {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let (Some(c), Some(uuid)) = (chain.as_ref(), v.get("uuid").and_then(Value::as_str)) {
            if !c.contains(uuid) {
                continue;
            }
        }
        for item in parser.value(&v) {
            if let Inbound::Event(e) = item {
                if matches!(e, ClaudeCodeEvent::FileChanged { .. }) {
                    continue; // historical, already on disk
                }
                events.push_back(e);
                if events.len() > MAX_TRANSCRIPT_EVENTS {
                    events.pop_front();
                    truncated = true;
                }
            }
        }
    }
    Ok((events.into_iter().collect(), truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_encoding_matches_the_cli() {
        assert_eq!(
            encode_cwd(Path::new(r"C:\Users\user\Documents\Projects\orkestr")),
            "C--Users-user-Documents-Projects-orkestr"
        );
        assert_eq!(encode_cwd(Path::new("/home/me/my proj.v2")), "-home-me-my-proj-v2");
    }

    #[test]
    fn session_ids_are_never_paths() {
        assert!(is_session_id("06a417de-ea3e-4314-9648-2072200e551d"));
        assert!(!is_session_id("../../secret"));
        assert!(!is_session_id("abc"));
        assert!(!is_session_id("06a417de-ea3e-4314-9648-2072200e551d.jsonl"));
    }

    #[test]
    fn rewound_turns_leave_the_active_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let sid = "11111111-2222-3333-4444-555555555555";
        let line = |uuid: &str, parent: Option<&str>, kind: &str, content: &str| {
            serde_json::json!({
                "type": kind, "uuid": uuid, "parentUuid": parent,
                "message": { "role": kind, "content": [{ "type": "text", "text": content }] },
            })
            .to_string()
        };
        let body = [
            line("aaaaaaaa-0000-0000-0000-000000000001", None, "user", "first"),
            line(
                "aaaaaaaa-0000-0000-0000-000000000002",
                Some("aaaaaaaa-0000-0000-0000-000000000001"),
                "assistant",
                "one",
            ),
            line(
                "aaaaaaaa-0000-0000-0000-000000000003",
                Some("aaaaaaaa-0000-0000-0000-000000000002"),
                "user",
                "second",
            ),
            line(
                "aaaaaaaa-0000-0000-0000-000000000004",
                Some("aaaaaaaa-0000-0000-0000-000000000003"),
                "assistant",
                "two",
            ),
            // Rewound to before "second", then a new message.
            line("aaaaaaaa-0000-0000-0000-000000000005", Some("aaaaaaaa-0000-0000-0000-000000000002"), "user", "again"),
        ]
        .join("\n");
        std::fs::write(tmp.path().join(format!("{sid}.jsonl")), body).unwrap();
        let turns = user_turns(tmp.path(), sid).unwrap();
        let texts: Vec<&str> = turns.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["first", "again"]);
        assert_eq!(turns[0].parent_uuid, None);
        assert_eq!(turns[1].parent_uuid.as_deref(), Some("aaaaaaaa-0000-0000-0000-000000000002"));
        let (events, _) = transcript(tmp.path(), sid).unwrap();
        assert!(!events.iter().any(|e| matches!(e, ClaudeCodeEvent::UserMessage { text } if text == "second")));
        assert!(!events.iter().any(|e| matches!(e, ClaudeCodeEvent::Text { text, .. } if text == "two")));
    }

    #[test]
    fn lists_and_reads_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let cwd = Path::new(r"C:\Work\Demo");
        let folder = projects.join("c--Work-Demo"); // lower-case drive, as VS Code writes it
        std::fs::create_dir_all(&folder).unwrap();
        let sid = "11111111-2222-3333-4444-555555555555";
        let body = [
            r#"{"type":"queue-operation","operation":"enqueue"}"#,
            r#"{"type":"user","message":{"role":"user","content":"Add a README"},"timestamp":"2026-10-01T10:00:00.000Z","isSidechain":false}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hidden"},{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"C:\\Work\\Demo\\README.md","content":"x"}}]},"timestamp":"2026-10-01T10:00:01.000Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"File created"}]},"timestamp":"2026-10-01T10:00:02.000Z"}"#,
            r#"{"type":"ai-title","aiTitle":"Add README","sessionId":"x"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Created README.md"}]},"timestamp":"2026-10-01T10:00:03.000Z"}"#,
        ]
        .join("\n");
        std::fs::write(folder.join(format!("{sid}.jsonl")), body).unwrap();
        std::fs::write(folder.join("not-a-session.jsonl"), "{}").unwrap();
        std::fs::create_dir_all(folder.join("memory")).unwrap();

        let found = folder_for(&projects, cwd).expect("case-insensitive folder match");
        let list = list(&found);
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(e.claude_session_id, sid);
        assert_eq!(e.title.as_deref(), Some("Add README"));
        assert_eq!(e.first_prompt.as_deref(), Some("Add a README"));
        assert_eq!(e.messages, 4);
        assert!(e.created_at.is_some());

        let (events, truncated) = transcript(&found, sid).unwrap();
        assert!(!truncated);
        assert!(matches!(&events[0], ClaudeCodeEvent::UserMessage { text } if text == "Add a README"));
        assert!(matches!(&events[1], ClaudeCodeEvent::ToolUse { name, .. } if name == "Write"));
        assert!(matches!(&events[2], ClaudeCodeEvent::ToolResult { .. }));
        assert!(matches!(&events[3], ClaudeCodeEvent::Text { text, .. } if text == "Created README.md"));
        assert_eq!(events.len(), 4, "no FileChanged replay, no thinking");
        assert!(!format!("{events:?}").contains("hidden"));
        assert!(transcript(&found, "../x").is_err());
        assert!(transcript(&found, "99999999-2222-3333-4444-555555555555").is_err());
    }
}

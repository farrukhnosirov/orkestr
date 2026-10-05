//! AI-assisted conflict resolution: prompt building and reply parsing.
//! The model only *proposes* content; it is stored as a suggestion and
//! applied when the user accepts it (`merge.resolveConflict` with `Content`).

use orkestr_protocol::methods::merge::MergeFileConflict;
use orkestr_providers::adapters::mock_merge::{CONFLICT_BEGIN, CONFLICT_END, MERGE_RESOLVE_MARKER};

/// Max characters of one side included in the prompt.
pub const MAX_PROMPT_SIDE: usize = 40_000;

/// System prompt (carries the marker the offline Mock keys on).
#[must_use]
pub fn system_prompt() -> String {
    format!(
        "{MERGE_RESOLVE_MARKER}\n\
         You are the merge assistant of Orkestr's Merge Manager. You resolve one git merge conflict.\n\
         Rules:\n\
         - Produce the complete resolved file that keeps the intent of BOTH sides.\n\
         - Never leave conflict markers (<<<<<<<, =======, >>>>>>>, |||||||).\n\
         - Do not invent unrelated changes; keep formatting and line endings.\n\
         - Reply with exactly one JSON object: {{\"content\": \"<full resolved file>\", \"note\": \"<one short sentence>\"}}.\n\
         - Never include hidden reasoning; the note is shown to the user."
    )
}

fn cut(s: &str) -> &str {
    if s.len() <= MAX_PROMPT_SIDE {
        return s;
    }
    let mut end = MAX_PROMPT_SIDE;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// User message for one conflicted file.
#[must_use]
pub fn user_prompt(conflict: &MergeFileConflict, ours_branch: &str, theirs_branch: &str) -> String {
    let side = |label: &str, v: &Option<String>| match v {
        Some(t) => format!("## {label}\n```\n{}\n```\n", cut(t)),
        None => format!("## {label}\n(file absent)\n"),
    };
    // The working copy sits verbatim between the two marker lines.
    let result = cut(conflict.result.as_deref().unwrap_or(""));
    let nl = if result.is_empty() || result.ends_with('\n') { "" } else { "\n" };
    format!(
        "File: {path}\n\n{base}\n{ours}\n{theirs}\n## Conflicted working copy\n{CONFLICT_BEGIN}\n{result}{nl}{CONFLICT_END}\n",
        path = conflict.path,
        base = side("Common ancestor (base)", &conflict.base),
        ours = side(&format!("Ours — integration branch (already merged: {ours_branch})"), &conflict.ours),
        theirs = side(&format!("Theirs — incoming branch {theirs_branch}"), &conflict.theirs),
    )
}

/// Whether `text` still contains git conflict markers.
#[must_use]
pub fn has_conflict_markers(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> ") || l == "<<<<<<<" || l == ">>>>>>>")
}

/// Parses `{"content", "note"}` from a model reply (tolerates prose or a
/// code fence around the JSON object).
pub fn parse_reply(text: &str) -> Result<(String, Option<String>), String> {
    let start = text.find('{').ok_or("the model reply contains no JSON object")?;
    let end = text.rfind('}').ok_or("the model reply contains no JSON object")?;
    if end < start {
        return Err("the model reply contains no JSON object".into());
    }
    let v: serde_json::Value =
        serde_json::from_str(&text[start..=end]).map_err(|e| format!("the model reply is not valid JSON: {e}"))?;
    let content = v.get("content").and_then(serde_json::Value::as_str).ok_or("the model reply has no `content`")?;
    if has_conflict_markers(content) {
        return Err("the suggested content still contains conflict markers".into());
    }
    let note = v.get("note").and_then(serde_json::Value::as_str).map(|n| n.chars().take(500).collect());
    Ok((content.to_owned(), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json_and_rejects_markers() {
        let (c, n) = parse_reply("Sure:\n```json\n{\"content\":\"a\\nb\\n\",\"note\":\"kept both\"}\n```").unwrap();
        assert_eq!(c, "a\nb\n");
        assert_eq!(n.as_deref(), Some("kept both"));
        assert!(parse_reply("{\"content\":\"<<<<<<< x\\n\"}").is_err());
        assert!(parse_reply("no json").is_err());
    }

    #[test]
    fn prompt_contains_the_marker_and_the_working_copy() {
        let c = MergeFileConflict {
            path: "a.txt".into(),
            base: None,
            ours: Some("x".into()),
            theirs: Some("y".into()),
            result: Some("<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n".into()),
            suggestion: None,
            suggestion_note: None,
            binary: false,
            resolved: false,
        };
        assert!(system_prompt().contains(MERGE_RESOLVE_MARKER));
        let u = user_prompt(&c, "agent/a", "agent/b");
        assert!(u.contains(CONFLICT_BEGIN) && u.contains("(file absent)") && u.contains("agent/b"));
    }
}

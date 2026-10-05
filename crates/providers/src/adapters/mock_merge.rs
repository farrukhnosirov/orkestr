//! Mock (offline demo) extension: merge-conflict resolution for the Merge
//! Manager (§16). A request whose system prompt contains
//! [`MERGE_RESOLVE_MARKER`] gets a deterministic *union* resolution of the
//! conflicted file found between [`CONFLICT_BEGIN`] and [`CONFLICT_END`] in the
//! last user message: for every conflict block the "ours" lines, then the
//! "theirs" lines that are not already present; the diff3 base section is
//! dropped. Reply: `{"content": "...", "note": "..."}`.

use serde_json::json;

use super::mock::MockScript;
use crate::types::{ChatRequest, Role};

/// System-prompt marker of a merge-resolution request.
pub const MERGE_RESOLVE_MARKER: &str = "ORKESTR_MERGE_RESOLVE";
/// Line that opens the conflicted file in the user message.
pub const CONFLICT_BEGIN: &str = "----- BEGIN CONFLICTED FILE -----";
/// Line that closes the conflicted file in the user message.
pub const CONFLICT_END: &str = "----- END CONFLICTED FILE -----";

/// Scripted reply for merge-resolution requests (`None` = not one).
#[must_use]
pub fn script(req: &ChatRequest) -> Option<MockScript> {
    if !req.system_prompt().is_some_and(|s| s.contains(MERGE_RESOLVE_MARKER)) {
        return None;
    }
    let user = req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.as_str()).unwrap_or("");
    let body = user
        .split_once(CONFLICT_BEGIN)
        .and_then(|(_, rest)| rest.rsplit_once(CONFLICT_END).map(|(b, _)| b))
        .map(|b| b.strip_prefix('\n').unwrap_or(b))
        .unwrap_or("");
    let (content, blocks) = union_resolve(body);
    Some(MockScript::Text(
        json!({
            "content": content,
            "note": format!("Mock (offline demo): kept both sides of {blocks} conflict block(s), ours first."),
        })
        .to_string(),
    ))
}

/// Union resolution of a file with git conflict markers. Returns the text
/// and the number of conflict blocks.
#[must_use]
pub fn union_resolve(text: &str) -> (String, usize) {
    #[derive(PartialEq)]
    enum S {
        Out,
        Ours,
        Base,
        Theirs,
    }
    let mut state = S::Out;
    let mut out: Vec<&str> = Vec::new();
    let mut ours: Vec<&str> = Vec::new();
    let mut theirs: Vec<&str> = Vec::new();
    let mut blocks = 0usize;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        match state {
            S::Out if bare.starts_with("<<<<<<<") => {
                state = S::Ours;
                ours.clear();
                theirs.clear();
            }
            S::Out => out.push(line),
            S::Ours if bare.starts_with("|||||||") => state = S::Base,
            S::Ours | S::Base if bare.starts_with("=======") => state = S::Theirs,
            S::Ours => ours.push(line),
            S::Base => {}
            S::Theirs if bare.starts_with(">>>>>>>") => {
                blocks += 1;
                out.extend(ours.iter().copied());
                for t in &theirs {
                    let tb = t.trim_end_matches(['\n', '\r']);
                    if !ours.iter().any(|o| o.trim_end_matches(['\n', '\r']) == tb) {
                        out.push(t);
                    }
                }
                state = S::Out;
            }
            S::Theirs => theirs.push(line),
        }
    }
    (out.concat(), blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    #[test]
    fn union_keeps_both_sides_without_markers() {
        let t = "a\n<<<<<<< ours\nX\nsame\n||||||| base\nB\n=======\nY\nsame\n>>>>>>> theirs\nz\n";
        let (r, n) = union_resolve(t);
        assert_eq!(n, 1);
        assert_eq!(r, "a\nX\nsame\nY\nz\n");
    }

    #[test]
    fn marker_selects_the_script() {
        let mut req = ChatRequest::new(
            "mock-smart",
            vec![ChatMessage::user(format!(
                "file\n{CONFLICT_BEGIN}\n<<<<<<< a\n1\n=======\n2\n>>>>>>> b\n{CONFLICT_END}\n"
            ))],
        );
        assert!(script(&req).is_none());
        req.system = Some(format!("{MERGE_RESOLVE_MARKER}\nresolve"));
        let Some(MockScript::Text(t)) = script(&req) else { panic!("expected text") };
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["content"], "1\n2\n");
    }
}

//! Scripted Mock behaviour for the computer-assistant planner (ADR-0035).
//! The planner's system prompt carries the marker "Orkestr's computer
//! assistant planner"; the Mock answers with a deterministic plan built from
//! the action vocabulary so the offline demo / acceptance runs reach
//! approve → run → remember. Note-like tasks open Notepad and type the text
//! in a new document (Ctrl+N — never into a restored tab; corrections are
//! appended); anything else becomes a web search. Clearly a
//! Mock answer ("Mock (offline demo)" in the explanation).

use serde_json::json;

use super::mock::MockScript;
use crate::types::{ChatRequest, Role};

const MARKER: &str = "Orkestr's computer assistant planner";

fn after_verb(task: &str) -> Option<String> {
    let lower = task.to_lowercase();
    for v in ["напиши ", "запиши ", "напечатай ", "write ", "type "] {
        if let Some(i) = lower.find(v) {
            let start = task.char_indices().nth(lower[..i + v.len()].chars().count()).map_or(task.len(), |(b, _)| b);
            let rest = task[start..].trim();
            // "запиши в блокнот X" → X
            let rest = ["в блокнот ", "in notepad ", "into notepad "]
                .iter()
                .find_map(|p| rest.to_lowercase().starts_with(p).then(|| rest[p.len()..].trim()))
                .unwrap_or(rest);
            if !rest.is_empty() {
                return Some(rest.to_owned());
            }
        }
    }
    None
}

/// The plan script when `req` is a computer-assistant planner prompt.
#[must_use]
pub fn script(req: &ChatRequest) -> Option<MockScript> {
    if !req.system_prompt().is_some_and(|s| s.contains(MARKER)) {
        return None;
    }
    let prompt = req.messages.iter().find(|m| m.role == Role::User).map(|m| m.content.as_str()).unwrap_or("");
    let task = prompt.lines().find_map(|l| l.strip_prefix("Task: ")).unwrap_or("").trim().to_owned();
    let corrections: Vec<&str> = prompt.lines().filter_map(|l| l.strip_prefix("Correction: ")).collect();
    let lower = task.to_lowercase();
    let notey = ["блокнот", "notepad", "заметк", "список", "note", "list", "напиши", "запиши", "write"]
        .iter()
        .any(|k| lower.contains(k));
    let answer = if notey {
        let mut text = after_verb(&task).unwrap_or_else(|| task.clone());
        for c in &corrections {
            text.push_str(", ");
            text.push_str(c.trim());
        }
        json!({
            "explanation": format!("Mock (offline demo): open Notepad and type “{text}”."),
            "steps": [
                {"type": "Launch", "target": "notepad"},
                {"type": "Wait", "ms": 1500},
                {"type": "Keys", "combo": "ctrl+n"},
                {"type": "Wait", "ms": 600},
                {"type": "TypeText", "text": text}
            ]
        })
    } else {
        json!({
            "explanation": format!("Mock (offline demo): search the web for “{task}”."),
            "steps": [{"type": "WebSearch", "query": task}]
        })
    };
    Some(MockScript::Text(answer.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    fn req(user: &str) -> ChatRequest {
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user(user)]);
        r.system = Some(format!("You are {MARKER}."));
        r
    }

    fn text(s: Option<MockScript>) -> serde_json::Value {
        match s {
            Some(MockScript::Text(t)) => serde_json::from_str(&t).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn note_tasks_open_notepad_and_corrections_append() {
        let v = text(script(&req("Language: ru\nTask: запиши в блокнот список покупок\nCorrection: и яйца\n")));
        assert_eq!(v["steps"][0]["target"], "notepad");
        assert_eq!(v["steps"][4]["text"], "список покупок, и яйца");
        let v = text(script(&req("Task: what is the weather in Paris\n")));
        assert_eq!(v["steps"][0]["type"], "WebSearch");
    }

    #[test]
    fn other_prompts_are_ignored() {
        assert!(script(&ChatRequest::new("mock-smart", vec![ChatMessage::user("hi")])).is_none());
    }
}

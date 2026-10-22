//! Scripted Mock behaviour for the Bug Detective hypothesis (Phase 7,
//! ADR-0042). The detective's system prompt carries the marker
//! "Orkestr's Bug Detective"; the Mock answers with a schema-shaped JSON
//! hypothesis that cites the real evidence ids of the prompt (`### evN [Kind] …`)
//! and the files of the code snippets, so the offline demo / tests reach the
//! fix gate. Clearly a Mock answer ("Mock (offline demo)" in the summary).

use serde_json::json;

use super::mock::MockScript;
use crate::types::{ChatRequest, Role};

const MARKER: &str = "Orkestr's Bug Detective";

/// The hypothesis script when `req` is a Bug Detective prompt.
#[must_use]
pub fn script(req: &ChatRequest) -> Option<MockScript> {
    if !req.system_prompt().is_some_and(|s| s.contains(MARKER)) {
        return None;
    }
    let prompt = req.messages.iter().find(|m| m.role == Role::User).map(|m| m.content.as_str()).unwrap_or("");
    // Evidence headers: "### ev3 [CodeSnippet] src/calc.rs:2 (src/calc.rs:2)".
    let mut refs = Vec::new();
    let mut files = Vec::new();
    for line in prompt.lines().filter_map(|l| l.strip_prefix("### ")) {
        let mut parts = line.splitn(3, ' ');
        let (Some(id), Some(kind)) = (parts.next(), parts.next()) else { continue };
        if matches!(kind, "[StackTrace]" | "[CodeSnippet]" | "[Logs]" | "[Diff]") && refs.len() < 3 {
            refs.push(id.to_owned());
        }
        if kind == "[CodeSnippet]" {
            if let Some(r) = line.rsplit_once(" (").map(|(_, r)| r.trim_end_matches(')')) {
                files.push(r.to_owned());
            }
        }
    }
    if refs.is_empty() {
        if let Some(first) = prompt.lines().find_map(|l| l.strip_prefix("### ")).and_then(|l| l.split(' ').next()) {
            refs.push(first.to_owned());
        }
    }
    let file = files.first().cloned().unwrap_or_else(|| "unknown".into());
    let answer = json!({
        "summary": format!("Mock (offline demo): failure at {file}"),
        "rootCause": format!("The stack trace and the recent diff point at {file}; the latest commit introduced the failing code path."),
        "confidence": if files.is_empty() { 0.4 } else { 0.8 },
        "evidenceRefs": refs,
        "suspectedFiles": files,
        "proposedFix": format!("Restore the guard removed in the last commit around {file} and add a regression test."),
        "reproduction": null
    });
    Some(MockScript::Text(answer.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    fn req(prompt: &str) -> ChatRequest {
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user(prompt)]);
        r.system = Some(format!("You are {MARKER}. Reply with JSON."));
        r
    }

    #[test]
    fn cites_real_evidence_and_snippet_files() {
        let p = "Incident: x\n\nEvidence:\n\n### ev1 [Alert] check down\nbody\n\n### ev2 [StackTrace] trace\nat calc\n\n### ev3 [CodeSnippet] src/calc.js:2 (src/calc.js:2)\ncode\n";
        let Some(MockScript::Text(t)) = script(&req(p)) else { panic!("expected text") };
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["evidenceRefs"], json!(["ev2", "ev3"]));
        assert_eq!(v["suspectedFiles"], json!(["src/calc.js:2"]));
        assert!(v["summary"].as_str().unwrap().starts_with("Mock (offline demo)"));
    }

    #[test]
    fn falls_back_to_first_evidence_and_ignores_other_prompts() {
        let Some(MockScript::Text(t)) = script(&req("### ev1 [Alert] a\n")) else { panic!() };
        assert!(t.contains("\"ev1\""));
        let mut other = req("### ev1 [Alert] a\n");
        other.system = Some("You are a reviewer.".into());
        assert!(script(&other).is_none());
    }
}

//! Mock extension: deterministic handoff summaries (context optimizer §20,
//! §57). A request whose system prompt carries [`HANDOFF_SUMMARY_MARKER`] is
//! answered with a compact digest of the history in the last user message —
//! one line per entry, newest kept, labelled "Mock (offline demo)" and capped
//! by `max_output_tokens`.

use crate::adapters::mock::MockScript;
use crate::types::{ChatRequest, Role};

/// Marker in the system prompt of handoff-summarization requests.
pub const HANDOFF_SUMMARY_MARKER: &str = "[orkestr:handoff-summary]";

/// Max entry lines in a mock summary.
const MAX_LINES: usize = 12;

/// The scripted answer for a summarization request, if `req` is one.
#[must_use]
pub fn script(req: &ChatRequest) -> Option<MockScript> {
    let system = req.system_prompt().unwrap_or_default();
    if !system.contains(HANDOFF_SUMMARY_MARKER) {
        return None;
    }
    let text = req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default();
    let entries: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('['))
        .map(|l| l.chars().take(140).collect::<String>())
        .collect();
    let skip = entries.len().saturating_sub(MAX_LINES);
    let mut out = format!("Summary of {} earlier entries — Mock (offline demo):\n", entries.len());
    if skip > 0 {
        out.push_str(&format!("- … {skip} older entries condensed\n"));
    }
    for e in entries.iter().skip(skip) {
        out.push_str("- ");
        out.push_str(e);
        out.push('\n');
    }
    // ≈ 4 characters per token.
    let cap = req.max_output_tokens.map_or(usize::MAX, |t| (t as usize).saturating_mul(4).max(40));
    if out.chars().count() > cap {
        out = out.chars().take(cap.saturating_sub(1)).collect::<String>();
        out.push('…');
    }
    Some(MockScript::Text(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    #[test]
    fn summarizes_only_marked_requests() {
        let history = (0..20).map(|i| format!("[assistant] step {i} done")).collect::<Vec<_>>().join("\n");
        let mut req = ChatRequest::new("mock-smart", vec![ChatMessage::user(history)]);
        assert!(script(&req).is_none());
        req.system = Some(format!("Summarize. {HANDOFF_SUMMARY_MARKER}"));
        let Some(MockScript::Text(t)) = script(&req) else { panic!("summary expected") };
        assert!(t.contains("Mock (offline demo)"));
        assert!(t.contains("step 19 done") && !t.contains("step 3 done"));
        assert!(t.contains("8 older entries condensed"));
        req.max_output_tokens = Some(20);
        let Some(MockScript::Text(short)) = script(&req) else { panic!("summary expected") };
        assert!(short.chars().count() <= 80);
    }
}

//! AI planner for unknown commands: a prompt describing the action
//! vocabulary, the user's request (+ corrections, the previous plan and the
//! active window) → a JSON plan `{explanation, steps}` validated against the
//! same vocabulary. The model is behind [`PlanModel`] (core wires the
//! provider stack; tests use a closure / the Mock provider).

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use orkestr_protocol::methods::computer::{AssistantCommand, ComputerAction, LocalizedText, SafetyLevel};

use crate::catalogue::validate_command;
use crate::error::{ComputerError, ComputerResult};

/// Marker the Mock provider recognizes.
pub const PLANNER_MARKER: &str = "Orkestr's computer assistant planner";

/// A chat model that answers one prompt.
#[async_trait]
pub trait PlanModel: Send + Sync {
    /// Returns `(answer text, model label)`.
    async fn complete(
        &self,
        system: String,
        user: String,
        cancel: CancellationToken,
    ) -> ComputerResult<(String, String)>;
}

/// The system prompt.
#[must_use]
pub fn system_prompt() -> String {
    format!(
        "You are {PLANNER_MARKER}. The user controls a Windows PC by voice or text. Turn the request into a short \
plan of steps from this JSON action vocabulary ONLY (field `type` selects the action):
- Launch {{target}} — app name (\"notepad\", \"chrome\", \"telegram\"), exe, URL or file
- OpenUrl {{url}} · WebSearch {{query, engine?}} (engine: google, yandex, youtube, wikipedia, github, maps…)
- OpenSettings {{page}} (ms-settings page id, e.g. display, sound, bluetooth, network-wifi)
- OpenPath {{path}} (file / folder; known: downloads, documents, desktop, pictures, music, videos)
- FocusApp {{app}} · CloseApp {{app, discard}} · KillProcess {{name}}
- Window {{op, app?}} op: Focus, Minimize, Maximize, Restore, Close, SnapLeft, SnapRight, NextMonitor, PreviousMonitor, SwitchNext, ShowDesktop
- Keys {{combo, times?}} e.g. \"ctrl+s\", \"alt+f4\", \"win+d\", \"enter\"
- TypeText {{text}} · Scroll {{direction: Up|Down|Left|Right, amount?, unit?: Lines|Pages|Edge}}
- ClickElement {{name}} (UI Automation name of a button / link / menu item in the active window)
- MouseClick {{button?: Left|Right|Middle|Double}}
- Media {{key: PlayPause|Next|Previous|Stop}} · Volume {{op: Up|Down|Set|Mute, value?}} · Brightness {{op, value?}}
- System {{op: Lock|Sleep|Shutdown|Restart|SignOut}} · Screenshot {{}} · ClipboardSet {{text}} · ClipboardGet {{}}
- Notify {{title, body}} · Timer {{seconds, message}} · Wait {{ms}} · Speak {{text}}
- FileCreate {{path, content}} · FolderCreate {{path}} · FileMove {{from, to}} · FileDelete {{path}}
- If {{condition: {{type: WindowActive|WindowExists, app}} | {{type: ProcessRunning, name}}, then: [...], otherwise: [...]}}
Rules: prefer the fewest steps; add Wait {{ms: 1000-2000}} after Launch before typing into the new app; never \
invent apps the user did not ask for; destructive steps (FileDelete, KillProcess, CloseApp with discard, \
Shutdown) only when explicitly requested — the user confirms them separately. Answer with ONE JSON object and \
nothing else: {{\"explanation\": \"<one sentence in the user's language>\", \"steps\": [ ... ]}}. If the request \
cannot be done with these actions, answer {{\"explanation\": \"<why>\", \"steps\": []}}."
    )
}

/// The user prompt.
#[must_use]
pub fn user_prompt(
    request: &str,
    corrections: &[String],
    previous: Option<&[ComputerAction]>,
    active_window: Option<&str>,
    lang: &str,
) -> String {
    let mut s = format!("Language: {}\nTask: {}\n", if lang.starts_with("ru") { "ru" } else { "en" }, request.trim());
    if let Some(w) = active_window {
        s.push_str(&format!("Active window: {w}\n"));
    }
    if let Some(p) = previous {
        s.push_str(&format!("Previous plan: {}\n", serde_json::to_string(p).unwrap_or_default()));
    }
    for c in corrections {
        s.push_str(&format!("Correction: {}\n", c.trim()));
    }
    s
}

#[derive(Deserialize)]
struct RawPlan {
    #[serde(default)]
    explanation: String,
    #[serde(default)]
    steps: Vec<serde_json::Value>,
}

/// Extracts and validates the JSON plan from a model answer.
///
/// # Errors
/// `Planner` when no valid plan is found.
pub fn parse_plan(text: &str) -> ComputerResult<(Vec<ComputerAction>, String)> {
    let start = text.find('{').ok_or_else(|| ComputerError::Planner("the model did not return a JSON plan".into()))?;
    let end = text.rfind('}').ok_or_else(|| ComputerError::Planner("the model did not return a JSON plan".into()))?;
    let raw: RawPlan = serde_json::from_str(&text[start..=end])
        .map_err(|e| ComputerError::Planner(format!("the plan is not valid JSON: {e}")))?;
    let mut steps = Vec::new();
    for (i, v) in raw.steps.into_iter().enumerate() {
        let s: ComputerAction = serde_json::from_value(v)
            .map_err(|e| ComputerError::Planner(format!("step {} is not a known action: {e}", i + 1)))?;
        steps.push(s);
    }
    if steps.is_empty() {
        let why = if raw.explanation.is_empty() { "the model found no way to do this".into() } else { raw.explanation };
        return Err(ComputerError::Planner(why));
    }
    // reuse command validation (depth, size)
    let probe = AssistantCommand {
        id: "plan".into(),
        category: "ai".into(),
        title: LocalizedText::default(),
        phrases: vec!["plan".into()],
        steps: steps.clone(),
        safety: SafetyLevel::Safe,
        context: None,
    };
    validate_command(&probe).map_err(|e| ComputerError::Planner(e.to_string()))?;
    if !crate::catalogue::step_slots(&steps).is_empty() {
        return Err(ComputerError::Planner("the plan uses {slots}; plans must be concrete".into()));
    }
    Ok((steps, raw.explanation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let t = "Sure!\n```json\n{\"explanation\":\"Opens Notepad\",\"steps\":[{\"type\":\"Launch\",\"target\":\"notepad\"},{\"type\":\"Wait\",\"ms\":1500},{\"type\":\"TypeText\",\"text\":\"hi\"}]}\n```";
        let (steps, why) = parse_plan(t).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(why, "Opens Notepad");
    }

    #[test]
    fn rejects_bad_plans() {
        assert!(parse_plan("no idea").is_err());
        assert!(parse_plan("{\"steps\":[{\"type\":\"FormatDisk\"}]}").is_err());
        assert!(parse_plan("{\"explanation\":\"cannot\",\"steps\":[]}").unwrap_err().to_string().contains("cannot"));
        assert!(parse_plan("{\"steps\":[{\"type\":\"Launch\",\"target\":\"{app}\"}]}").is_err());
    }

    #[test]
    fn prompts_carry_the_marker_and_context() {
        assert!(system_prompt().contains(PLANNER_MARKER));
        let u =
            user_prompt("x", &["y".into()], Some(&[ComputerAction::Speak { text: "a".into() }]), Some("Notepad"), "ru");
        assert!(
            u.contains("Task: x")
                && u.contains("Correction: y")
                && u.contains("Previous plan")
                && u.contains("Language: ru")
        );
    }
}

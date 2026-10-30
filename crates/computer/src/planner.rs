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

/// The system prompt: the rules + the full action vocabulary generated from
/// the registry ([`crate::vocab`]) so the model knows everything Orkestr can
/// do on this PC (sound, windows, cursor, keyboard, displays, radios…).
#[must_use]
pub fn system_prompt() -> String {
    format!(
        "You are {PLANNER_MARKER}. The user controls their Windows PC by voice or text. You have NO tools of your own in this conversation and must ignore any tools of your environment: Orkestr executes the JSON steps below on the user's PC, and these actions give you FULL control of the computer — sound (system volume, per-app volume, mute, microphone, output / input device, media), windows, virtual desktops, cursor and mouse, keyboard, displays, screenshots, Wi-Fi / Bluetooth, power, files, the browser and system information. Never say that you cannot control something these actions cover.
The request may come from speech recognition and contain recognition errors: read it by sound and meaning («громко с 10%» = «громкость 10%», «звук на десять» = volume 10%, «на полную» = 100%).
Action vocabulary (field `type` selects the action; `?` = optional; `A|B` = allowed values):
{vocab}Rules: prefer the fewest steps; add Wait {{ms: 1000-2000}} after Launch before typing into the new app; never invent apps the user did not ask for; destructive steps (FileDelete, KillProcess, CloseApp with discard, EmptyRecycleBin, Radio, Shutdown) only when explicitly requested — the user confirms them separately; never type into password fields. Answer with ONE JSON object and nothing else: {{\"explanation\": \"<one sentence in the user's language>\", \"steps\": [ ... ]}}. Only when a request truly cannot be done with these actions, answer {{\"explanation\": \"<exactly which part is impossible and why>\", \"steps\": []}}.",
        vocab = crate::vocab::prompt_vocabulary()
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
    fn prompt_gives_the_full_vocabulary() {
        let p = system_prompt();
        // the «нет управления звуком» regression: sound is in the prompt with examples
        for must in [
            "\"type\":\"Volume\",\"op\":\"Set\",\"value\":10",
            "«громко с 10%»",
            "«сделай погромче»",
            "«звук на десять»",
            "AudioDevice",
            "Microphone",
            "MouseMove",
            "WindowOpacity",
            "NO tools of your own",
        ] {
            assert!(p.contains(must), "{must} missing from the planner prompt");
        }
        for s in crate::vocab::ACTIONS {
            assert!(p.contains(&format!("- {} ", s.name)), "{} missing", s.name);
        }
    }

    #[test]
    fn planned_volume_answers_parse() {
        for answer in [
            r#"{"explanation":"Громкость 10%","steps":[{"type":"Volume","op":"Set","value":10}]}"#,
            r#"{"explanation":"Погромче","steps":[{"type":"Volume","op":"Up","value":10}]}"#,
            r#"{"explanation":"Spotify тише","steps":[{"type":"Volume","op":"Down","value":20,"app":"spotify"}]}"#,
        ] {
            let (steps, _) = parse_plan(answer).unwrap();
            assert_eq!(steps[0].type_name(), "Volume");
        }
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

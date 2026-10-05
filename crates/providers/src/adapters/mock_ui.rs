//! Mock (offline demo) script for the AI Manager's `update_ui_config` tool
//! (§29 AI-editable UI): turns a plain-language UI request ("Сделай Mission
//! Control компактнее, Claude справа, убери график токенов, сделай фон
//! темнее") into a JSON Patch over the UI configuration schema, by keyword
//! rules. Real models produce the patch from the tool description; the core
//! validates either way.

use serde_json::{json, Value};

/// Words that mark a request as being about Orkestr's own UI.
const UI_CONTEXT: &[&str] = &[
    "интерфейс",
    "mission control",
    "миссион",
    "панел",
    " фон",
    " тема",
    " тему",
    " темы",
    " темой",
    "график",
    "виджет",
    "раскладк",
    "компактн",
    "просторн",
    "hud",
    " ui",
    "ui ",
    "interface",
    "panel",
    "background",
    "theme",
    "chart",
    "widget",
    "layout",
    "compact",
    "spacious",
];

/// Panel / lane names the mock recognises → id suffix.
const PANELS: &[(&[&str], &str)] = &[
    (&["claude"], "claude"),
    (&["gpt", "openai", "codex"], "openai"),
    (&["gemini"], "gemini"),
    (&["терминал", "terminal"], "terminal"),
    (&["логи", "logs"], "logs"),
    (&["агент", "agents"], "agents"),
    (&["задач", "tasks"], "tasks"),
];

/// Widget names → widget id suffix.
const WIDGETS: &[(&[&str], &str)] = &[
    (&["график токенов", "токен", "token chart", "tokens chart", "token usage"], "tokenChart"),
    (&["график расход", "расход", "cost chart", "spend chart"], "costChart"),
    (&["граф задач", "task graph"], "taskGraph"),
    (&["ресурс", "cpu", "resources"], "resources"),
];

const HIDE_WORDS: &[&str] = &["убери", "скрой", "спрячь", "удали", "hide", "remove", "get rid"];
const SHOW_WORDS: &[&str] = &["покажи", "верни", "show", "bring back"];

/// Human name of a panel / widget id suffix for the summary.
fn label(id: &str, ru: bool) -> &str {
    match (id, ru) {
        ("claude", _) => "Claude",
        ("openai", _) => "OpenAI",
        ("gemini", _) => "Gemini",
        ("terminal", true) => "терминал",
        ("logs", true) => "журнал",
        ("agents", true) => "агенты",
        ("tasks", true) => "задачи",
        ("tokenChart", true) => "график токенов",
        ("tokenChart", false) => "token chart",
        ("costChart", true) => "график расходов",
        ("costChart", false) => "cost chart",
        ("taskGraph", true) => "граф задач",
        ("taskGraph", false) => "task graph",
        ("resources", true) => "ресурсы",
        (other, _) => other,
    }
}

fn side_of(text: &str, at: usize) -> Option<&'static str> {
    // Look at the words right after the panel name.
    let tail: String = text[at..].chars().take(40).collect();
    let rules: [(&[&str], &str); 4] = [
        (&["справа", "вправо", "направо", "right"], "Right"),
        (&["слева", "влево", "налево", "left"], "Left"),
        (&["снизу", "вниз", "bottom"], "Bottom"),
        (&["в центр", "по центру", "center", "centre"], "Center"),
    ];
    rules
        .iter()
        .filter_map(|(words, side)| words.iter().filter_map(|w| tail.find(w)).min().map(|pos| (pos, *side)))
        .min_by_key(|(pos, _)| *pos)
        .map(|(_, side)| side)
}

/// The patch + a one-line summary for a UI request, or `None` when the text
/// is not a recognisable UI request. `ru` picks the summary language.
#[must_use]
pub fn ui_intent_patch(text: &str, ru: bool) -> Option<(String, Value)> {
    let lower = format!(" {} ", text.to_lowercase());
    if !UI_CONTEXT.iter().any(|w| lower.contains(w)) {
        return None;
    }
    let mission = lower.contains("mission control") || lower.contains("миссион");
    let scope = if mission { "missionControl." } else { "" };
    let mut ops: Vec<Value> = Vec::new();
    let mut parts_en: Vec<String> = Vec::new();
    let mut parts_ru: Vec<String> = Vec::new();

    if ["компактн", "компактнее", "плотнее", "compact", "denser", "tighter"].iter().any(|w| lower.contains(w))
    {
        ops.push(json!({"op": "replace", "path": "/prefs/density", "value": "Compact"}));
        parts_en.push("compact density".into());
        parts_ru.push("компактная плотность".into());
    } else if ["просторн", "свободнее", "spacious", "comfortable", "roomier"].iter().any(|w| lower.contains(w))
    {
        ops.push(json!({"op": "replace", "path": "/prefs/density", "value": "Comfortable"}));
        parts_en.push("comfortable density".into());
        parts_ru.push("просторная плотность".into());
    }

    let hide = HIDE_WORDS.iter().any(|w| lower.contains(w));
    let show = SHOW_WORDS.iter().any(|w| lower.contains(w));
    for (names, id) in WIDGETS {
        if !names.iter().any(|n| lower.contains(n)) {
            continue;
        }
        let widget = format!("{}{id}", if scope.is_empty() { "missionControl." } else { scope });
        if hide {
            ops.push(json!({"op": "add", "path": "/prefs/hiddenWidgets/-", "value": widget}));
            parts_en.push(format!("hide the {}", label(id, false)));
            parts_ru.push(format!("скрыть {}", label(id, true)));
        } else if show {
            parts_en.push(format!("show the {}", label(id, false)));
            parts_ru.push(format!("показать {}", label(id, true)));
            // Showing = removing from the hidden list; the mock cannot know
            // the index, so it resets the list (the preview makes it visible).
            ops.push(json!({"op": "replace", "path": "/prefs/hiddenWidgets", "value": []}));
        }
    }

    for (names, id) in PANELS {
        let Some(at) = names.iter().filter_map(|n| lower.find(n)).min() else { continue };
        if let Some(side) = side_of(&lower, at) {
            let panel = format!("{scope}{id}");
            ops.push(json!({"op": "add", "path": format!("/prefs/panels/{panel}"), "value": {"side": side, "hidden": false}}));
            parts_en.push(format!("{} on the {}", label(id, false), side.to_lowercase()));
            let side_ru = match side {
                "Right" => "справа",
                "Left" => "слева",
                "Bottom" => "снизу",
                _ => "в центре",
            };
            parts_ru.push(format!("{} {side_ru}", label(id, true)));
        }
    }

    let background = lower.contains(" фон") || lower.contains("background");
    if background && ["темнее", "тёмн", "темн", "darker", "dark"].iter().any(|w| lower.contains(w)) {
        ops.push(json!({"op": "replace", "path": "/prefs/backgroundDim", "value": 0.55}));
        ops.push(json!({"op": "add", "path": "/theme/tokens/color-bg", "value": "#020308"}));
        parts_en.push("darker background".into());
        parts_ru.push("более тёмный фон".into());
    } else if background && ["светлее", "lighter", "brighter"].iter().any(|w| lower.contains(w)) {
        ops.push(json!({"op": "replace", "path": "/prefs/backgroundDim", "value": 0.0}));
        parts_en.push("lighter background".into());
        parts_ru.push("более светлый фон".into());
    }

    if ops.is_empty() {
        return None;
    }
    let s = if ru { parts_ru.join(", ") } else { parts_en.join(", ") };
    let mut c = s.chars();
    let summary = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    Some((summary, Value::Array(ops)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mission_control_example() {
        let (summary, patch) = ui_intent_patch(
            "Сделай Mission Control компактнее, Claude справа, убери график токенов, сделай фон темнее",
            true,
        )
        .unwrap();
        let paths: Vec<&str> = patch.as_array().unwrap().iter().map(|o| o["path"].as_str().unwrap()).collect();
        assert_eq!(
            paths,
            vec![
                "/prefs/density",
                "/prefs/hiddenWidgets/-",
                "/prefs/panels/missionControl.claude",
                "/prefs/backgroundDim",
                "/theme/tokens/color-bg"
            ]
        );
        assert_eq!(patch[1]["value"], "missionControl.tokenChart");
        assert_eq!(patch[2]["value"]["side"], "Right");
        assert!(summary.contains("справа"), "{summary}");
    }

    #[test]
    fn english_and_non_ui_requests() {
        let (summary, patch) =
            ui_intent_patch("Make the UI compact and put the terminal at the bottom", false).unwrap();
        assert_eq!(patch.as_array().unwrap().len(), 2);
        assert!(summary.starts_with("Compact density"), "{summary}");
        assert!(ui_intent_patch("Create a hello module with tests", false).is_none());
        assert!(ui_intent_patch("Сделай модуль авторизации", true).is_none());
        // Substrings of unrelated words must not trigger (систем-а, теле-фон).
        assert!(ui_intent_patch("Почини систему оплаты по телефону", true).is_none());
    }
}

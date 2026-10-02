//! Reply language of the AI Manager and agents: they answer the user in the
//! UI language (`AppSettings.language`, resolved by the UI; `system` is
//! resolved there). The core keeps the current value in a shared
//! [`UiLanguage`] handle that both the Manager and the agent runtime read
//! when they build a system prompt.

use std::sync::Arc;

use orkestr_providers::UI_LANGUAGE_MARKER;
use parking_lot::RwLock;

/// Language used until the UI reports one.
pub const DEFAULT_UI_LANGUAGE: &str = "en";

/// Shared, live-updatable UI language (primary BCP-47 subtag, e.g. `ru`).
#[derive(Clone)]
pub struct UiLanguage(Arc<RwLock<String>>);

impl Default for UiLanguage {
    fn default() -> Self {
        Self::new(DEFAULT_UI_LANGUAGE)
    }
}

impl std::fmt::Debug for UiLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("UiLanguage").field(&*self.0.read()).finish()
    }
}

impl UiLanguage {
    /// A handle starting at `lang` (falls back to English when `lang` is not a language tag).
    #[must_use]
    pub fn new(lang: &str) -> Self {
        Self(Arc::new(RwLock::new(normalize(lang).unwrap_or_else(|| DEFAULT_UI_LANGUAGE.to_owned()))))
    }

    /// Current language.
    #[must_use]
    pub fn get(&self) -> String {
        self.0.read().clone()
    }

    /// Sets the language; `system` and invalid values are ignored (returns whether it changed).
    pub fn set(&self, lang: &str) -> bool {
        let Some(next) = normalize(lang) else { return false };
        let mut cur = self.0.write();
        if *cur == next {
            return false;
        }
        *cur = next;
        true
    }
}

/// Primary subtag of a BCP-47 tag (`ru-RU` → `ru`); `None` for `system` or garbage.
#[must_use]
pub fn normalize(tag: &str) -> Option<String> {
    let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
    ((2..=3).contains(&primary.len()) && primary.chars().all(|c| c.is_ascii_lowercase())).then_some(primary)
}

/// English name of a language (for the model), or the code itself.
#[must_use]
pub fn language_name(code: &str) -> &str {
    match code {
        "en" => "English",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        "pt" => "Portuguese",
        "it" => "Italian",
        "zh" => "Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        other => other,
    }
}

/// System-prompt section that makes the model answer in the UI language.
/// Starts with [`UI_LANGUAGE_MARKER`] (the Mock provider reads it).
#[must_use]
pub fn reply_language_instruction(lang: &str) -> String {
    let code = normalize(lang).unwrap_or_else(|| DEFAULT_UI_LANGUAGE.to_owned());
    let name = language_name(&code);
    format!(
        "{UI_LANGUAGE_MARKER} {code} ({name}).\n\
         - Write everything meant for the user (answers, summaries, questions, task titles, descriptions and acceptance criteria) in {name}. \
         If the user writes in another language, answer in that language.\n\
         - Keep code, identifiers, file paths, shell commands, tool arguments and git commit messages in English.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_ignores_system() {
        assert_eq!(normalize("ru-RU").as_deref(), Some("ru"));
        assert_eq!(normalize("EN").as_deref(), Some("en"));
        assert_eq!(normalize("system"), None);
        assert_eq!(normalize(""), None);
        let l = UiLanguage::default();
        assert_eq!(l.get(), "en");
        assert!(l.set("ru"));
        assert!(!l.set("system"));
        assert!(!l.set("ru"));
        assert_eq!(l.clone().get(), "ru");
    }

    #[test]
    fn instruction_names_the_language() {
        let s = reply_language_instruction("ru");
        assert!(s.starts_with(&format!("{UI_LANGUAGE_MARKER} ru (Russian)")));
        assert!(s.contains("in Russian"));
        // Must not trigger the Mock's planning heuristics.
        let lower = s.to_ascii_lowercase();
        assert!(!lower.contains("plan") && !lower.contains("json") && !lower.contains("decompose"));
    }
}

//! Spoken Jarvis phrases of cursor mode (ADR-0050). Simple local strings;
//! the Jarvis phrasing engine (wave 9 package B) can replace them by
//! installing a [`PhraseHook`] with `CursorService::set_phrase_hook`.

use std::sync::Arc;

use orkestr_protocol::methods::cursor::CursorAction;

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moment<'a> {
    /// Control taken.
    Start,
    /// An action is about to run (the model's own `say` wins when present).
    Act(&'a CursorAction),
    /// A risky step waits.
    Confirm,
    /// A question for the user.
    Ask(&'a str),
    /// Paused.
    Paused,
    /// Resumed.
    Resumed,
    /// Done (summary).
    Done(&'a str),
    /// Could not finish (summary).
    Impossible(&'a str),
    /// Aborted (reason code).
    Aborted(&'a str),
    /// Failed (no model / errors).
    Failed,
    /// Switched to another backend (label).
    Failover(&'a str),
}

/// Replaces the built-in phrase (`None` = keep the built-in one).
pub type PhraseHook = Arc<dyn Fn(&Moment<'_>, &str) -> Option<String> + Send + Sync>;

fn ru(lang: &str) -> bool {
    lang.starts_with("ru")
}

/// Built-in phrase (`None` = say nothing).
#[must_use]
pub fn phrase(m: &Moment<'_>, lang: &str) -> Option<String> {
    let r = ru(lang);
    let s = |ru_s: &str, en_s: &str| Some(if r { ru_s.to_owned() } else { en_s.to_owned() });
    match m {
        Moment::Start => s("Беру управление, сэр.", "Taking control, sir."),
        Moment::Act(a) => match a {
            CursorAction::Launch { app } => {
                Some(if r { format!("Открываю {app}…") } else { format!("Opening {app}…") })
            }
            CursorAction::FocusWindow { title } => Some(if r {
                format!("Переключаюсь на {title}…")
            } else {
                format!("Switching to {title}…")
            }),
            CursorAction::Type { .. } => s("Печатаю…", "Typing…"),
            _ => None,
        },
        Moment::Confirm => s("Требуется ваше подтверждение, сэр.", "I need your confirmation, sir."),
        Moment::Ask(q) => Some((*q).to_owned()),
        Moment::Paused => s("Пауза. Курсор ваш, сэр.", "Paused. The cursor is yours, sir."),
        Moment::Resumed => s("Продолжаю, сэр.", "Resuming, sir."),
        Moment::Done(summary) => {
            let tail = if r {
                "Готово, возвращаю управление."
            } else {
                "Done, handing control back."
            };
            Some(if summary.is_empty() { tail.to_owned() } else { format!("{summary} {tail}") })
        }
        Moment::Impossible(summary) => Some(if r {
            format!("Не получилось, сэр. {summary} Возвращаю управление.")
        } else {
            format!("I couldn't do it, sir. {summary} Handing control back.")
        }),
        Moment::Aborted(reason) => match *reason {
            "esc" | "mouse" | "stop" => s("Остановлено. Управление ваше, сэр.", "Stopped. Control is yours, sir."),
            "stepLimit" | "timeLimit" => {
                s("Лимит исчерпан, останавливаюсь. Управление ваше.", "Limit reached, stopping. Control is yours.")
            }
            _ => s("Останавливаюсь, сэр. Управление ваше.", "Stopping, sir. Control is yours."),
        },
        Moment::Failed => {
            s("Не удалось выполнить, сэр. Возвращаю управление.", "I couldn't complete it, sir. Handing control back.")
        }
        Moment::Failover(to) => {
            Some(if r { format!("Переключаюсь на {to}.") } else { format!("Switching to {to}.") })
        }
    }
}

/// Badge text of the overlay.
#[must_use]
pub fn badge(lang: &str, current: &str) -> String {
    let head = if ru(lang) {
        "ИИ управляет курсором — нажмите Esc или пошевелите мышью, чтобы остановить"
    } else {
        "AI is controlling the cursor — press Esc or move the mouse to stop"
    };
    if current.is_empty() {
        head.to_owned()
    } else {
        format!("{head}\n{}", current.chars().take(90).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_are_jarvis_style_in_both_languages() {
        assert_eq!(phrase(&Moment::Start, "ru").unwrap(), "Беру управление, сэр.");
        assert_eq!(phrase(&Moment::Done(""), "ru").unwrap(), "Готово, возвращаю управление.");
        assert!(phrase(&Moment::Done("Bluetooth включён."), "ru").unwrap().starts_with("Bluetooth включён."));
        assert_eq!(phrase(&Moment::Start, "en").unwrap(), "Taking control, sir.");
        let a = CursorAction::Launch { app: "настройки".into() };
        assert_eq!(phrase(&Moment::Act(&a), "ru").unwrap(), "Открываю настройки…");
        assert!(phrase(&Moment::Act(&CursorAction::Wait { ms: 1 }), "ru").is_none());
        assert!(badge("ru", "").contains("Esc"));
    }
}

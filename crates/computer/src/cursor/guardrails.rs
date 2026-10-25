//! Per-step safety rules of cursor mode (ADR-0050). Every decision is
//! checked before it runs:
//!
//! - **Refused**: input into / clicks on Orkestr's own windows, typing
//!   secrets (registered vault values, key / token patterns, card numbers).
//! - **Confirmation** (approvals inbox + panel, with a screenshot of the
//!   target): password fields, purchases / payments, deleting, sending
//!   messages / mail / posts, discarding unsaved work, changing system
//!   settings, and anything the model itself flagged as risky.

use orkestr_protocol::methods::cursor::{CursorAction, CursorElement};

use crate::safety::title_looks_unsaved;

/// The verdict for one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run it.
    Allow,
    /// Ask the user first.
    Confirm {
        /// `password`, `purchase`, `delete`, `send`, `closeUnsaved`, `system`, `modelFlagged`.
        code: String,
        /// What (element / window).
        detail: String,
    },
    /// Never.
    Refuse {
        /// `ownWindow`, `secret`.
        code: String,
        /// Why.
        detail: String,
    },
}

/// What the rules look at.
#[derive(Debug, Clone, Default)]
pub struct StepContext<'a> {
    /// Foreground window title.
    pub window_title: &'a str,
    /// Foreground process (`notepad.exe`).
    pub window_process: &'a str,
    /// The foreground window is Orkestr's.
    pub foreground_is_own: bool,
    /// The point the action targets is inside an Orkestr window.
    pub target_is_own: bool,
    /// The element the action targets (by id, or the one under the point).
    pub element: Option<&'a CursorElement>,
    /// The focused control is a password field.
    pub focused_password: bool,
    /// Risk declared by the model.
    pub model_risk: Option<&'a str>,
}

const PURCHASE: &[&str] = &[
    "купить", "оплатить", "оплата", "оформить заказ", "подтвердить заказ", "заказать", "в корзину", "pay", "buy", "checkout",
    "place order", "purchase", "subscribe", "подписаться за", "donate",
];
const DELETE: &[&str] = &[
    "удалить", "удаление", "стереть", "очистить", "delete", "remove", "erase", "uninstall", "wipe", "empty recycle",
];
const SEND: &[&str] = &["отправить", "опубликовать", "переслать", "send", "post", "publish", "tweet", "reply all"];
const DISCARD: &[&str] = &["не сохранять", "don't save", "dont save", "discard", "не сохранить", "без сохранения"];
const SYSTEM_PROCS: &[&str] = &["systemsettings", "control", "regedit", "mmc", "services", "gpedit", "msconfig", "devmgmt", "taskmgr"];
const MESSENGERS: &[&str] = &["telegram", "whatsapp", "outlook", "thunderbird", "slack", "discord", "teams", "viber", "skype", "signal"];

fn norm(s: &str) -> String {
    s.to_lowercase().replace('ё', "е").replace('&', "")
}

fn has_word(hay: &str, words: &[&str]) -> Option<String> {
    let h = norm(hay);
    words.iter().find(|w| h.contains(*w)).map(|w| (*w).to_owned())
}

fn proc_key(p: &str) -> String {
    norm(p).trim_end_matches(".exe").to_owned()
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| if i % 2 == 1 { if d * 2 > 9 { d * 2 - 9 } else { d * 2 } } else { d })
        .sum();
    sum % 10 == 0
}

/// True when the text looks like a secret (never typed by the agent).
#[must_use]
pub fn looks_like_secret(text: &str) -> bool {
    if orkestr_security::redact::redact(text) != text {
        return true;
    }
    // card numbers: 13–19 digits (spaces / dashes allowed) passing Luhn
    let digits: Vec<u32> = text.chars().filter_map(|c| c.to_digit(10)).collect();
    let only_card_chars = text.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '-');
    if only_card_chars && (13..=19).contains(&digits.len()) && luhn(&digits) {
        return true;
    }
    // long unbroken high-entropy tokens (keys, JWTs)
    text.split_whitespace().any(|w| {
        let n = w.chars().count();
        let classes = [
            w.chars().any(|c| c.is_ascii_lowercase()),
            w.chars().any(|c| c.is_ascii_uppercase()),
            w.chars().any(|c| c.is_ascii_digit()),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        n >= 32 && classes >= 3 && !w.contains("://") && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.=+/".contains(c))
    })
}

fn confirm(code: &str, detail: impl Into<String>) -> Verdict {
    Verdict::Confirm { code: code.into(), detail: detail.into() }
}

/// Checks one action.
#[must_use]
pub fn check(action: &CursorAction, cx: &StepContext<'_>) -> Verdict {
    let el_name = cx.element.map(|e| e.name.as_str()).unwrap_or_default();
    let el_role = cx.element.map(|e| e.role.as_str()).unwrap_or_default();
    let in_system = SYSTEM_PROCS.contains(&proc_key(cx.window_process).as_str());
    match action {
        CursorAction::Done { .. } | CursorAction::AskUser { .. } | CursorAction::Wait { .. } => return Verdict::Allow,
        CursorAction::FocusWindow { title } => {
            if norm(title).contains("orkestr") {
                return Verdict::Refuse { code: "ownWindow".into(), detail: title.clone() };
            }
            return Verdict::Allow;
        }
        CursorAction::Launch { app } => {
            if norm(app).contains("orkestr") {
                return Verdict::Refuse { code: "ownWindow".into(), detail: app.clone() };
            }
            return cx.model_risk.map_or(Verdict::Allow, |r| confirm(r, app.clone()));
        }
        _ => {}
    }
    // never act on Orkestr itself
    let pointer = action.target().is_some();
    if (pointer && cx.target_is_own) || (!pointer && cx.foreground_is_own) {
        return Verdict::Refuse { code: "ownWindow".into(), detail: "Orkestr".into() };
    }
    match action {
        CursorAction::Type { text } => {
            if looks_like_secret(text) {
                return Verdict::Refuse { code: "secret".into(), detail: "secret-like text".into() };
            }
            if cx.focused_password || cx.element.is_some_and(|e| e.password) {
                return confirm("password", el_name);
            }
        }
        CursorAction::Keys { keys } => {
            let k: Vec<&str> = keys.iter().map(String::as_str).collect();
            let has = |x: &str| k.contains(&x);
            if has("alt") && has("f4") || has("ctrl") && has("w") {
                if title_looks_unsaved(cx.window_title) {
                    return confirm("closeUnsaved", cx.window_title);
                }
            } else if has("delete") || has("shift") && has("delete") {
                if proc_key(cx.window_process) == "explorer" {
                    return confirm("delete", cx.window_title);
                }
            } else if has("enter") && MESSENGERS.contains(&proc_key(cx.window_process).as_str()) {
                return confirm("send", cx.window_title);
            }
        }
        _ => {}
    }
    if matches!(action, CursorAction::Click { .. }) && !el_name.is_empty() {
        if has_word(el_name, DISCARD).is_some() {
            return confirm("closeUnsaved", el_name);
        }
        if has_word(el_name, PURCHASE).is_some() {
            return confirm("purchase", el_name);
        }
        if has_word(el_name, SEND).is_some() {
            return confirm("send", el_name);
        }
        if has_word(el_name, DELETE).is_some() {
            return confirm("delete", el_name);
        }
        let toggles = ["checkbox", "radiobutton", "button", "toggle", "switch", "combobox", "slider"];
        let chrome = ["назад", "back", "поиск", "search", "свернуть", "развернуть", "закрыть", "minimize", "maximize", "close"];
        if in_system && toggles.contains(&el_role) && has_word(el_name, &chrome).is_none() {
            return confirm("system", el_name);
        }
    }
    if let Some(r) = cx.model_risk {
        let code = if ["purchase", "delete", "send", "password", "system", "closeUnsaved"].contains(&r) { r } else { "modelFlagged" };
        return confirm(code, if el_name.is_empty() { cx.window_title } else { el_name });
    }
    Verdict::Allow
}

#[cfg(test)]
mod tests {
    use orkestr_protocol::methods::computer::MouseButton;
    use orkestr_protocol::methods::cursor::CursorTarget;

    use super::*;

    fn el(role: &str, name: &str) -> CursorElement {
        CursorElement { id: 1, role: role.into(), name: name.into(), rect: [0, 0, 10, 10], enabled: true, password: false, value: None }
    }
    fn click() -> CursorAction {
        CursorAction::Click { target: CursorTarget::element(1), button: MouseButton::Left, double: false }
    }
    fn code(v: &Verdict) -> &str {
        match v {
            Verdict::Allow => "allow",
            Verdict::Confirm { code, .. } | Verdict::Refuse { code, .. } => code,
        }
    }

    #[test]
    fn dangerous_clicks_need_confirmation() {
        for (name, want) in [
            ("Оформить заказ", "purchase"),
            ("Buy now", "purchase"),
            ("Удалить", "delete"),
            ("Отправить", "send"),
            ("Don't save", "closeUnsaved"),
            ("Не сохранять", "closeUnsaved"),
            ("OK", "allow"),
        ] {
            let e = el("button", name);
            let cx = StepContext { element: Some(&e), window_process: "chrome.exe", ..StepContext::default() };
            assert_eq!(code(&check(&click(), &cx)), want, "{name}");
        }
    }

    #[test]
    fn system_settings_toggles_need_confirmation_but_navigation_does_not() {
        let t = el("button", "Bluetooth");
        let cx = StepContext { element: Some(&t), window_process: "SystemSettings.exe", ..StepContext::default() };
        assert_eq!(code(&check(&click(), &cx)), "system");
        let nav = el("listitem", "Bluetooth и устройства");
        let cx = StepContext { element: Some(&nav), window_process: "SystemSettings.exe", ..StepContext::default() };
        assert_eq!(code(&check(&click(), &cx)), "allow");
    }

    #[test]
    fn own_windows_and_secrets_are_refused() {
        let cx = StepContext { target_is_own: true, ..StepContext::default() };
        assert_eq!(code(&check(&click(), &cx)), "ownWindow");
        let cx = StepContext { foreground_is_own: true, ..StepContext::default() };
        assert_eq!(code(&check(&CursorAction::Type { text: "hi".into() }, &cx)), "ownWindow");
        assert_eq!(code(&check(&CursorAction::FocusWindow { title: "Orkestr".into() }, &cx)), "ownWindow");
        let cx = StepContext::default();
        for s in ["4111 1111 1111 1111", "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789", "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab"] {
            assert_eq!(code(&check(&CursorAction::Type { text: s.into() }, &cx)), "secret", "{s}");
        }
        assert_eq!(code(&check(&CursorAction::Type { text: "Привет из Orkestr".into() }, &cx)), "allow");
        assert_eq!(code(&check(&CursorAction::Type { text: "https://www.google.com/search?q=tickets".into() }, &cx)), "allow");
    }

    #[test]
    fn password_fields_and_model_flags_need_confirmation() {
        let cx = StepContext { focused_password: true, ..StepContext::default() };
        assert_eq!(code(&check(&CursorAction::Type { text: "x".into() }, &cx)), "password");
        let cx = StepContext { model_risk: Some("send"), ..StepContext::default() };
        assert_eq!(code(&check(&CursorAction::Keys { keys: vec!["enter".into()] }, &cx)), "send");
        let cx = StepContext { model_risk: Some("weird"), ..StepContext::default() };
        assert_eq!(code(&check(&CursorAction::Keys { keys: vec!["enter".into()] }, &cx)), "modelFlagged");
    }

    #[test]
    fn closing_unsaved_and_sending_by_keys() {
        let keys = |k: &[&str]| CursorAction::Keys { keys: k.iter().map(|s| (*s).to_owned()).collect() };
        let cx = StepContext { window_title: "*Без имени — Блокнот", ..StepContext::default() };
        assert_eq!(code(&check(&keys(&["alt", "f4"]), &cx)), "closeUnsaved");
        let cx = StepContext { window_title: "Без имени — Блокнот", ..StepContext::default() };
        assert_eq!(code(&check(&keys(&["alt", "f4"]), &cx)), "allow");
        let cx = StepContext { window_process: "Telegram.exe", ..StepContext::default() };
        assert_eq!(code(&check(&keys(&["enter"]), &cx)), "send");
        let cx = StepContext { window_process: "explorer.exe", ..StepContext::default() };
        assert_eq!(code(&check(&keys(&["shift", "delete"]), &cx)), "delete");
    }
}

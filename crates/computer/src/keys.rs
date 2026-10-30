//! Key combinations: `ctrl+shift+t`, `alt f4`, `контрол шифт т`, `win+d`.
//! Parsed into canonical key names; the Windows backend maps them to
//! virtual-key codes.

use crate::error::{ComputerError, ComputerResult};

/// Canonical key names the backends understand.
pub const KEY_NAMES: &[&str] = &[
    "ctrl",
    "shift",
    "alt",
    "win",
    "enter",
    "esc",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
    "printscreen",
    "capslock",
    "numlock",
    "scrolllock",
    "pause",
    "menu",
    "volumeup",
    "volumedown",
    "volumemute",
    "playpause",
    "nexttrack",
    "prevtrack",
    "mediastop",
    "browserback",
    "browserforward",
    "browserrefresh",
    "browserhome",
    "plus",
    "minus",
    "equals",
    "comma",
    "period",
    "slash",
    "backslash",
    "semicolon",
    "quote",
    "bracketleft",
    "bracketright",
    "backquote",
];

const ALIASES: &[(&str, &str)] = &[
    ("control", "ctrl"),
    ("контрол", "ctrl"),
    ("контрл", "ctrl"),
    ("контроль", "ctrl"),
    ("ктрл", "ctrl"),
    ("шифт", "shift"),
    ("альт", "alt"),
    ("windows", "win"),
    ("вин", "win"),
    ("виндоус", "win"),
    ("windows key", "win"),
    ("meta", "win"),
    ("super", "win"),
    ("return", "enter"),
    ("ввод", "enter"),
    ("энтер", "enter"),
    ("интер", "enter"),
    ("escape", "esc"),
    ("эскейп", "esc"),
    ("эскейь", "esc"),
    ("таб", "tab"),
    ("табуляция", "tab"),
    ("пробел", "space"),
    ("spacebar", "space"),
    ("бэкспейс", "backspace"),
    ("бекспейс", "backspace"),
    ("del", "delete"),
    ("делит", "delete"),
    ("удаление", "delete"),
    ("ins", "insert"),
    ("инсерт", "insert"),
    ("хоум", "home"),
    ("энд", "end"),
    ("pgup", "pageup"),
    ("page_up", "pageup"),
    ("пейджап", "pageup"),
    ("pgdn", "pagedown"),
    ("page_down", "pagedown"),
    ("пейдждаун", "pagedown"),
    ("arrowup", "up"),
    ("arrowdown", "down"),
    ("arrowleft", "left"),
    ("arrowright", "right"),
    ("вверх", "up"),
    ("вниз", "down"),
    ("влево", "left"),
    ("вправо", "right"),
    ("prtsc", "printscreen"),
    ("prtscr", "printscreen"),
    ("print", "printscreen"),
    ("принтскрин", "printscreen"),
    ("apps", "menu"),
    ("contextmenu", "menu"),
    ("volume_up", "volumeup"),
    ("volume_down", "volumedown"),
    ("mute", "volumemute"),
    ("play", "playpause"),
    ("next", "nexttrack"),
    ("prev", "prevtrack"),
    ("previous", "prevtrack"),
    ("+", "plus"),
    ("-", "minus"),
    ("=", "equals"),
    (",", "comma"),
    (".", "period"),
    ("/", "slash"),
    ("\\", "backslash"),
    (";", "semicolon"),
    ("'", "quote"),
    ("[", "bracketleft"),
    ("]", "bracketright"),
    ("`", "backquote"),
];

/// Spoken names of Latin letters (as Russian speech recognition writes them):
/// «контрл шифт эс» → ctrl+shift+s.
const LETTER_NAMES: &[(&str, &str)] = &[
    ("эй", "a"),
    ("би", "b"),
    ("бэ", "b"),
    ("си", "c"),
    ("цэ", "c"),
    ("ди", "d"),
    ("дэ", "d"),
    ("эф", "f"),
    ("джи", "g"),
    ("гэ", "g"),
    ("эйч", "h"),
    ("аш", "h"),
    ("ай", "i"),
    ("джей", "j"),
    ("кей", "k"),
    ("ка", "k"),
    ("эл", "l"),
    ("эль", "l"),
    ("эм", "m"),
    ("эн", "n"),
    ("оу", "o"),
    ("пи", "p"),
    ("пэ", "p"),
    ("кью", "q"),
    ("ар", "r"),
    ("эр", "r"),
    ("эс", "s"),
    ("ти", "t"),
    ("тэ", "t"),
    ("ю", "u"),
    ("ви", "v"),
    ("вэ", "v"),
    ("дабл", "w"),
    ("икс", "x"),
    ("игрек", "y"),
    ("уай", "y"),
    ("зед", "z"),
    ("зет", "z"),
    ("зэт", "z"),
    ("контрал", "ctrl"),
    ("контр", "ctrl"),
    ("кнтрл", "ctrl"),
    ("шыфт", "shift"),
    ("шифтом", "shift"),
    ("альтом", "alt"),
    ("эскейп", "esc"),
    ("эскейб", "esc"),
    ("ентер", "enter"),
    ("капслок", "capslock"),
    ("капс", "capslock"),
    ("плюс", "plus"),
    ("минус", "minus"),
];

const RU_LAYOUT: &str = "йцукенгшщзфывапролдячсмить";
const EN_LAYOUT: &str = "qwertyuiopasdfghjklzxcvbnm";

fn canonical(part: &str) -> Option<String> {
    let p = part.trim().to_lowercase();
    if p.is_empty() {
        return None;
    }
    if KEY_NAMES.contains(&p.as_str()) {
        return Some(p);
    }
    if let Some((_, k)) = ALIASES.iter().find(|(a, _)| *a == p) {
        return Some((*k).to_owned());
    }
    if let Some((_, k)) = LETTER_NAMES.iter().find(|(a, _)| *a == p) {
        return Some((*k).to_owned());
    }
    let chars: Vec<char> = p.chars().collect();
    if chars.len() == 1 {
        let c = chars[0];
        if c.is_ascii_alphanumeric() {
            return Some(c.to_string());
        }
        if let Some(i) = RU_LAYOUT.chars().position(|r| r == c) {
            return EN_LAYOUT.chars().nth(i).map(|e| e.to_string());
        }
    }
    // f1..f24 / ф1..ф24
    let rest = p.strip_prefix('f').or_else(|| p.strip_prefix('ф'));
    if let Some(n) = rest.and_then(|r| r.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Some(format!("f{n}"));
        }
    }
    None
}

/// Parses a combination into canonical key names (modifiers first, as written).
///
/// # Errors
/// `Invalid` for unknown keys or an empty combination.
pub fn parse_combo(combo: &str) -> ComputerResult<Vec<String>> {
    let trimmed = combo.trim();
    if trimmed.is_empty() {
        return Err(ComputerError::Invalid("empty key combination".into()));
    }
    // A lone "+" / "-" is the key itself.
    if let Some(k) = canonical(trimmed) {
        return Ok(vec![k]);
    }
    let mut keys = Vec::new();
    for part in trimmed.split(['+', ' ']).filter(|p| !p.trim().is_empty()) {
        let k = canonical(part).ok_or_else(|| ComputerError::Invalid(format!("unknown key `{part}` in `{combo}`")))?;
        keys.push(k);
    }
    if keys.is_empty() {
        return Err(ComputerError::Invalid(format!("no keys in `{combo}`")));
    }
    if keys.len() > 5 {
        return Err(ComputerError::Invalid(format!("too many keys in `{combo}`")));
    }
    Ok(keys)
}

/// True for modifier keys.
#[must_use]
pub fn is_modifier(key: &str) -> bool {
    matches!(key, "ctrl" | "shift" | "alt" | "win")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combos() {
        assert_eq!(parse_combo("ctrl+shift+t").unwrap(), ["ctrl", "shift", "t"]);
        assert_eq!(parse_combo("Alt+F4").unwrap(), ["alt", "f4"]);
        assert_eq!(parse_combo("контрол шифт т").unwrap(), ["ctrl", "shift", "n"]);
        assert_eq!(parse_combo("ctrl+с").unwrap(), ["ctrl", "c"]);
        assert_eq!(parse_combo("win+d").unwrap(), ["win", "d"]);
        assert_eq!(parse_combo("ctrl+plus").unwrap(), ["ctrl", "plus"]);
        assert_eq!(parse_combo("+").unwrap(), ["plus"]);
        assert_eq!(parse_combo("пробел").unwrap(), ["space"]);
        assert_eq!(parse_combo("ф5").unwrap(), ["f5"]);
        assert_eq!(parse_combo("контрл шифт эс").unwrap(), ["ctrl", "shift", "s"]);
        assert_eq!(parse_combo("контрол эф").unwrap(), ["ctrl", "f"]);
        assert_eq!(parse_combo("альт таб").unwrap(), ["alt", "tab"]);
        assert_eq!(parse_combo("вин икс").unwrap(), ["win", "x"]);
        assert!(parse_combo("ctrl+nothing").is_err());
        assert!(parse_combo("").is_err());
    }

    #[test]
    fn every_alias_targets_a_known_key() {
        for (a, k) in ALIASES {
            assert!(KEY_NAMES.contains(k), "{a} → {k}");
        }
    }
}

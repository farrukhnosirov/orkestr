//! VS Code color themes: JSONC theme files with `include` chains, token
//! colors inline or in a referenced `.tmTheme` (plist) / JSON file, and
//! classic TextMate `.tmTheme` themes. The result is plain data the editor
//! maps to a Monaco theme.

use std::collections::BTreeMap;
use std::path::Path;

use orkestr_protocol::methods::extensions::TokenColorRule;
use serde_json::Value;

use crate::error::{ExtError, ExtResult};
use crate::paths;

/// Max `include` depth.
const MAX_DEPTH: usize = 8;
/// Max size of one theme file.
const MAX_FILE: u64 = 8 << 20;

/// A resolved theme body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeBody {
    /// Workbench colors.
    pub colors: BTreeMap<String, String>,
    /// Token colors (base rules first).
    pub token_colors: Vec<TokenColorRule>,
    /// `semanticHighlighting`.
    pub semantic_highlighting: bool,
}

/// Normalizes a CSS hex color to lowercase `#rrggbb[aa]`; `None` otherwise.
#[must_use]
pub fn normalize_color(c: &str) -> Option<String> {
    let c = c.trim();
    let hex = c.strip_prefix('#')?;
    if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let lower = hex.to_ascii_lowercase();
    match lower.len() {
        3 | 4 => Some(format!("#{}", lower.chars().flat_map(|ch| [ch, ch]).collect::<String>())),
        6 | 8 => Some(format!("#{lower}")),
        _ => None,
    }
}

fn read_file(root: &Path, rel: &str) -> ExtResult<String> {
    let p = paths::resolve_existing(root, rel)?;
    crate::archive::read_text_capped(&p, MAX_FILE)
}

fn is_plist(rel: &str, text: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".tmtheme") || lower.ends_with(".plist") || text.trim_start().starts_with("<?xml")
}

/// Parses a plist document into JSON.
pub fn plist_to_json(text: &str) -> ExtResult<Value> {
    let v: plist::Value = plist::from_bytes(text.as_bytes())
        .map_err(|e| ExtError::Invalid(format!("invalid plist: {e}")))?;
    serde_json::to_value(v).map_err(|e| ExtError::Invalid(format!("invalid plist: {e}")))
}

fn scopes(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn rule_of(entry: &Value) -> Option<TokenColorRule> {
    let settings = entry.get("settings")?;
    let foreground = settings.get("foreground").and_then(Value::as_str).and_then(normalize_color);
    let background = settings.get("background").and_then(Value::as_str).and_then(normalize_color);
    let font_style = settings.get("fontStyle").and_then(Value::as_str).map(|s| s.trim().to_owned());
    if foreground.is_none() && background.is_none() && font_style.is_none() {
        return None;
    }
    Some(TokenColorRule { scope: scopes(entry.get("scope")), foreground, background, font_style })
}

/// Global settings of a `.tmTheme` (`settings[0]` without a scope) → workbench colors.
fn tm_globals(entry: &Value, colors: &mut BTreeMap<String, String>) {
    let Some(s) = entry.get("settings").and_then(Value::as_object) else { return };
    let map: &[(&str, &str)] = &[
        ("background", "editor.background"),
        ("foreground", "editor.foreground"),
        ("caret", "editorCursor.foreground"),
        ("selection", "editor.selectionBackground"),
        ("lineHighlight", "editor.lineHighlightBackground"),
        ("invisibles", "editorWhitespace.foreground"),
        ("findHighlight", "editor.findMatchHighlightBackground"),
        ("guide", "editorIndentGuide.background1"),
        ("activeGuide", "editorIndentGuide.activeBackground1"),
    ];
    for (tm, vs) in map {
        if let Some(c) = s.get(*tm).and_then(Value::as_str).and_then(normalize_color) {
            colors.entry((*vs).to_owned()).or_insert(c);
        }
    }
}

fn token_colors_from_array(arr: &[Value], body: &mut ThemeBody) {
    for entry in arr {
        if entry.get("scope").is_none() && entry.get("name").is_none() {
            // `.tmTheme`-style global settings inside tokenColors.
            tm_globals(entry, &mut body.colors);
            if let Some(r) = rule_of(entry) {
                // An unscoped rule sets the default foreground in VS Code.
                if r.foreground.is_some() || r.font_style.is_some() {
                    body.token_colors.push(TokenColorRule { scope: Vec::new(), background: None, ..r });
                }
            }
            continue;
        }
        if let Some(r) = rule_of(entry) {
            body.token_colors.push(r);
        }
    }
}

fn load_token_file(root: &Path, rel: &str, depth: usize, body: &mut ThemeBody) -> ExtResult<()> {
    if depth > MAX_DEPTH {
        return Err(ExtError::Invalid("theme include chain is too deep".into()));
    }
    let text = read_file(root, rel)?;
    let v = if is_plist(rel, &text) { plist_to_json(&text)? } else { crate::jsonc::parse(&text)? };
    if let Some(arr) = v.get("settings").and_then(Value::as_array) {
        token_colors_from_array(arr, body);
    } else if let Some(arr) = v.get("tokenColors").and_then(Value::as_array) {
        token_colors_from_array(arr, body);
    } else if let Some(arr) = v.as_array() {
        token_colors_from_array(arr, body);
    }
    Ok(())
}

fn load(root: &Path, rel: &str, depth: usize, stack: &mut Vec<String>, body: &mut ThemeBody) -> ExtResult<()> {
    if depth > MAX_DEPTH {
        return Err(ExtError::Invalid("theme include chain is too deep".into()));
    }
    if stack.iter().any(|s| s == rel) {
        return Err(ExtError::Invalid(format!("theme include cycle at `{rel}`")));
    }
    stack.push(rel.to_owned());
    let text = read_file(root, rel)?;
    if is_plist(rel, &text) {
        let v = plist_to_json(&text)?;
        if let Some(arr) = v.get("settings").and_then(Value::as_array) {
            token_colors_from_array(arr, body);
        }
        stack.pop();
        return Ok(());
    }
    let v = crate::jsonc::parse(&text)?;
    let dir = paths::parent(rel).to_owned();
    // Base first: included theme rules come before this file's rules.
    if let Some(inc) = v.get("include").and_then(Value::as_str) {
        let inc_rel = paths::join_rel(&dir, inc)?;
        load(root, &inc_rel, depth + 1, stack, body)?;
    }
    if let Some(colors) = v.get("colors").and_then(Value::as_object) {
        for (k, c) in colors {
            match c.as_str().and_then(normalize_color) {
                Some(c) => {
                    body.colors.insert(k.clone(), c);
                }
                None if c.is_null() => {
                    body.colors.remove(k);
                }
                None => {}
            }
        }
    }
    match v.get("tokenColors") {
        Some(Value::Array(arr)) => token_colors_from_array(arr, body),
        Some(Value::String(p)) => {
            let tok_rel = paths::join_rel(&dir, p)?;
            load_token_file(root, &tok_rel, depth + 1, body)?;
        }
        _ => {}
    }
    if let Some(b) = v.get("semanticHighlighting").and_then(Value::as_bool) {
        body.semantic_highlighting = b;
    }
    stack.pop();
    Ok(())
}

/// Resolves the theme file `rel` (extension-relative) of the extension at `root`.
pub fn resolve(root: &Path, rel: &str) -> ExtResult<ThemeBody> {
    let mut body = ThemeBody::default();
    let rel = paths::normalize(rel)?;
    load(root, &rel, 0, &mut Vec::new(), &mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn colors() {
        assert_eq!(normalize_color("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(normalize_color("#abcd").as_deref(), Some("#aabbccdd"));
        assert_eq!(normalize_color("#112233").as_deref(), Some("#112233"));
        assert_eq!(normalize_color("#11223344").as_deref(), Some("#11223344"));
        assert_eq!(normalize_color("red"), None);
        assert_eq!(normalize_color("#12345"), None);
        assert_eq!(normalize_color("#gggggg"), None);
    }

    #[test]
    fn include_chain_and_jsonc() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "themes/base.json",
            r##"{ // base
                "colors": { "editor.background": "#000000", "editor.foreground": "#ffffff" },
                "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#888", "fontStyle": "italic" } } ],
            }"##,
        );
        write(
            dir.path(),
            "themes/dark.json",
            r##"{
                "include": "./base.json",
                "colors": { "editor.background": "#101010", "bad": "rgb(1,2,3)" },
                "tokenColors": [
                    { "scope": "keyword, storage.type", "settings": { "foreground": "#FF79C6" } },
                    { "scope": ["string"], "settings": { "foreground": "#f1fa8c" } },
                    { "settings": { "foreground": "#f8f8f2" } }
                ],
                "semanticHighlighting": true
            }"##,
        );
        let t = resolve(dir.path(), "./themes/dark.json").unwrap();
        assert_eq!(t.colors["editor.background"], "#101010");
        assert_eq!(t.colors["editor.foreground"], "#ffffff");
        assert!(!t.colors.contains_key("bad"));
        assert_eq!(t.token_colors[0].scope, vec!["comment".to_string()]);
        assert_eq!(t.token_colors[0].foreground.as_deref(), Some("#888888"));
        assert_eq!(t.token_colors[1].scope, vec!["keyword".to_string(), "storage.type".to_string()]);
        assert_eq!(t.token_colors[1].foreground.as_deref(), Some("#ff79c6"));
        assert!(t.token_colors.iter().any(|r| r.scope.is_empty() && r.foreground.as_deref() == Some("#f8f8f2")));
        assert!(t.semantic_highlighting);
    }

    #[test]
    fn token_colors_from_tmtheme_file() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "t/x.tmTheme",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>settings</key><array>
<dict><key>settings</key><dict><key>background</key><string>#272822</string><key>foreground</key><string>#F8F8F2</string></dict></dict>
<dict><key>scope</key><string>comment</string><key>settings</key><dict><key>foreground</key><string>#75715E</string></dict></dict>
</array></dict></plist>"#,
        );
        write(dir.path(), "t/theme.json", r#"{ "tokenColors": "./x.tmTheme" }"#);
        let t = resolve(dir.path(), "t/theme.json").unwrap();
        assert_eq!(t.colors["editor.background"], "#272822");
        assert!(t.token_colors.iter().any(|r| r.scope == ["comment"] && r.foreground.as_deref() == Some("#75715e")));
    }

    #[test]
    fn cycles_and_escapes_fail() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.json", r#"{ "include": "./b.json" }"#);
        write(dir.path(), "b.json", r#"{ "include": "./a.json" }"#);
        assert!(resolve(dir.path(), "a.json").is_err());
        write(dir.path(), "c.json", r#"{ "include": "../../secret.json" }"#);
        assert!(matches!(resolve(dir.path(), "c.json"), Err(ExtError::Unsafe(_))));
    }
}

//! Extension manifest (`package.json`) parsing and contribution classification.
//!
//! Only declarative contribution points are supported by Orkestr's editor
//! (`themes`, `iconThemes`, `grammars`, `languages`, `snippets`); they are
//! read as data. Every other contribution point and any activation code
//! (`main` / `browser`) is listed as "needs full VS Code".

use std::collections::BTreeMap;

use orkestr_protocol::methods::extensions::{
    ContributionSummary, GrammarContribution, IconThemeContribution, LanguageContribution, SnippetContribution,
    ThemeContribution, UnsupportedContribution,
};
use serde_json::{Map, Value};

use crate::error::{ExtError, ExtResult};
use crate::paths;

/// VS Code API level whose declarative contribution format Orkestr reads.
/// `engines.vscode` ranges are checked against it (informational only: no
/// extension code runs).
pub const EMULATED_VSCODE_VERSION: &str = "1.95.0";

/// Contribution points Orkestr's editor applies.
pub const SUPPORTED_POINTS: &[&str] = &["themes", "iconThemes", "grammars", "languages", "snippets"];

/// Parsed manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Publisher.
    pub publisher: String,
    /// Name.
    pub name: String,
    /// Display name (NLS resolved).
    pub display_name: String,
    /// Version.
    pub version: String,
    /// Description (NLS resolved).
    pub description: Option<String>,
    /// License.
    pub license: Option<String>,
    /// `engines.vscode`.
    pub engine: Option<String>,
    /// Icon path (normalized).
    pub icon: Option<String>,
    /// Whether `main` / `browser` is declared.
    pub has_code: bool,
    /// Raw `contributes`.
    pub contributes: Map<String, Value>,
}

impl Manifest {
    /// `publisher.name`, lowercase.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}.{}", self.publisher, self.name).to_ascii_lowercase()
    }

    /// Whether `engines.vscode` accepts [`EMULATED_VSCODE_VERSION`].
    #[must_use]
    pub fn engine_compatible(&self) -> bool {
        self.engine.as_deref().is_none_or(engine_accepts)
    }
}

/// Whether an `engines.vscode` range accepts [`EMULATED_VSCODE_VERSION`].
#[must_use]
pub fn engine_accepts(range: &str) -> bool {
    let r = range.trim();
    if r.is_empty() || r == "*" {
        return true;
    }
    // VS Code ranges sometimes carry `-insider` pre-release tags; compare the release part.
    let cleaned: String = r.split('-').next().unwrap_or(r).to_owned();
    let Ok(req) = semver::VersionReq::parse(&cleaned) else {
        return false;
    };
    let Ok(v) = semver::Version::parse(EMULATED_VSCODE_VERSION) else {
        return false;
    };
    req.matches(&v)
}

fn identifier(v: &Value, field: &str) -> ExtResult<String> {
    let s = v.get(field).and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if s.is_empty() {
        return Err(ExtError::Invalid(format!("package.json has no `{field}`")));
    }
    if s.len() > 128 || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || s.starts_with('.')
    {
        return Err(ExtError::Invalid(format!("package.json `{field}` `{s}` is not a valid identifier")));
    }
    Ok(s.to_owned())
}

/// Resolves `%key%` placeholders from `package.nls.json`.
fn nls(value: Option<&str>, table: Option<&Value>) -> Option<String> {
    let v = value?.trim();
    if v.len() > 2 && v.starts_with('%') && v.ends_with('%') {
        let key = &v[1..v.len() - 1];
        let resolved = table.and_then(|t| t.get(key)).and_then(|e| {
            e.as_str().map(str::to_owned).or_else(|| e.get("message").and_then(Value::as_str).map(str::to_owned))
        });
        return resolved.or_else(|| Some(key.to_owned()));
    }
    Some(v.to_owned())
}

/// Parses `package.json` (with an optional `package.nls.json`).
pub fn parse(text: &str, nls_table: Option<&Value>) -> ExtResult<Manifest> {
    let v = crate::jsonc::parse(text)?;
    if !v.is_object() {
        return Err(ExtError::Invalid("package.json is not an object".into()));
    }
    let publisher = identifier(&v, "publisher")?;
    let name = identifier(&v, "name")?;
    let version = v.get("version").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_owned();
    if version.is_empty()
        || version.len() > 64
        || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        return Err(ExtError::Invalid(format!("package.json `version` `{version}` is not valid")));
    }
    let display_name =
        nls(v.get("displayName").and_then(Value::as_str), nls_table).filter(|s| !s.is_empty()).unwrap_or(name.clone());
    let description = nls(v.get("description").and_then(Value::as_str), nls_table).filter(|s| !s.is_empty());
    let license = v.get("license").and_then(Value::as_str).map(str::to_owned);
    let engine = v.pointer("/engines/vscode").and_then(Value::as_str).map(str::to_owned);
    let icon = v.get("icon").and_then(Value::as_str).and_then(|p| paths::normalize(p).ok());
    let has_code = v.get("main").and_then(Value::as_str).is_some() || v.get("browser").and_then(Value::as_str).is_some();
    let contributes = v.get("contributes").and_then(Value::as_object).cloned().unwrap_or_default();
    Ok(Manifest { publisher, name, display_name, version, description, license, engine, icon, has_code, contributes })
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect())
        .unwrap_or_default()
}

fn entries<'a>(c: &'a Map<String, Value>, key: &str) -> impl Iterator<Item = &'a Map<String, Value>> {
    c.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object)
}

fn path_of(e: &Map<String, Value>, warn: &mut Vec<String>) -> Option<String> {
    let raw = e.get("path").and_then(Value::as_str)?;
    match paths::normalize(raw) {
        Ok(p) => Some(p),
        Err(err) => {
            warn.push(err.to_string());
            None
        }
    }
}

/// Classifies the contributions of a manifest.
#[must_use]
pub fn summarize(m: &Manifest, nls_table: Option<&Value>) -> ContributionSummary {
    let c = &m.contributes;
    let mut warn = Vec::new();
    let mut out = ContributionSummary { has_code: m.has_code, ..ContributionSummary::default() };

    for e in entries(c, "themes") {
        let Some(path) = path_of(e, &mut warn) else { continue };
        let label = nls(e.get("label").and_then(Value::as_str), nls_table).unwrap_or_else(|| path.clone());
        let id = e.get("id").and_then(Value::as_str).map_or_else(|| label.clone(), str::to_owned);
        let ui_theme = e.get("uiTheme").and_then(Value::as_str).unwrap_or("vs-dark").to_owned();
        out.themes.push(ThemeContribution { id, label, ui_theme, path });
    }
    for e in entries(c, "iconThemes") {
        let Some(path) = path_of(e, &mut warn) else { continue };
        let Some(id) = e.get("id").and_then(Value::as_str).map(str::to_owned) else { continue };
        let label = nls(e.get("label").and_then(Value::as_str), nls_table).unwrap_or_else(|| id.clone());
        out.icon_themes.push(IconThemeContribution { id, label, path });
    }
    for e in entries(c, "grammars") {
        let Some(path) = path_of(e, &mut warn) else { continue };
        let Some(scope_name) = e.get("scopeName").and_then(Value::as_str).map(str::to_owned) else { continue };
        out.grammars.push(GrammarContribution {
            language: e.get("language").and_then(Value::as_str).map(str::to_owned),
            scope_name,
            path,
            embedded_languages: str_map(e.get("embeddedLanguages")),
            injection_to: str_list(e.get("injectTo")),
            token_types: str_map(e.get("tokenTypes")),
        });
    }
    for e in entries(c, "languages") {
        let Some(id) = e.get("id").and_then(Value::as_str).map(str::to_owned) else { continue };
        let configuration = e.get("configuration").and_then(Value::as_str).and_then(|p| match paths::normalize(p) {
            Ok(p) => Some(p),
            Err(err) => {
                warn.push(err.to_string());
                None
            }
        });
        out.languages.push(LanguageContribution {
            id,
            aliases: str_list(e.get("aliases")),
            extensions: str_list(e.get("extensions")),
            filenames: str_list(e.get("filenames")),
            filename_patterns: str_list(e.get("filenamePatterns")),
            first_line: e.get("firstLine").and_then(Value::as_str).map(str::to_owned),
            configuration,
        });
    }
    for e in entries(c, "snippets") {
        let Some(path) = path_of(e, &mut warn) else { continue };
        let language = e.get("language").and_then(Value::as_str).unwrap_or("*").to_owned();
        out.snippets.push(SnippetContribution { language, path });
    }
    for (k, v) in c {
        if SUPPORTED_POINTS.contains(&k.as_str()) {
            continue;
        }
        let count = match v {
            Value::Array(a) => a.len(),
            Value::Object(_) => 1,
            _ => 1,
        };
        out.unsupported.push(UnsupportedContribution { kind: k.clone(), count: u32::try_from(count).unwrap_or(u32::MAX) });
    }
    if m.has_code {
        out.unsupported.push(UnsupportedContribution { kind: "activation".into(), count: 1 });
    }
    for w in warn {
        tracing::warn!(extension = %m.id(), warning = %w, "skipped an unsafe contribution path");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        // comments are allowed
        "name": "theme-dracula", "publisher": "dracula-theme", "version": "2.25.1",
        "displayName": "%displayName%", "description": "Dark theme",
        "license": "MIT", "engines": { "vscode": "^1.13.0" }, "icon": "./icon.png",
        "main": "./out/extension.js",
        "contributes": {
            "themes": [
                { "label": "Dracula", "uiTheme": "vs-dark", "path": "./theme/dracula.json" },
                { "id": "soft", "label": "Dracula Soft", "uiTheme": "vs-dark", "path": "./theme/dracula-soft.json" },
                { "label": "Evil", "path": "../../escape.json" }
            ],
            "grammars": [
                { "language": "rust", "scopeName": "source.rust", "path": "./syntaxes/rust.tmLanguage.json",
                  "embeddedLanguages": { "meta.embedded.block.sql": "sql" } },
                { "scopeName": "markdown.rust.codeblock", "path": "./syntaxes/md.json", "injectTo": ["text.html.markdown"] }
            ],
            "languages": [
                { "id": "rust", "aliases": ["Rust", "rust"], "extensions": [".rs"], "configuration": "./language-configuration.json" }
            ],
            "snippets": [ { "language": "rust", "path": "./snippets/rust.json" } ],
            "commands": [ { "command": "a" }, { "command": "b" } ],
            "configuration": { "title": "x", "properties": {} },
            "keybindings": [ { "key": "ctrl+a" } ],
        },
    }"#;

    #[test]
    fn parses_and_classifies() {
        let nls = serde_json::json!({ "displayName": "Dracula Official" });
        let m = parse(MANIFEST, Some(&nls)).unwrap();
        assert_eq!(m.id(), "dracula-theme.theme-dracula");
        assert_eq!(m.display_name, "Dracula Official");
        assert_eq!(m.icon.as_deref(), Some("icon.png"));
        assert!(m.engine_compatible());
        let s = summarize(&m, Some(&nls));
        assert_eq!(s.themes.len(), 2, "the escaping theme path is dropped");
        assert_eq!(s.themes[0].id, "Dracula");
        assert_eq!(s.themes[0].path, "theme/dracula.json");
        assert_eq!(s.themes[1].id, "soft");
        assert_eq!(s.grammars.len(), 2);
        assert_eq!(s.grammars[0].embedded_languages.get("meta.embedded.block.sql").map(String::as_str), Some("sql"));
        assert_eq!(s.grammars[1].injection_to, vec!["text.html.markdown".to_string()]);
        assert_eq!(s.grammars[1].language, None);
        assert_eq!(s.languages[0].configuration.as_deref(), Some("language-configuration.json"));
        assert_eq!(s.snippets[0].path, "snippets/rust.json");
        let kinds: Vec<_> = s.unsupported.iter().map(|u| (u.kind.as_str(), u.count)).collect();
        assert!(kinds.contains(&("commands", 2)));
        assert!(kinds.contains(&("configuration", 1)));
        assert!(kinds.contains(&("keybindings", 1)));
        assert!(kinds.contains(&("activation", 1)));
        assert!(s.has_code);
    }

    #[test]
    fn rejects_bad_identifiers() {
        assert!(parse(r#"{"name":"../x","publisher":"p","version":"1.0.0"}"#, None).is_err());
        assert!(parse(r#"{"name":"x","publisher":"p","version":"1.0.0; rm"}"#, None).is_err());
        assert!(parse(r#"{"name":"x","version":"1.0.0"}"#, None).is_err());
        assert!(parse("[]", None).is_err());
    }

    #[test]
    fn engine_ranges() {
        assert!(engine_accepts("^1.60.0"));
        assert!(engine_accepts(">=1.50.0"));
        assert!(engine_accepts("*"));
        assert!(engine_accepts("^1.80.0-insider"));
        assert!(!engine_accepts("^1.99.0"));
        assert!(!engine_accepts("garbage"));
    }
}

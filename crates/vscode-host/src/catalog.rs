//! Catalog of installed VS Code extensions read from their `package.json`
//! manifests on disk: display name, description, icon, contributed commands
//! (with localized titles from `package.nls*.json`), keybindings, view
//! containers and settings. Works without a running server; when the bridge
//! is connected, commands that an extension registers at runtime without
//! declaring them ("hidden" commands) are merged in by [`merge_runtime`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use orkestr_protocol::methods::vscode::{VsCodeCommandInfo, VsCodeExtensionCommands, VsCodeViewContainer};
use serde_json::Value;

use crate::bridge::BRIDGE_EXTENSION_ID;

/// Largest icon embedded as a data URL.
const MAX_ICON_BYTES: u64 = 256 * 1024;
/// Commands kept per extension.
const MAX_COMMANDS: usize = 600;

/// Words that make a command "dangerous" (needs a confirmation in Orkestr).
const DANGEROUS_WORDS: &[&str] = &[
    "delete", "remove", "uninstall", "reset", "clear", "discard", "kill", "terminate", "revert", "force", "push",
    "purge", "wipe", "drop", "destroy", "logout", "log out", "signout", "sign out", "erase", "overwrite", "sendsequence",
    "runselectedtext", "runactivefile", "unsafe", "dangerously", "bypass",
];

/// Whether running `id` (titled `title`) looks destructive.
#[must_use]
pub fn is_dangerous_command(id: &str, title: &str) -> bool {
    let hay = format!("{} {}", id.to_ascii_lowercase(), title.to_lowercase());
    // Split camelCase ids (`deleteFile`) into words too.
    let mut split = String::new();
    for ch in id.chars() {
        if ch.is_uppercase() {
            split.push(' ');
        }
        split.extend(ch.to_lowercase());
    }
    let hay = format!("{hay} {split}");
    DANGEROUS_WORDS.iter().any(|w| hay.contains(w))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Localized strings of one extension (`package.nls.json` + locale overrides).
fn load_nls(dir: &Path, locale: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut merge = |v: Option<Value>| {
        if let Some(Value::Object(o)) = v {
            for (k, v) in o {
                let s = match v {
                    Value::String(s) => Some(s),
                    Value::Object(m) => m.get("message").and_then(Value::as_str).map(str::to_owned),
                    _ => None,
                };
                if let Some(s) = s {
                    map.insert(k, s);
                }
            }
        }
    };
    merge(read_json(&dir.join("package.nls.json")));
    if let Some(loc) = locale.map(str::to_ascii_lowercase).filter(|l| !l.is_empty() && l != "en") {
        merge(read_json(&dir.join(format!("package.nls.{loc}.json"))));
        if let Some((lang, _)) = loc.split_once('-') {
            merge(read_json(&dir.join(format!("package.nls.{lang}.json"))));
        }
    }
    map
}

fn localize(v: Option<&Value>, nls: &HashMap<String, String>) -> Option<String> {
    let s = match v? {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("value").or_else(|| o.get("original")).and_then(Value::as_str)?.to_owned(),
        _ => return None,
    };
    let s = s.trim();
    if let Some(key) = s.strip_prefix('%').and_then(|k| k.strip_suffix('%')) {
        return Some(nls.get(key).cloned().unwrap_or_else(|| key.to_owned()));
    }
    Some(s.to_owned()).filter(|s| !s.is_empty())
}

fn as_list(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(o @ Value::Object(_)) => vec![o],
        _ => Vec::new(),
    }
}

fn icon_data_url(dir: &Path, rel: &str) -> Option<String> {
    let path = dir.join(rel);
    // Stay inside the extension directory.
    let canon_dir = std::fs::canonicalize(dir).ok()?;
    let canon = std::fs::canonicalize(&path).ok()?;
    if !canon.starts_with(&canon_dir) || std::fs::metadata(&canon).ok()?.len() > MAX_ICON_BYTES {
        return None;
    }
    let mime = match canon.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(&canon).ok()?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Humanizes a runtime-only command id: `claude-vscode.openInNewTab` → `Open In New Tab`.
#[must_use]
pub fn humanize_command_id(id: &str) -> String {
    let last = id.rsplit('.').next().unwrap_or(id);
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in last.chars() {
        if ch == '-' || ch == '_' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        out.push(ch);
    }
    let mut chars = out.trim().chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => id.to_owned(),
    }
}

/// Parses one extension directory (`None` when it has no valid manifest).
#[must_use]
pub fn parse_extension(dir: &Path, locale: Option<&str>) -> Option<VsCodeExtensionCommands> {
    let pkg = read_json(&dir.join("package.json"))?;
    let publisher = pkg.get("publisher")?.as_str()?.to_owned();
    let name = pkg.get("name")?.as_str()?.to_owned();
    let id = format!("{publisher}.{name}").to_ascii_lowercase();
    let nls = load_nls(dir, locale);
    let contributes = pkg.get("contributes").cloned().unwrap_or(Value::Null);

    // Keybindings (Windows key preferred on this host family, else the default).
    let mut keys: HashMap<String, String> = HashMap::new();
    for kb in as_list(contributes.get("keybindings")) {
        let Some(cmd) = kb.get("command").and_then(Value::as_str) else { continue };
        let key = kb.get("win").or_else(|| kb.get("key")).and_then(Value::as_str);
        if let Some(key) = key {
            keys.entry(cmd.to_owned()).or_insert_with(|| key.to_owned());
        }
    }
    // Commands hidden from the command palette (`when: false`).
    let mut hidden: HashSet<String> = HashSet::new();
    if let Some(palette) = contributes.get("menus").and_then(|m| m.get("commandPalette")) {
        for item in as_list(Some(palette)) {
            if item.get("when").and_then(Value::as_str).is_some_and(|w| w.trim() == "false") {
                if let Some(c) = item.get("command").and_then(Value::as_str) {
                    hidden.insert(c.to_owned());
                }
            }
        }
    }
    let mut commands = Vec::new();
    let mut seen = HashSet::new();
    for c in as_list(contributes.get("commands")) {
        let Some(cid) = c.get("command").and_then(Value::as_str) else { continue };
        if !seen.insert(cid.to_owned()) {
            continue;
        }
        let title = localize(c.get("title"), &nls).unwrap_or_else(|| humanize_command_id(cid));
        let category = localize(c.get("category"), &nls);
        commands.push(VsCodeCommandInfo {
            id: cid.to_owned(),
            dangerous: is_dangerous_command(cid, &title),
            title,
            category,
            hidden: hidden.contains(cid),
            runtime_only: false,
            keybinding: keys.get(cid).cloned(),
        });
        if commands.len() >= MAX_COMMANDS {
            break;
        }
    }
    let mut view_containers = Vec::new();
    if let Some(Value::Object(locs)) = contributes.get("viewsContainers") {
        for (location, list) in locs {
            for vc in as_list(Some(list)) {
                let Some(vid) = vc.get("id").and_then(Value::as_str) else { continue };
                view_containers.push(VsCodeViewContainer {
                    id: vid.to_owned(),
                    title: localize(vc.get("title"), &nls).unwrap_or_else(|| vid.to_owned()),
                    location: location.clone(),
                    command: format!("workbench.view.extension.{vid}"),
                });
            }
        }
    }
    let has_settings = match contributes.get("configuration") {
        Some(Value::Array(a)) => a.iter().any(|c| c.get("properties").is_some_and(|p| p.as_object().is_some_and(|o| !o.is_empty()))),
        Some(c @ Value::Object(_)) => c.get("properties").is_some_and(|p| p.as_object().is_some_and(|o| !o.is_empty())),
        _ => false,
    };
    let categories = pkg
        .get("categories")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    Some(VsCodeExtensionCommands {
        display_name: localize(pkg.get("displayName"), &nls).unwrap_or_else(|| name.clone()),
        description: localize(pkg.get("description"), &nls),
        version: pkg.get("version").and_then(Value::as_str).map(str::to_owned),
        publisher,
        icon: pkg.get("icon").and_then(Value::as_str).and_then(|rel| icon_data_url(dir, rel)),
        categories,
        commands,
        view_containers,
        has_settings,
        id,
        dir: dir.to_string_lossy().into_owned(),
    })
}

fn version_key(v: Option<&str>) -> Vec<u64> {
    v.unwrap_or("0").split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
}

/// Folders VS Code marked as removed (`.obsolete`).
fn obsolete(extensions_dir: &Path) -> HashSet<String> {
    match read_json(&extensions_dir.join(".obsolete")) {
        Some(Value::Object(o)) => o.into_iter().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k).collect(),
        _ => HashSet::new(),
    }
}

/// Scans `extensions_dir` (newest version per id, sorted by display name).
/// The Orkestr bridge itself is left out.
#[must_use]
pub fn scan(extensions_dir: &Path, locale: Option<&str>) -> Vec<VsCodeExtensionCommands> {
    let skip = obsolete(extensions_dir);
    let mut by_id: BTreeMap<String, VsCodeExtensionCommands> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(extensions_dir) else { return Vec::new() };
    let dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| !skip.contains(&e.file_name().to_string_lossy().into_owned()))
        .map(|e| e.path())
        .collect();
    for dir in dirs {
        let Some(ext) = parse_extension(&dir, locale) else { continue };
        if ext.id == BRIDGE_EXTENSION_ID {
            continue;
        }
        let newer = by_id
            .get(&ext.id)
            .is_none_or(|cur| version_key(ext.version.as_deref()) > version_key(cur.version.as_deref()));
        if newer {
            by_id.insert(ext.id.clone(), ext);
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by_key(|e| e.display_name.to_lowercase());
    out
}

/// Adds commands that are registered at runtime (`getCommands(true)`) but not
/// declared in any manifest to the extension whose command prefix they share.
/// Ids starting with `_` are internal and skipped.
pub fn merge_runtime(extensions: &mut [VsCodeExtensionCommands], runtime: &[String]) {
    let mut prefix_owner: HashMap<String, usize> = HashMap::new();
    let mut known: HashSet<String> = HashSet::new();
    for (i, ext) in extensions.iter().enumerate() {
        let name = ext.id.split_once('.').map_or(ext.id.as_str(), |(_, n)| n).to_owned();
        prefix_owner.entry(name).or_insert(i);
        prefix_owner.entry(ext.id.clone()).or_insert(i);
        for c in &ext.commands {
            known.insert(c.id.clone());
            if let Some((prefix, _)) = c.id.rsplit_once('.') {
                prefix_owner.entry(prefix.to_ascii_lowercase()).or_insert(i);
            }
        }
    }
    for id in runtime {
        if id.starts_with('_') || known.contains(id) {
            continue;
        }
        let Some((prefix, _)) = id.rsplit_once('.') else { continue };
        let first = id.split_once('.').map_or("", |(f, _)| f);
        let owner =
            prefix_owner.get(&prefix.to_ascii_lowercase()).or_else(|| prefix_owner.get(&first.to_ascii_lowercase()));
        let Some(&owner) = owner else { continue };
        let ext = &mut extensions[owner];
        if ext.commands.len() >= MAX_COMMANDS {
            continue;
        }
        let title = humanize_command_id(id);
        ext.commands.push(VsCodeCommandInfo {
            id: id.clone(),
            dangerous: is_dangerous_command(id, &title),
            title,
            category: None,
            hidden: true,
            runtime_only: true,
            keybinding: None,
        });
        known.insert(id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let ext = tmp.path().join("mechatroner.rainbow-csv-3.6.0");
        write(
            &ext,
            "package.json",
            r#"{
              "publisher": "mechatroner", "name": "rainbow-csv", "version": "3.6.0",
              "displayName": "%ext.name%", "description": "Highlight CSV", "icon": "icon.png",
              "categories": ["Formatters"],
              "contributes": {
                "commands": [
                  {"command": "rainbow-csv.Align", "title": "%cmd.align%", "category": "Rainbow CSV"},
                  {"command": "rainbow-csv.Shrink", "title": {"value": "Shrink", "original": "Shrink"}},
                  {"command": "rainbow-csv.DeleteCache", "title": "Delete cache"},
                  {"command": "rainbow-csv.Internal", "title": "Internal"}
                ],
                "keybindings": [{"command": "rainbow-csv.Align", "key": "ctrl+alt+a"}],
                "menus": {"commandPalette": [{"command": "rainbow-csv.Internal", "when": "false"}]},
                "viewsContainers": {"activitybar": [{"id": "rainbow", "title": "Rainbow", "icon": "x.svg"}]},
                "configuration": {"properties": {"rainbow_csv.enable": {"type": "boolean"}}}
              }
            }"#,
        );
        write(&ext, "package.nls.json", r#"{"ext.name": "Rainbow CSV", "cmd.align": "Align columns"}"#);
        write(&ext, "package.nls.ru.json", r#"{"cmd.align": {"message": "Выровнять столбцы"}}"#);
        std::fs::write(ext.join("icon.png"), [0x89, b'P', b'N', b'G']).unwrap();
        // An older version and an obsolete folder are ignored.
        write(
            &tmp.path().join("mechatroner.rainbow-csv-3.0.0"),
            "package.json",
            r#"{"publisher":"mechatroner","name":"rainbow-csv","version":"3.0.0"}"#,
        );
        write(&tmp.path().join("old.gone-1.0.0"), "package.json", r#"{"publisher":"old","name":"gone","version":"1.0.0"}"#);
        write(tmp.path(), ".obsolete", r#"{"old.gone-1.0.0": true}"#);
        // The bridge is hidden.
        write(
            &tmp.path().join("orkestr.orkestr-bridge-0.1.0"),
            "package.json",
            r#"{"publisher":"orkestr","name":"orkestr-bridge","version":"0.1.0"}"#,
        );
        tmp
    }

    #[test]
    fn maps_manifests_to_command_cards() {
        let tmp = fixture();
        let list = scan(tmp.path(), None);
        assert_eq!(list.len(), 1, "{list:?}");
        let ext = &list[0];
        assert_eq!(ext.id, "mechatroner.rainbow-csv");
        assert_eq!(ext.display_name, "Rainbow CSV");
        assert_eq!(ext.version.as_deref(), Some("3.6.0"));
        assert!(ext.icon.as_deref().unwrap().starts_with("data:image/png;base64,"));
        assert!(ext.has_settings);
        assert_eq!(ext.view_containers[0].command, "workbench.view.extension.rainbow");
        assert_eq!(ext.view_containers[0].location, "activitybar");
        let align = &ext.commands[0];
        assert_eq!(align.title, "Align columns");
        assert_eq!(align.category.as_deref(), Some("Rainbow CSV"));
        assert_eq!(align.keybinding.as_deref(), Some("ctrl+alt+a"));
        assert!(!align.dangerous && !align.hidden);
        assert_eq!(ext.commands[1].title, "Shrink");
        assert!(ext.commands[2].dangerous, "delete is dangerous");
        assert!(ext.commands[3].hidden, "when:false = hidden from the palette");
    }

    #[test]
    fn localizes_titles() {
        let tmp = fixture();
        let list = scan(tmp.path(), Some("ru"));
        assert_eq!(list[0].commands[0].title, "Выровнять столбцы");
        assert_eq!(list[0].display_name, "Rainbow CSV", "falls back to package.nls.json");
    }

    #[test]
    fn merges_runtime_only_commands() {
        let tmp = fixture();
        let mut list = scan(tmp.path(), None);
        merge_runtime(
            &mut list,
            &[
                "rainbow-csv.Align".into(),
                "rainbow-csv.secretToggle".into(),
                "_rainbow-csv.private".into(),
                "workbench.action.showCommands".into(),
            ],
        );
        let extra: Vec<_> = list[0].commands.iter().filter(|c| c.runtime_only).collect();
        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].id, "rainbow-csv.secretToggle");
        assert_eq!(extra[0].title, "Secret Toggle");
        assert!(extra[0].hidden);
    }

    #[test]
    fn danger_heuristics() {
        assert!(is_dangerous_command("workbench.action.terminal.sendSequence", "Send sequence"));
        assert!(is_dangerous_command("git.push", "Push"));
        assert!(is_dangerous_command("ext.deleteFile", "Something"));
        assert!(!is_dangerous_command("workbench.action.showCommands", "Show All Commands"));
        assert!(!is_dangerous_command("claude-vscode.editor.open", "Open in New Tab"));
        assert_eq!(humanize_command_id("claude-vscode.openInNewTab"), "Open In New Tab");
    }
}

//! Extension management that the server CLI cannot do (ADR-0043):
//! **enable / disable** from Orkestr, and validation of the theme colors
//! Orkestr pushes into the embedded workbench.
//!
//! The VS Code server keeps its extension list in `<extensions>/extensions.json`
//! and **deletes** every folder of its extensions directory that the list
//! does not mention. The browser workbench stores its own enablement state in
//! the page's IndexedDB, out of Orkestr's reach. So "disabled from Orkestr"
//! means: while the server is stopped, the extension's folder is *moved* to
//! `<root>/disabled-extensions/` and its list entry is kept next to it
//! (`entries.json`); enabling moves both back. Moving only happens while the
//! server is stopped (Windows keeps loaded native modules locked), so a
//! change made while it runs is applied on the next start.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{HostError, HostResult};

/// Stash of disabled entries (inside the disabled-extensions directory).
pub const ENTRIES_FILE: &str = "entries.json";
/// The server's extension list.
pub const PROFILE_FILE: &str = "extensions.json";
/// Most colors one theme sync may carry.
pub const MAX_THEME_COLORS: usize = 400;

/// Outcome of [`apply_disabled`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// Extensions moved out of the extensions directory.
    pub disabled: Vec<String>,
    /// Extensions moved back.
    pub enabled: Vec<String>,
    /// Problems (the affected extension keeps its previous state).
    pub errors: Vec<String>,
}

fn read_list(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default()
}

fn write_list(path: &Path, list: &[Value]) -> HostResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.orkestr-tmp");
    let text = serde_json::to_vec(list).map_err(|e| HostError::Invalid(e.to_string()))?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Lower-case `identifier.id` of a list entry.
#[must_use]
pub fn entry_id(entry: &Value) -> Option<String> {
    entry.get("identifier")?.get("id")?.as_str().map(str::to_ascii_lowercase)
}

/// `relativeLocation` of a list entry — only a single, plain path component
/// is accepted (never `..`, separators or a drive).
#[must_use]
pub fn entry_dir(entry: &Value) -> Option<String> {
    let rel = entry.get("relativeLocation")?.as_str()?;
    let ok = !rel.is_empty()
        && rel.len() <= 255
        && rel != "."
        && rel != ".."
        && !rel.contains(['/', '\\', ':'])
        && !rel.chars().any(char::is_control);
    ok.then(|| rel.to_owned())
}

fn entry_version(entry: &Value) -> Option<String> {
    entry.get("version").and_then(Value::as_str).map(str::to_owned)
}

/// Extensions currently stashed as disabled: `(id, version)`, sorted by id.
#[must_use]
pub fn stashed(stash_dir: &Path) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = read_list(&stash_dir.join(ENTRIES_FILE))
        .iter()
        .filter_map(|e| Some((entry_id(e)?, entry_version(e))))
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Moves the folders of `disabled` ids out of `ext_dir` (and every stashed
/// extension that is no longer in `disabled` back). Must only run while the
/// server is stopped.
pub fn apply_disabled(ext_dir: &Path, stash_dir: &Path, disabled: &[String]) -> ApplyReport {
    let mut report = ApplyReport::default();
    let profile_path = ext_dir.join(PROFILE_FILE);
    let stash_path = stash_dir.join(ENTRIES_FILE);
    let mut profile = read_list(&profile_path);
    let mut stash = read_list(&stash_path);
    let wanted = |id: &str| disabled.iter().any(|d| d.eq_ignore_ascii_case(id));
    let (mut profile_changed, mut stash_changed) = (false, false);

    // Disable: profile → stash.
    let mut keep = Vec::with_capacity(profile.len());
    for entry in profile.drain(..) {
        let (Some(id), Some(dir)) = (entry_id(&entry), entry_dir(&entry)) else {
            keep.push(entry);
            continue;
        };
        if !wanted(&id) {
            keep.push(entry);
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(stash_dir) {
            report.errors.push(format!("{id}: {e}"));
            keep.push(entry);
            continue;
        }
        let from = ext_dir.join(&dir);
        let to = stash_dir.join(&dir);
        let moved = if from.is_dir() {
            // An older stashed copy of the same folder is replaced.
            let cleared = if to.exists() { std::fs::remove_dir_all(&to) } else { Ok(()) };
            cleared.and_then(|()| std::fs::rename(&from, &to))
        } else {
            Ok(())
        };
        match moved {
            Ok(()) => {
                stash.retain(|s| entry_id(s).as_deref() != Some(id.as_str()));
                stash.push(entry);
                report.disabled.push(id);
                profile_changed = true;
                stash_changed = true;
            }
            Err(e) => {
                report.errors.push(format!("{id}: {e}"));
                keep.push(entry);
            }
        }
    }
    profile = keep;

    // Enable: stash → profile.
    let mut still = Vec::with_capacity(stash.len());
    for entry in stash.drain(..) {
        let (Some(id), Some(dir)) = (entry_id(&entry), entry_dir(&entry)) else {
            stash_changed = true; // drop unusable entries
            continue;
        };
        if wanted(&id) {
            still.push(entry);
            continue;
        }
        let from = stash_dir.join(&dir);
        let to = ext_dir.join(&dir);
        if !from.is_dir() {
            stash_changed = true; // the folder is gone: forget the entry
            continue;
        }
        let moved = if to.exists() {
            Ok(())
        } else {
            std::fs::create_dir_all(ext_dir).and_then(|()| std::fs::rename(&from, &to))
        };
        match moved {
            Ok(()) => {
                if from.is_dir() {
                    // A newer copy was installed meanwhile: drop the stashed one.
                    let _ = std::fs::remove_dir_all(&from);
                }
                if !profile.iter().any(|p| entry_id(p).as_deref() == Some(id.as_str())) {
                    profile.push(entry);
                    profile_changed = true;
                }
                report.enabled.push(id);
                stash_changed = true;
            }
            Err(e) => {
                report.errors.push(format!("{id}: {e}"));
                still.push(entry);
            }
        }
    }
    stash = still;

    // Write the stash first: a crash in between leaves the extension listed
    // in both files, which the next apply resolves, never in neither.
    if stash_changed {
        if let Err(e) = write_list(&stash_path, &stash) {
            report.errors.push(format!("{}: {e}", stash_path.display()));
            return report;
        }
    }
    if profile_changed {
        if let Err(e) = write_list(&profile_path, &profile) {
            report.errors.push(format!("{}: {e}", profile_path.display()));
        }
    }
    report
}

/// Deletes a stashed (disabled) extension for good. Returns whether it was stashed.
///
/// # Errors
/// The folder or the stash file could not be changed.
pub fn remove_stashed(stash_dir: &Path, id: &str) -> HostResult<bool> {
    let stash_path = stash_dir.join(ENTRIES_FILE);
    let mut stash = read_list(&stash_path);
    let before = stash.len();
    let mut dirs: Vec<PathBuf> = Vec::new();
    stash.retain(|e| {
        if entry_id(e).is_some_and(|x| x.eq_ignore_ascii_case(id)) {
            if let Some(d) = entry_dir(e) {
                dirs.push(stash_dir.join(d));
            }
            false
        } else {
            true
        }
    });
    if stash.len() == before {
        return Ok(false);
    }
    for d in dirs {
        if d.is_dir() {
            std::fs::remove_dir_all(&d)?;
        }
    }
    write_list(&stash_path, &stash)?;
    Ok(true)
}

/// Built-in VS Code theme Orkestr's colors are layered on.
#[must_use]
pub fn base_theme(light: bool) -> &'static str {
    if light {
        "Default Light Modern"
    } else {
        "Default Dark Modern"
    }
}

fn is_color_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn is_hex_color(v: &str) -> bool {
    let Some(hex) = v.strip_prefix('#') else { return false };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Validates the colors of a theme sync (`workbench.colorCustomizations`).
///
/// # Errors
/// Too many entries, a malformed color id or a value that is not `#hex`.
pub fn validate_theme_colors(colors: &BTreeMap<String, String>) -> HostResult<()> {
    if colors.len() > MAX_THEME_COLORS {
        return Err(HostError::Invalid(format!("at most {MAX_THEME_COLORS} theme colors")));
    }
    for (k, v) in colors {
        if !is_color_id(k) {
            return Err(HostError::Invalid(format!("invalid color id: {k:.60}")));
        }
        if !is_hex_color(v) {
            return Err(HostError::Invalid(format!("invalid color for {k}: {v:.40}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, dir: &str) -> Value {
        json!({ "identifier": { "id": id }, "version": "1.0.0", "relativeLocation": dir, "location": { "path": dir } })
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let ext = tmp.path().join("extensions");
        let stash = tmp.path().join("disabled-extensions");
        for (id, dir) in [("a.one", "a.one-1.0.0"), ("b.two", "b.two-1.0.0")] {
            std::fs::create_dir_all(ext.join(dir)).unwrap();
            std::fs::write(ext.join(dir).join("package.json"), format!("{{\"name\":\"{id}\"}}")).unwrap();
        }
        write_list(&ext.join(PROFILE_FILE), &[entry("a.one", "a.one-1.0.0"), entry("B.Two", "b.two-1.0.0")]).unwrap();
        (tmp, ext, stash)
    }

    fn ids(path: &Path) -> Vec<String> {
        read_list(path).iter().filter_map(entry_id).collect()
    }

    #[test]
    fn disable_moves_folder_and_entry_out_and_enable_restores_them() {
        let (_tmp, ext, stash) = setup();
        let r = apply_disabled(&ext, &stash, &["b.two".into()]);
        assert_eq!(r.disabled, vec!["b.two".to_owned()]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(!ext.join("b.two-1.0.0").exists());
        assert!(stash.join("b.two-1.0.0").join("package.json").is_file());
        assert_eq!(ids(&ext.join(PROFILE_FILE)), vec!["a.one"]);
        assert_eq!(stashed(&stash), vec![("b.two".to_owned(), Some("1.0.0".to_owned()))]);

        // Idempotent.
        let again = apply_disabled(&ext, &stash, &["b.two".into()]);
        assert!(again.disabled.is_empty() && again.enabled.is_empty());

        let r = apply_disabled(&ext, &stash, &[]);
        assert_eq!(r.enabled, vec!["b.two".to_owned()]);
        assert!(ext.join("b.two-1.0.0").join("package.json").is_file());
        assert!(!stash.join("b.two-1.0.0").exists());
        assert_eq!(ids(&ext.join(PROFILE_FILE)), vec!["a.one", "b.two"]);
        assert!(stashed(&stash).is_empty());
    }

    #[test]
    fn a_reinstalled_copy_wins_over_the_stashed_one() {
        let (_tmp, ext, stash) = setup();
        apply_disabled(&ext, &stash, &["a.one".into()]);
        // Reinstalled while disabled: the folder and entry are back in the profile.
        std::fs::create_dir_all(ext.join("a.one-1.0.0")).unwrap();
        let mut profile = read_list(&ext.join(PROFILE_FILE));
        profile.push(entry("a.one", "a.one-1.0.0"));
        write_list(&ext.join(PROFILE_FILE), &profile).unwrap();
        let r = apply_disabled(&ext, &stash, &[]);
        assert_eq!(r.enabled, vec!["a.one".to_owned()]);
        assert_eq!(ids(&ext.join(PROFILE_FILE)).iter().filter(|i| *i == "a.one").count(), 1);
        assert!(!stash.join("a.one-1.0.0").exists());
    }

    #[test]
    fn unsafe_relative_locations_are_never_moved() {
        for bad in ["..", "../x", "a/b", "a\\b", "C:x", ""] {
            assert!(entry_dir(&json!({ "relativeLocation": bad })).is_none(), "{bad}");
        }
        assert_eq!(entry_dir(&json!({ "relativeLocation": "x.y-1.0.0" })).as_deref(), Some("x.y-1.0.0"));
    }

    #[test]
    fn remove_stashed_deletes_the_folder() {
        let (_tmp, ext, stash) = setup();
        apply_disabled(&ext, &stash, &["a.one".into()]);
        assert!(remove_stashed(&stash, "A.ONE").unwrap());
        assert!(!stash.join("a.one-1.0.0").exists());
        assert!(stashed(&stash).is_empty());
        assert!(!remove_stashed(&stash, "a.one").unwrap());
    }

    #[test]
    fn theme_colors_are_validated() {
        let mut ok = BTreeMap::new();
        ok.insert("editor.background".to_owned(), "#0a0d1c".to_owned());
        ok.insert("focusBorder".to_owned(), "#7c8cffcc".to_owned());
        assert!(validate_theme_colors(&ok).is_ok());
        for (k, v) in [("editor.background", "red"), ("bad key", "#fff"), ("x", "#12345"), ("x", "url(#a)")] {
            let mut m = BTreeMap::new();
            m.insert(k.to_owned(), v.to_owned());
            assert!(validate_theme_colors(&m).is_err(), "{k}={v}");
        }
        let many: BTreeMap<String, String> =
            (0..=MAX_THEME_COLORS).map(|i| (format!("c{i}"), "#000".to_owned())).collect();
        assert!(validate_theme_colors(&many).is_err());
        assert_eq!(base_theme(true), "Default Light Modern");
    }
}

//! VS Code file icon themes (`contributes.iconThemes`): icon definitions
//! (SVG/PNG files or icon-font glyphs) and name → icon associations. Icon
//! and font paths are rewritten to extension-relative paths (confined); the
//! client loads them through `extensions.readAsset`.

use std::collections::BTreeMap;
use std::path::Path;

use orkestr_protocol::methods::extensions::{IconAssociations, IconDefinition, IconFont};
use serde_json::{Map, Value};

use crate::error::ExtResult;
use crate::paths;

/// Max size of an icon theme file.
const MAX_FILE: u64 = 16 << 20;

/// A parsed icon theme (without its reference / label).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IconThemeBody {
    /// Definitions.
    pub definitions: BTreeMap<String, IconDefinition>,
    /// Dark (default) associations.
    pub dark: IconAssociations,
    /// Light overrides.
    pub light: Option<IconAssociations>,
    /// Fonts.
    pub fonts: Vec<IconFont>,
    /// `hidesExplorerArrows`.
    pub hides_explorer_arrows: bool,
}

fn lower_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.trim_start_matches('.').to_ascii_lowercase(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn associations(o: &Map<String, Value>) -> IconAssociations {
    let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
    IconAssociations {
        file: s("file"),
        folder: s("folder"),
        folder_expanded: s("folderExpanded"),
        file_extensions: lower_map(o.get("fileExtensions")),
        file_names: lower_map(o.get("fileNames")),
        folder_names: lower_map(o.get("folderNames")),
        folder_names_expanded: lower_map(o.get("folderNamesExpanded")),
        language_ids: o
            .get("languageIds")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect())
            .unwrap_or_default(),
    }
}

fn font_format(path: &str, declared: Option<&str>) -> String {
    if let Some(f) = declared {
        return f.to_owned();
    }
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".woff2") {
        "woff2".into()
    } else if lower.ends_with(".woff") {
        "woff".into()
    } else if lower.ends_with(".otf") {
        "opentype".into()
    } else {
        "truetype".into()
    }
}

/// Parses an icon theme file `rel` of the extension at `root`.
pub fn resolve(root: &Path, rel: &str) -> ExtResult<IconThemeBody> {
    let rel = paths::normalize(rel)?;
    let file = paths::resolve_existing(root, &rel)?;
    let text = crate::archive::read_text_capped(&file, MAX_FILE)?;
    let v = crate::jsonc::parse(&text)?;
    parse_value(&v, paths::parent(&rel))
}

/// Parses an icon theme JSON value whose file lives in `dir` (extension-relative).
pub fn parse_value(v: &Value, dir: &str) -> ExtResult<IconThemeBody> {
    let empty = Map::new();
    let o = v.as_object().unwrap_or(&empty);
    let mut body = IconThemeBody {
        dark: associations(o),
        light: o.get("light").and_then(Value::as_object).map(associations),
        hides_explorer_arrows: o.get("hidesExplorerArrows").and_then(Value::as_bool).unwrap_or(false),
        ..IconThemeBody::default()
    };
    if let Some(defs) = o.get("iconDefinitions").and_then(Value::as_object) {
        for (id, d) in defs {
            let icon_path = match d.get("iconPath").and_then(Value::as_str) {
                Some(p) => match paths::join_rel(dir, p) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        tracing::debug!(icon = %id, error = %e, "skipped unsafe icon path");
                        None
                    }
                },
                None => None,
            };
            let font_character = d.get("fontCharacter").and_then(Value::as_str).map(str::to_owned);
            if icon_path.is_none() && font_character.is_none() {
                continue;
            }
            body.definitions.insert(
                id.clone(),
                IconDefinition {
                    icon_path,
                    font_character,
                    font_color: d.get("fontColor").and_then(Value::as_str).map(str::to_owned),
                    font_id: d.get("fontId").and_then(Value::as_str).map(str::to_owned),
                    font_size: d.get("fontSize").and_then(Value::as_str).map(str::to_owned),
                },
            );
        }
    }
    if let Some(fonts) = o.get("fonts").and_then(Value::as_array) {
        for f in fonts {
            let Some(id) = f.get("id").and_then(Value::as_str) else { continue };
            let Some(src) = f.get("src").and_then(Value::as_array).and_then(|a| a.first()) else { continue };
            let Some(path) = src.get("path").and_then(Value::as_str).and_then(|p| paths::join_rel(dir, p).ok()) else {
                continue;
            };
            body.fonts.push(IconFont {
                id: id.to_owned(),
                format: font_format(&path, src.get("format").and_then(Value::as_str)),
                path,
                weight: f.get("weight").and_then(Value::as_str).map(str::to_owned),
                style: f.get("style").and_then(Value::as_str).map(str::to_owned),
                size: f.get("size").and_then(Value::as_str).map(str::to_owned),
            });
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_material_style_theme() {
        let v = serde_json::json!({
            "iconDefinitions": {
                "rust": { "iconPath": "./../icons/rust.svg" },
                "folder": { "iconPath": "./../icons/folder.svg" },
                "evil": { "iconPath": "../../../../etc/x.svg" },
                "glyph": { "fontCharacter": "\\E001", "fontColor": "#ff0000" }
            },
            "file": "glyph", "folder": "folder",
            "fileExtensions": { "RS": "rust", ".d.ts": "glyph" },
            "fileNames": { "Cargo.toml": "rust" },
            "languageIds": { "rust": "rust" },
            "light": { "fileExtensions": { "rs": "glyph" } },
            "fonts": [ { "id": "f", "src": [ { "path": "./font.woff2" } ], "size": "150%" } ],
            "hidesExplorerArrows": true
        });
        let b = parse_value(&v, "dist").unwrap();
        assert_eq!(b.definitions["rust"].icon_path.as_deref(), Some("icons/rust.svg"));
        assert!(!b.definitions.contains_key("evil"));
        assert_eq!(b.definitions["glyph"].font_character.as_deref(), Some("\\E001"));
        assert_eq!(b.dark.file_extensions["rs"], "rust");
        assert_eq!(b.dark.file_extensions["d.ts"], "glyph");
        assert_eq!(b.dark.file_names["cargo.toml"], "rust");
        assert_eq!(b.dark.language_ids["rust"], "rust");
        assert_eq!(b.light.as_ref().unwrap().file_extensions["rs"], "glyph");
        assert_eq!(b.fonts[0].path, "dist/font.woff2");
        assert_eq!(b.fonts[0].format, "woff2");
        assert!(b.hides_explorer_arrows);
    }
}

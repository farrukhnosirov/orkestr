//! Validated JSON Patch edits of the UI configuration (§29 "AI-editable UI",
//! ADR-0015).
//!
//! The AI Manager (and the user's own editors) change the UI only through an
//! RFC 6902 subset — `add`, `remove`, `replace`, `test` — over the
//! [`UiConfig`] document. A patch is accepted only if **all** of these hold:
//!
//! 1. at most [`MAX_PATCH_OPS`] operations, each value at most
//!    [`MAX_OP_VALUE_BYTES`] of JSON;
//! 2. every path is a valid JSON pointer inside the actor's allowlist
//!    ([`is_path_allowed`]; `/schemaVersion` is never writable and AI actors
//!    cannot touch the raw docking layout);
//! 3. no string in a value contains markup or code-like constructs
//!    ([`is_safe_text`]: `<`, `>`, `{`, `}`, `;`, `\`, `url(`, `javascript:`,
//!    `expression(`, `@import`, …);
//! 4. the patched document deserializes into the typed [`UiConfig`] **and**
//!    contains no member the schema does not know (serde would silently drop
//!    it otherwise);
//! 5. [`UiConfig::validate`] passes (ranges, token grammar, ids).
//!
//! The outcome carries a minimal, human-readable change list (per changed
//! container) and the **inverse patch**, so every change can be previewed and
//! undone exactly.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ts_rs::TS;

use crate::ui_config::{UiConfig, UiConfigIssue};

/// Max operations in one patch.
pub const MAX_PATCH_OPS: usize = 64;
/// Max serialized size of one operation value.
pub const MAX_OP_VALUE_BYTES: usize = 16 * 1024;
/// Max length of one string inside a patch value.
pub const MAX_PATCH_STRING_LEN: usize = 512;

/// Operation kind (RFC 6902 subset; `move`/`copy` are not supported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum UiPatchOpKind {
    /// Add / set a member, insert into or append to (`-`) an array.
    Add,
    /// Remove a member or array element.
    Remove,
    /// Replace an existing value.
    Replace,
    /// Assert a value (the patch fails if it differs).
    Test,
}

/// One JSON Patch operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UiPatchOp {
    /// Kind.
    pub op: UiPatchOpKind,
    /// JSON pointer (`/prefs/density`).
    pub path: String,
    /// Value for `add` / `replace` / `test`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub value: Option<Value>,
}

impl UiPatchOp {
    /// `add` operation.
    #[must_use]
    pub fn add(path: impl Into<String>, value: Value) -> Self {
        Self { op: UiPatchOpKind::Add, path: path.into(), value: Some(value) }
    }
    /// `replace` operation.
    #[must_use]
    pub fn replace(path: impl Into<String>, value: Value) -> Self {
        Self { op: UiPatchOpKind::Replace, path: path.into(), value: Some(value) }
    }
    /// `remove` operation.
    #[must_use]
    pub fn remove(path: impl Into<String>) -> Self {
        Self { op: UiPatchOpKind::Remove, path: path.into(), value: None }
    }
}

/// Who authored a patch. AI actors get the narrow allowlist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum UiPatchActor {
    /// The user (settings, editors, undo).
    #[default]
    User,
    /// The AI Manager.
    Manager,
    /// An agent.
    Agent,
}

impl UiPatchActor {
    /// Whether this actor is an AI.
    #[must_use]
    pub fn is_ai(self) -> bool {
        matches!(self, Self::Manager | Self::Agent)
    }
}

/// One changed part of the document (per container: a leaf, a whole array or
/// a member that appeared / disappeared).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiChange {
    /// JSON pointer of the changed container.
    pub path: String,
    /// Value before (`None` = did not exist).
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    /// Value after (`None` = removed).
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

/// How strictly operations are matched against the base document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    /// RFC 6902 semantics (`replace`/`remove` need an existing target, `test` must match).
    Strict,
    /// Shape check against a document that may differ from the user's real
    /// one (the Manager's tool validates against defaults): `replace` acts as
    /// `add`, removing a missing target and failing `test`s are ignored.
    Lenient,
}

/// Result of applying a patch.
#[derive(Debug, Clone, PartialEq)]
pub struct UiPatchOutcome {
    /// The patched, validated document.
    pub config: UiConfig,
    /// What changed.
    pub changes: Vec<UiChange>,
    /// Patch that restores the base document from `config`.
    pub inverse: Vec<UiPatchOp>,
    /// Non-fatal notes (e.g. duplicate widget ids dropped).
    pub warnings: Vec<String>,
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> UiConfigIssue {
    UiConfigIssue { path: path.into(), message: message.into() }
}

/// Parses an RFC 6901 JSON pointer into unescaped segments.
pub fn parse_pointer(path: &str) -> Result<Vec<String>, String> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = path.strip_prefix('/') else {
        return Err("a JSON pointer must start with `/`".into());
    };
    if path.len() > 512 {
        return Err("pointer too long".into());
    }
    rest.split('/')
        .map(|seg| {
            let mut out = String::with_capacity(seg.len());
            let mut chars = seg.chars();
            while let Some(c) = chars.next() {
                if c == '~' {
                    match chars.next() {
                        Some('0') => out.push('~'),
                        Some('1') => out.push('/'),
                        _ => return Err("invalid `~` escape in pointer".to_owned()),
                    }
                } else {
                    out.push(c);
                }
            }
            Ok(out)
        })
        .collect()
}

/// Formats segments as a JSON pointer.
#[must_use]
pub fn format_pointer(segs: &[String]) -> String {
    segs.iter().map(|s| format!("/{}", s.replace('~', "~0").replace('/', "~1"))).collect()
}

/// Path allowlist (ADR-0015). `schemaVersion` and the document root are
/// never writable; AI actors may change theme tokens/effects/mode/name, the
/// layout preset, the performance mode and the shell preferences — not the
/// raw docking JSON.
#[must_use]
pub fn is_path_allowed(actor: UiPatchActor, segs: &[String]) -> bool {
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    if actor.is_ai() {
        matches!(
            s.as_slice(),
            ["theme", "tokens", _]
                | ["theme", "effects"]
                | ["theme", "effects", _]
                | ["theme", "mode" | "name"]
                | ["layout", "preset"]
                | ["performanceMode"]
                | ["prefs", ..]
        )
    } else {
        matches!(s.as_slice(), ["theme", ..] | ["layout", ..] | ["performanceMode"] | ["prefs", ..])
    }
}

/// Whether a string is free of markup / code-like constructs.
#[must_use]
pub fn is_safe_text(s: &str) -> bool {
    if s.chars().count() > MAX_PATCH_STRING_LEN {
        return false;
    }
    if s.chars().any(|c| matches!(c, '<' | '>' | '{' | '}' | ';' | '\\' | '`') || c.is_control()) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    !["url(", "expression(", "javascript:", "vbscript:", "data:", "@import", "image-set(", "attr(", "eval("]
        .iter()
        .any(|bad| lower.contains(bad))
}

fn check_strings(v: &Value, at: &str, issues: &mut Vec<UiConfigIssue>) {
    match v {
        Value::String(s) if !is_safe_text(s) => issues.push(issue(at, "unsafe text in value (markup or code is not allowed)")),
        Value::Array(items) => items.iter().for_each(|i| check_strings(i, at, issues)),
        Value::Object(m) => {
            for (k, i) in m {
                if !is_safe_text(k) {
                    issues.push(issue(at, "unsafe member name in value"));
                }
                check_strings(i, at, issues);
            }
        }
        _ => {}
    }
}

/// The document as JSON with every optional section present (so patches can
/// address `/prefs/...` even when the stored document predates it).
#[must_use]
pub fn normalized_value(cfg: &UiConfig) -> Value {
    let mut c = cfg.clone();
    if c.prefs.is_none() {
        c.prefs = Some(crate::ui_prefs::UiPrefs::default());
    }
    serde_json::to_value(&c).unwrap_or(Value::Null)
}

fn get<'a>(root: &'a Value, segs: &[String]) -> Option<&'a Value> {
    segs.iter().try_fold(root, |cur, seg| match cur {
        Value::Object(m) => m.get(seg),
        Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

fn get_mut<'a>(root: &'a mut Value, segs: &[String]) -> Option<&'a mut Value> {
    segs.iter().try_fold(root, |cur, seg| match cur {
        Value::Object(m) => m.get_mut(seg),
        Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get_mut(i)),
        _ => None,
    })
}

/// Applies one operation to `doc`. `Ok(false)` = skipped (lenient).
fn apply_op(doc: &mut Value, op: &UiPatchOp, segs: &[String], mode: PatchMode) -> Result<bool, String> {
    let Some((last, parent_path)) = segs.split_last() else {
        return Err("the document root cannot be patched".into());
    };
    let lenient = mode == PatchMode::Lenient;
    let value = || op.value.clone().ok_or_else(|| "`value` is required".to_owned());
    match op.op {
        UiPatchOpKind::Test => {
            let expected = value()?;
            match get(doc, segs) {
                Some(actual) if *actual == expected => Ok(true),
                _ if lenient => Ok(false),
                _ => Err("test failed: the value differs".into()),
            }
        }
        UiPatchOpKind::Add | UiPatchOpKind::Replace => {
            let v = value()?;
            let replace = op.op == UiPatchOpKind::Replace && !lenient;
            let parent = get_mut(doc, parent_path).ok_or_else(|| "path not found".to_owned())?;
            match parent {
                Value::Object(m) => {
                    if replace && !m.contains_key(last) {
                        return Err("path not found".into());
                    }
                    m.insert(last.clone(), v);
                    Ok(true)
                }
                Value::Array(a) => {
                    if last == "-" {
                        if replace {
                            return Err("`-` cannot be replaced".into());
                        }
                        a.push(v);
                        return Ok(true);
                    }
                    let i: usize = last.parse().map_err(|_| "invalid array index".to_owned())?;
                    if replace || (op.op == UiPatchOpKind::Replace && i < a.len()) {
                        let slot = a.get_mut(i).ok_or_else(|| "array index out of bounds".to_owned())?;
                        *slot = v;
                    } else if i <= a.len() {
                        a.insert(i, v);
                    } else {
                        return Err("array index out of bounds".into());
                    }
                    Ok(true)
                }
                _ => Err("path not found".into()),
            }
        }
        UiPatchOpKind::Remove => {
            let removed = match get_mut(doc, parent_path) {
                Some(Value::Object(m)) => m.remove(last).is_some(),
                Some(Value::Array(a)) => match last.parse::<usize>() {
                    Ok(i) if i < a.len() => {
                        a.remove(i);
                        true
                    }
                    _ => false,
                },
                _ => false,
            };
            match (removed, lenient) {
                (true, _) => Ok(true),
                (false, true) => Ok(false),
                (false, false) => Err("path not found".into()),
            }
        }
    }
}

/// First member of `patched` that the typed document does not know.
fn unknown_member(patched: &Value, typed: &Value, at: &mut Vec<String>) -> Option<String> {
    match (patched, typed) {
        (Value::Object(p), Value::Object(t)) => {
            for (k, v) in p {
                at.push(k.clone());
                let found = match t.get(k) {
                    Some(tv) => unknown_member(v, tv, at),
                    None if v.is_null() => None,
                    None => Some(format_pointer(at)),
                };
                at.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        (Value::Array(p), Value::Array(t)) if p.len() == t.len() => {
            for (i, (pv, tv)) in p.iter().zip(t).enumerate() {
                at.push(i.to_string());
                let found = unknown_member(pv, tv, at);
                at.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        _ => None,
    }
}

/// The container of a change: the path cut at the first array or at the
/// first member missing from `base`. Returns it and whether it exists in base.
fn container(base: &Value, segs: &[String]) -> (Vec<String>, bool) {
    let mut cur = base;
    for (i, seg) in segs.iter().enumerate() {
        match cur {
            Value::Object(m) => match m.get(seg) {
                Some(v) if v.is_array() => return (segs[..=i].to_vec(), true),
                Some(v) => cur = v,
                None => return (segs[..=i].to_vec(), false),
            },
            _ => return (segs[..i].to_vec(), true),
        }
    }
    (segs.to_vec(), true)
}

/// Applies a patch to `base` as `actor` (see the module docs for every rule).
/// Errors list every problem found, with the failing JSON pointer.
pub fn apply_ui_patch(
    base: &UiConfig,
    ops: &[UiPatchOp],
    actor: UiPatchActor,
    mode: PatchMode,
) -> Result<UiPatchOutcome, Vec<UiConfigIssue>> {
    if ops.len() > MAX_PATCH_OPS {
        return Err(vec![issue("", format!("a patch may have at most {MAX_PATCH_OPS} operations"))]);
    }
    let base_value = normalized_value(base);
    let mut doc = base_value.clone();
    let mut issues = Vec::new();
    let mut parsed = Vec::with_capacity(ops.len());
    for op in ops {
        let segs = match parse_pointer(&op.path) {
            Ok(s) => s,
            Err(e) => {
                issues.push(issue(op.path.clone(), e));
                continue;
            }
        };
        if !is_path_allowed(actor, &segs) {
            issues.push(issue(op.path.clone(), "this path is not editable"));
            continue;
        }
        if let Some(v) = &op.value {
            if serde_json::to_vec(v).map_or(usize::MAX, |b| b.len()) > MAX_OP_VALUE_BYTES {
                issues.push(issue(op.path.clone(), "value too large"));
                continue;
            }
            if segs.first().map(String::as_str) != Some("layout") || segs.get(1).map(String::as_str) != Some("docking") {
                check_strings(v, &op.path, &mut issues);
            }
        }
        parsed.push((op, segs));
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    for (op, segs) in &parsed {
        if let Err(e) = apply_op(&mut doc, op, segs, mode) {
            return Err(vec![issue(op.path.clone(), e)]);
        }
    }
    let mut config: UiConfig =
        serde_json::from_value(doc.clone()).map_err(|e| vec![issue("", format!("does not match the UI schema: {e}"))])?;
    let typed = serde_json::to_value(&config).unwrap_or(Value::Null);
    if let Some(path) = unknown_member(&doc, &typed, &mut Vec::new()) {
        return Err(vec![issue(path, "unknown setting")]);
    }
    let mut warnings = Vec::new();
    if let Some(prefs) = config.prefs.as_mut() {
        let before = prefs.hidden_widgets.len();
        prefs.normalize();
        if prefs.hidden_widgets.len() != before {
            warnings.push("duplicate hidden widget ids were merged".to_owned());
        }
    }
    config.validate()?;
    let after_value = normalized_value(&config);

    let mut containers: Vec<(Vec<String>, bool)> = Vec::new();
    for (_, segs) in &parsed {
        let c = container(&base_value, segs);
        if !containers.iter().any(|(p, _)| p == &c.0) {
            containers.push(c);
        }
    }
    // Drop containers nested inside another changed container.
    let all: Vec<Vec<String>> = containers.iter().map(|(p, _)| p.clone()).collect();
    containers.retain(|(p, _)| !all.iter().any(|q| q.len() < p.len() && p.starts_with(q)));

    let mut changes = Vec::new();
    let mut inverse = Vec::new();
    for (path, existed) in containers {
        let before = existed.then(|| get(&base_value, &path).cloned()).flatten();
        let after = get(&after_value, &path).cloned();
        if before == after {
            continue;
        }
        let pointer = format_pointer(&path);
        inverse.push(match &before {
            Some(v) => UiPatchOp::add(pointer.clone(), v.clone()),
            None => UiPatchOp::remove(pointer.clone()),
        });
        changes.push(UiChange { path: pointer, before, after });
    }
    Ok(UiPatchOutcome { config, changes, inverse, warnings })
}

/// Converts arbitrary JSON (e.g. a model's tool arguments) into operations.
pub fn ops_from_json(v: &Value) -> Result<Vec<UiPatchOp>, String> {
    let list = match v {
        Value::Array(_) => v.clone(),
        Value::Object(m) => m.get("patch").cloned().unwrap_or(Value::Array(Vec::new())),
        _ => return Err("the patch must be an array of operations".into()),
    };
    serde_json::from_value::<Vec<UiPatchOp>>(list).map_err(|e| format!("invalid patch: {e}"))
}

/// Empty object value (helper for callers building patches).
#[must_use]
pub fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_prefs::{Density, PanelSide};
    use serde_json::json;

    fn ops(v: Value) -> Vec<UiPatchOp> {
        ops_from_json(&v).unwrap()
    }

    fn manager(base: &UiConfig, v: Value) -> Result<UiPatchOutcome, Vec<UiConfigIssue>> {
        apply_ui_patch(base, &ops(v), UiPatchActor::Manager, PatchMode::Strict)
    }

    #[test]
    fn pointer_roundtrip() {
        let segs = parse_pointer("/prefs/panels/a~1b~0c").unwrap();
        assert_eq!(segs, vec!["prefs", "panels", "a/b~c"]);
        assert_eq!(format_pointer(&segs), "/prefs/panels/a~1b~0c");
        assert!(parse_pointer("prefs").is_err());
        assert!(parse_pointer("/a~2").is_err());
    }

    #[test]
    fn mission_control_example_patch_applies_and_inverts() {
        let base = UiConfig::default();
        let out = manager(
            &base,
            json!([
                {"op": "replace", "path": "/prefs/density", "value": "Compact"},
                {"op": "add", "path": "/prefs/panels/missionControl.claude", "value": {"side": "Right", "hidden": false}},
                {"op": "add", "path": "/prefs/hiddenWidgets/-", "value": "missionControl.tokenChart"},
                {"op": "replace", "path": "/prefs/backgroundDim", "value": 0.5},
                {"op": "add", "path": "/theme/tokens/color-bg", "value": "#020308"}
            ]),
        )
        .unwrap();
        let prefs = out.config.prefs.clone().unwrap();
        assert_eq!(prefs.density, Density::Compact);
        assert_eq!(prefs.panels["missionControl.claude"].side, Some(PanelSide::Right));
        assert_eq!(prefs.hidden_widgets, vec!["missionControl.tokenChart".to_owned()]);
        assert_eq!(out.config.theme.tokens["color-bg"], "#020308");
        assert_eq!(out.changes.len(), 5, "{:?}", out.changes);
        // The inverse restores the base exactly.
        let back = apply_ui_patch(&out.config, &out.inverse, UiPatchActor::User, PatchMode::Strict).unwrap();
        assert_eq!(normalized_value(&back.config), normalized_value(&base));
    }

    #[test]
    fn rejects_unknown_paths_and_members() {
        let base = UiConfig::default();
        // Unknown member inside an allowed subtree.
        let e = manager(&base, json!([{"op": "add", "path": "/prefs/evil", "value": 1}])).unwrap_err();
        assert_eq!(e[0].path, "/prefs/evil");
        let e = manager(&base, json!([{"op": "add", "path": "/prefs/hud/script", "value": "x"}])).unwrap_err();
        assert_eq!(e[0].path, "/prefs/hud/script");
        // Outside the allowlist.
        for p in ["/schemaVersion", "/layout/docking", "/robots", "", "/theme"] {
            let e = manager(&base, json!([{"op": "add", "path": p, "value": 2}])).unwrap_err();
            assert!(e[0].message.contains("not editable") || e[0].message.contains("root"), "{p}: {e:?}");
        }
        // The user may not change the schema version either.
        let e = apply_ui_patch(
            &base,
            &ops(json!([{"op": "replace", "path": "/schemaVersion", "value": 2}])),
            UiPatchActor::User,
            PatchMode::Strict,
        )
        .unwrap_err();
        assert!(e[0].message.contains("not editable"));
        // Unsupported operations.
        assert!(ops_from_json(&json!([{"op": "move", "from": "/a", "path": "/b"}])).is_err());
    }

    #[test]
    fn rejects_code_injection() {
        let base = UiConfig::default();
        for bad in [
            json!("url(https://evil.example/x.png)"),
            json!("red; background: blue"),
            json!("<script>alert(1)</script>"),
            json!("javascript:alert(1)"),
            json!("expression(alert(1))"),
            json!("a{b:c}"),
        ] {
            let e = manager(&base, json!([{"op": "add", "path": "/theme/tokens/color-bg", "value": bad}])).unwrap_err();
            assert!(!e.is_empty());
        }
        // A hidden widget id carrying markup is rejected by the id grammar too.
        let e = manager(&base, json!([{"op": "add", "path": "/prefs/hiddenWidgets/-", "value": "a\" onload=\"x"}]));
        assert!(e.is_err());
        // Type errors are rejected.
        let e = manager(&base, json!([{"op": "replace", "path": "/prefs/density", "value": "Huge"}])).unwrap_err();
        assert!(e[0].message.contains("schema"));
        // Range errors are rejected.
        let e = manager(&base, json!([{"op": "replace", "path": "/prefs/backgroundDim", "value": 5}])).unwrap_err();
        assert_eq!(e[0].path, "prefs.backgroundDim");
    }

    #[test]
    fn strict_and_lenient_modes() {
        let base = UiConfig::default();
        let p = json!([{"op": "replace", "path": "/theme/tokens/color-accent", "value": "#ff00aa"}]);
        assert!(manager(&base, p.clone()).is_err(), "replace of a missing token fails strictly");
        let out = apply_ui_patch(&base, &ops(p), UiPatchActor::Manager, PatchMode::Lenient).unwrap();
        assert_eq!(out.config.theme.tokens["color-accent"], "#ff00aa");
        let p = json!([{"op": "remove", "path": "/prefs/panels/nope"}]);
        assert!(manager(&base, p.clone()).is_err());
        assert!(apply_ui_patch(&base, &ops(p), UiPatchActor::Manager, PatchMode::Lenient).unwrap().changes.is_empty());
        let p = json!([{"op": "test", "path": "/prefs/density", "value": "Compact"}]);
        assert!(manager(&base, p).is_err());
    }

    #[test]
    fn limits_and_dedupe() {
        let base = UiConfig::default();
        let many: Vec<Value> =
            (0..=MAX_PATCH_OPS).map(|_| json!({"op": "replace", "path": "/prefs/density", "value": "Compact"})).collect();
        assert!(manager(&base, Value::Array(many)).is_err());
        let out = manager(
            &base,
            json!([
                {"op": "add", "path": "/prefs/hiddenWidgets/-", "value": "a"},
                {"op": "add", "path": "/prefs/hiddenWidgets/-", "value": "a"}
            ]),
        )
        .unwrap();
        assert_eq!(out.config.prefs.unwrap().hidden_widgets, vec!["a".to_owned()]);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn no_op_patch_has_no_changes() {
        let base = UiConfig::default();
        let out = manager(&base, json!([{"op": "replace", "path": "/prefs/density", "value": "Comfortable"}])).unwrap();
        assert!(out.changes.is_empty());
        assert!(out.inverse.is_empty());
    }
}

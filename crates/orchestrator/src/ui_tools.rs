//! The Manager's UI tool (§29 "AI-editable UI"): `update_ui_config`.
//!
//! The Manager never writes UI code. To change how Orkestr looks or is laid
//! out it proposes a JSON Patch over the validated UI configuration schema
//! (`orkestr_protocol::ui_patch`). The tool shape-checks the patch with the
//! AI allowlist (against the default document, leniently — the user's real
//! document is checked again by the core on preview and apply), then
//! publishes `UiPatchProposed`. The UI shows a live preview with a diff
//! summary; nothing is persisted until the user keeps it (`ui.applyPatch`),
//! and every applied change can be undone.

use orkestr_protocol::event::{EventKind, UiPatchProposedPayload};
use orkestr_protocol::ui_config::UiConfig;
use orkestr_protocol::ui_patch::{apply_ui_patch, ops_from_json, PatchMode, UiPatchActor, UiPatchOp};
use orkestr_protocol::NewEvent;
use orkestr_providers::ToolSpec;
use serde_json::{json, Value};

use crate::tools::{ToolOutcome, TurnCtx};

/// Tool name.
pub const UPDATE_UI_CONFIG: &str = "update_ui_config";

/// Line appended to the Manager's system prompt.
pub const PROMPT_LINE: &str = "- To change how Orkestr itself looks or is arranged (density, panel placement, hidden widgets, background, theme tokens, performance mode), call update_ui_config with a validated patch of the UI configuration. Never write UI code; the user previews the change and keeps or reverts it.\n";

/// Tool spec offered to the Manager model.
#[must_use]
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: UPDATE_UI_CONFIG.into(),
        description: "Propose a change to Orkestr's own UI as an RFC 6902 JSON Patch over the validated UI configuration \
            (never code). Editable paths: /prefs/density (\"Comfortable\" | \"Compact\"), \
            /prefs/panels/<id> ({\"side\": \"Left\"|\"Right\"|\"Bottom\"|\"Center\", \"hidden\": bool}; ids like \"terminal\", \
            \"missionControl.claude\"), /prefs/hiddenWidgets/- (widget id such as \"missionControl.tokenChart\"), \
            /prefs/backgroundDim (0..0.9, higher is darker), /prefs/hud/opacity (0.4..1), /theme/tokens/<token> \
            (CSS color/length, e.g. color-bg, color-accent), /theme/effects/blurPx|glowIntensity|panelOpacity|animations, \
            /theme/mode (\"Dark\"|\"Light\"), /layout/preset (\"Coding\"|\"MissionControl\"|\"Monitoring\"|\"Focus\"), \
            /performanceMode (\"UltraVisual\"|\"Balanced\"|\"Performance\"|\"BatterySaver\"). The user sees a live preview and keeps or reverts it."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "One short sentence for the user describing the change"},
                "patch": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 32,
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {"type": "string", "enum": ["add", "remove", "replace"]},
                            "path": {"type": "string"},
                            "value": {}
                        },
                        "required": ["op", "path"]
                    }
                }
            },
            "required": ["summary", "patch"],
            "additionalProperties": false
        }),
    }
}

/// Parses and shape-checks the tool arguments. Returns the patch and summary.
pub fn check_args(args: &Value) -> Result<(Vec<UiPatchOp>, String), String> {
    let summary = args
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("`summary` is required")?
        .chars()
        .take(200)
        .collect::<String>();
    let patch = ops_from_json(args)?;
    if patch.is_empty() {
        return Err("the patch is empty".into());
    }
    apply_ui_patch(&UiConfig::default(), &patch, UiPatchActor::Manager, PatchMode::Lenient).map_err(|issues| {
        issues.iter().map(|i| format!("{}: {}", i.path, i.message)).collect::<Vec<_>>().join("; ")
    })?;
    Ok((patch, summary))
}

impl crate::service::Inner {
    /// Executes `update_ui_config`.
    pub(crate) async fn update_ui_config_tool(&self, ctx: &TurnCtx, args: &Value) -> ToolOutcome {
        if self.deps.security.emergency.flags().all_stopped {
            return ToolOutcome::err("Stop All AI is active");
        }
        let (patch, summary) = match check_args(args) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(format!("the UI patch was rejected: {e}")),
        };
        let proposal_id = uuid::Uuid::now_v7().to_string();
        let ops = patch.len();
        self.deps
            .publish(NewEvent::new(EventKind::UiPatchProposed(UiPatchProposedPayload {
                proposal_id: proposal_id.clone(),
                project_id: Some(ctx.project.id),
                author: UiPatchActor::Manager,
                summary: summary.clone(),
                patch,
            })))
            .await;
        ToolOutcome::ok(
            format!(
                "Proposed UI change {proposal_id} ({ops} operation(s)). The user now sees a live preview and keeps or reverts it; nothing is saved until they keep it. Do not propose the same change again — tell the user briefly what will change."
            ),
            format!("UI change proposed: {summary}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_an_object_schema() {
        let s = spec();
        assert_eq!(s.name, UPDATE_UI_CONFIG);
        assert_eq!(s.parameters["type"], "object");
    }

    #[test]
    fn checks_the_example_patch() {
        let args = json!({
            "summary": "Compact Mission Control, Claude on the right, no token chart, darker background",
            "patch": [
                {"op": "replace", "path": "/prefs/density", "value": "Compact"},
                {"op": "add", "path": "/prefs/panels/missionControl.claude", "value": {"side": "Right", "hidden": false}},
                {"op": "add", "path": "/prefs/hiddenWidgets/-", "value": "missionControl.tokenChart"},
                {"op": "replace", "path": "/prefs/backgroundDim", "value": 0.55},
                {"op": "replace", "path": "/theme/tokens/color-bg", "value": "#020308"}
            ]
        });
        let (patch, summary) = check_args(&args).unwrap();
        assert_eq!(patch.len(), 5);
        assert!(summary.starts_with("Compact"));
    }

    #[test]
    fn rejects_code_and_forbidden_paths() {
        for patch in [
            json!([{"op": "add", "path": "/theme/tokens/color-bg", "value": "url(https://x)"}]),
            json!([{"op": "add", "path": "/layout/docking", "value": {}}]),
            json!([{"op": "replace", "path": "/schemaVersion", "value": 9}]),
            json!([{"op": "add", "path": "/prefs/onClick", "value": "alert(1)"}]),
            json!([]),
        ] {
            assert!(check_args(&json!({"summary": "x", "patch": patch})).is_err());
        }
        assert!(check_args(&json!({"patch": [{"op": "replace", "path": "/prefs/density", "value": "Compact"}]})).is_err());
    }
}

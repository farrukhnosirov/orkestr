//! Lines Orkestr writes to the CLI's stdin (`--input-format stream-json`).
//!
//! Control protocol verified against Claude Code 2.1.292 (see the module docs
//! of [`super`]): Orkestr → CLI `control_request {request_id, request:
//! {subtype: initialize | interrupt | set_permission_mode | set_model}}`, the
//! CLI answers `control_response {response: {subtype: success | error,
//! request_id, response | error}}`. The CLI → Orkestr `can_use_tool` request
//! is answered with `control_response {subtype: success, response: {behavior:
//! allow, updatedInput, updatedPermissions?} | {behavior: deny, message}}`.

use serde_json::{json, Value};

/// A user turn.
#[must_use]
pub fn user_message(text: &str) -> String {
    user_message_with_images(text, &[])
}

/// A user turn with images (`(media_type, base64)`) before the text — the
/// Messages API content-block format the CLI accepts on stdin (verified with
/// 2.1.292: the model sees the image).
#[must_use]
pub fn user_message_with_images(text: &str, images: &[(String, String)]) -> String {
    let mut content: Vec<Value> = images
        .iter()
        .map(|(media_type, data)| {
            json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
        })
        .collect();
    content.push(json!({"type": "text", "text": text}));
    json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "parent_tool_use_id": null,
        "session_id": "",
    })
    .to_string()
}

/// `set_max_thinking_tokens` (`0` = extended thinking off).
#[must_use]
pub fn set_max_thinking_tokens(tokens: u32) -> Value {
    json!({"subtype": "set_max_thinking_tokens", "max_thinking_tokens": tokens})
}

/// A read-only query: `get_usage`, `get_context_usage` or `mcp_status`.
#[must_use]
pub fn query(subtype: &str) -> Value {
    json!({"subtype": subtype})
}

/// A control request from Orkestr.
#[must_use]
pub fn control_request(request_id: &str, request: Value) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": request}).to_string()
}

/// `initialize`.
#[must_use]
pub fn initialize() -> Value {
    json!({"subtype": "initialize"})
}

/// `interrupt`.
#[must_use]
pub fn interrupt() -> Value {
    json!({"subtype": "interrupt"})
}

/// `set_permission_mode`.
#[must_use]
pub fn set_permission_mode(mode: &str) -> Value {
    json!({"subtype": "set_permission_mode", "mode": mode})
}

/// `set_model` (`None` = the CLI's default model).
#[must_use]
pub fn set_model(model: Option<&str>) -> Value {
    json!({"subtype": "set_model", "model": model})
}

fn success(request_id: &str, response: Value) -> String {
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}})
        .to_string()
}

/// Error answer to a control request Orkestr does not support.
#[must_use]
pub fn error_response(request_id: &str, error: &str) -> String {
    json!({"type": "control_response", "response": {"subtype": "error", "request_id": request_id, "error": error}})
        .to_string()
}

/// Session-scoped permission updates from the CLI's suggestions: every
/// suggestion is re-targeted to `session`, so "allow for this session" never
/// writes the user's or the project's settings files. Without suggestions,
/// an allow rule for the whole tool is added for the session.
#[must_use]
pub fn session_permissions(tool_name: &str, suggestions: Option<&Value>) -> Vec<Value> {
    let mut out: Vec<Value> = suggestions
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.is_object())
        .map(|s| {
            let mut s = s.clone();
            s["destination"] = json!("session");
            s
        })
        .collect();
    if out.is_empty() {
        out.push(json!({
            "type": "addRules",
            "rules": [{"toolName": tool_name}],
            "behavior": "allow",
            "destination": "session",
        }));
    }
    out
}

/// Allows a tool call.
#[must_use]
pub fn allow(request_id: &str, input: &Value, updated_permissions: Option<Vec<Value>>) -> String {
    let mut body = json!({"behavior": "allow", "updatedInput": input});
    if let Some(p) = updated_permissions {
        body["updatedPermissions"] = Value::Array(p);
    }
    success(request_id, body)
}

/// Denies a tool call (`message` is shown to Claude as the tool result).
#[must_use]
pub fn deny(request_id: &str, message: &str) -> String {
    success(request_id, json!({"behavior": "deny", "message": message}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_match_the_verified_protocol() {
        let u: Value = serde_json::from_str(&user_message("hi\nthere")).unwrap();
        assert_eq!(u["type"], "user");
        assert_eq!(u["message"]["content"][0]["text"], "hi\nthere");
        assert!(!user_message("a\nb").contains('\n'), "one JSON line per message");
        let r: Value = serde_json::from_str(&control_request("r1", set_permission_mode("plan"))).unwrap();
        assert_eq!(r["request"]["subtype"], "set_permission_mode");
        assert_eq!(r["request"]["mode"], "plan");
        let a: Value = serde_json::from_str(&allow("q", &json!({"x": 1}), None)).unwrap();
        assert_eq!(a["response"]["request_id"], "q");
        assert_eq!(a["response"]["response"]["behavior"], "allow");
        assert_eq!(a["response"]["response"]["updatedInput"]["x"], 1);
        assert!(a["response"]["response"].get("updatedPermissions").is_none());
        let d: Value = serde_json::from_str(&deny("q", "no")).unwrap();
        assert_eq!(d["response"]["response"], json!({"behavior": "deny", "message": "no"}));
        let e: Value = serde_json::from_str(&error_response("q", "unsupported")).unwrap();
        assert_eq!(e["response"]["subtype"], "error");
    }

    #[test]
    fn session_permissions_never_write_settings_files() {
        let sugg = json!([
            {"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "mkdir x *"}], "behavior": "allow", "destination": "localSettings"},
            {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            "junk"
        ]);
        let p = session_permissions("Bash", Some(&sugg));
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|s| s["destination"] == "session"));
        let p = session_permissions("WebFetch", None);
        assert_eq!(p[0]["rules"][0]["toolName"], "WebFetch");
        assert_eq!(p[0]["destination"], "session");
    }
}

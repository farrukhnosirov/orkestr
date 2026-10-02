//! Argument validation for the JSON-schema subset the built-in tools use:
//! `type` (object/string/integer/number/boolean/array), `properties`,
//! `required`, `additionalProperties: false`, `enum`, `items`, `minimum`,
//! `maximum`, `maxLength`. Invalid arguments become a tool error the model
//! can correct (never a crash).

use serde_json::Value;

/// Validates `value` against `schema`; the error names the offending field.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(raw) = value.get("__invalid_json") {
        let raw = raw.as_str().unwrap_or_default();
        let shown: String = raw.chars().take(120).collect();
        return Err(format!("arguments are not valid JSON: {shown}"));
    }
    check(schema, value, "arguments")
}

fn type_ok(ty: &str, v: &Value) -> bool {
    match ty {
        "object" => v.is_object(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn check(schema: &Value, v: &Value, at: &str) -> Result<(), String> {
    if let Some(ty) = schema.get("type").and_then(Value::as_str) {
        if !type_ok(ty, v) {
            return Err(format!("`{at}` must be of type {ty}"));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(v) {
            let names: Vec<String> = options.iter().map(Value::to_string).collect();
            return Err(format!("`{at}` must be one of {}", names.join(", ")));
        }
    }
    if let Some(n) = v.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
            if n < min {
                return Err(format!("`{at}` must be ≥ {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
            if n > max {
                return Err(format!("`{at}` must be ≤ {max}"));
            }
        }
    }
    if let (Some(s), Some(max)) = (v.as_str(), schema.get("maxLength").and_then(Value::as_u64)) {
        if s.chars().count() as u64 > max {
            return Err(format!("`{at}` is longer than {max} characters"));
        }
    }
    if let Some(map) = v.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for r in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(r) {
                    return Err(format!("missing required field `{r}` in `{at}`"));
                }
            }
        }
        for (k, child) in map {
            match props.and_then(|p| p.get(k)) {
                Some(s) => check(s, child, k)?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("unknown field `{k}` in `{at}`"));
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), v.as_array()) {
        for (i, item) in arr.iter().enumerate() {
            check(items, item, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::descriptor;
    use serde_json::json;

    #[test]
    fn validates_tool_arguments() {
        let s = descriptor("read_file").unwrap().input_schema;
        assert!(validate(&s, &json!({"path": "a.rs"})).is_ok());
        assert!(validate(&s, &json!({"path": "a.rs", "limit": 10})).is_ok());
        assert!(validate(&s, &json!({})).unwrap_err().contains("missing required field `path`"));
        assert!(validate(&s, &json!({"path": 3})).unwrap_err().contains("type string"));
        assert!(validate(&s, &json!({"path": "a", "limit": 0})).unwrap_err().contains("≥ 1"));
        assert!(validate(&s, &json!({"path": "a", "limit": 5000})).unwrap_err().contains("≤ 2000"));
        assert!(validate(&s, &json!({"path": "a", "extra": 1})).unwrap_err().contains("unknown field `extra`"));
        assert!(validate(&s, &json!({"__invalid_json": "{oops"})).unwrap_err().contains("not valid JSON"));
        let r = descriptor("submit_result").unwrap().input_schema;
        assert!(validate(&r, &json!({"summary": "ok", "status": "done"})).is_ok());
        assert!(validate(&r, &json!({"summary": "ok", "status": "great"})).unwrap_err().contains("one of"));
        assert!(validate(&r, &json!({"summary": "ok", "tests_run": [{"passed": true}]}))
            .unwrap_err()
            .contains("command"));
        assert!(validate(&r, &json!({"summary": "x".repeat(4001)})).unwrap_err().contains("longer"));
    }
}

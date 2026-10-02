//! Small shared helpers.

use chrono::SecondsFormat;
use orkestr_domain::Timestamp;
use orkestr_storage::StorageError;
use serde::Serialize;

/// Fixed-width, lexicographically sortable timestamp.
pub(crate) fn ts_key(t: &Timestamp) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Serde name of a unit enum variant (`MemoryKind::Api` → `"Api"`).
pub(crate) fn enum_name<T: Serialize>(value: &T) -> Result<String, StorageError> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StorageError::Invalid(format!("expected a unit enum, got {other}"))),
    }
}

/// SQL `IN (...)` list of quoted enum names. Names come from our own enums
/// (`[A-Za-z]+`), validated here again before being inlined.
pub(crate) fn sql_in_list<T: Serialize>(values: &[T]) -> Result<String, StorageError> {
    let mut parts = Vec::with_capacity(values.len());
    for v in values {
        let name = enum_name(v)?;
        if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(StorageError::Invalid(format!("unexpected enum name {name}")));
        }
        parts.push(format!("'{name}'"));
    }
    Ok(format!("({})", parts.join(",")))
}

/// Clamps an optional limit into `1..=max` with a default.
pub(crate) fn clamp_limit(limit: Option<u32>, default: u32, max: u32) -> u32 {
    limit.unwrap_or(default).clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::MemoryKind;

    #[test]
    fn helpers() {
        assert_eq!(enum_name(&MemoryKind::KnownIssue).unwrap(), "KnownIssue");
        assert_eq!(sql_in_list(&[MemoryKind::Api, MemoryKind::Todo]).unwrap(), "('Api','Todo')");
        assert_eq!(clamp_limit(None, 50, 200), 50);
        assert_eq!(clamp_limit(Some(0), 50, 200), 1);
        assert_eq!(clamp_limit(Some(9999), 50, 200), 200);
        let t = chrono::DateTime::parse_from_rfc3339("2026-10-02T12:00:00Z").unwrap().with_timezone(&chrono::Utc);
        assert_eq!(ts_key(&t), "2026-10-02T12:00:00.000000Z");
    }
}

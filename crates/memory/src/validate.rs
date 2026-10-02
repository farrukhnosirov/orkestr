//! Input validation and normalization for memory items and decisions.

use orkestr_protocol::methods::memory::{MAX_BODY_BYTES, MAX_KEY_LEN, MAX_TAGS, MAX_TAG_LEN, MAX_TITLE_LEN};

use crate::{MemoryError, MemoryResult};

/// Max length of a `source` string.
pub const MAX_SOURCE_LEN: usize = 500;
/// Max size of long decision text fields (bytes).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Max alternatives per decision.
pub const MAX_ALTERNATIVES: usize = 20;
/// Max related files / tasks per decision.
pub const MAX_LINKS: usize = 200;
/// Max length of one related file path.
pub const MAX_PATH_LEN: usize = 1024;

fn invalid(msg: impl Into<String>) -> MemoryError {
    MemoryError::Invalid(msg.into())
}

/// Trims and checks a title.
pub fn title(value: &str) -> MemoryResult<String> {
    let t = value.trim();
    if t.is_empty() {
        return Err(invalid("title must not be empty"));
    }
    if t.chars().count() > MAX_TITLE_LEN {
        return Err(invalid(format!("title must be at most {MAX_TITLE_LEN} characters")));
    }
    Ok(t.to_owned())
}

/// Checks a markdown body.
pub fn body(value: &str) -> MemoryResult<String> {
    if value.len() > MAX_BODY_BYTES {
        return Err(invalid(format!(
            "content must be at most {} KiB; add larger documents as a knowledge source",
            MAX_BODY_BYTES / 1024
        )));
    }
    Ok(value.to_owned())
}

/// Normalizes tags: trimmed, de-duplicated case-insensitively, bounded.
pub fn tags(values: &[String]) -> MemoryResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for raw in values {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > MAX_TAG_LEN {
            return Err(invalid(format!("tags must be at most {MAX_TAG_LEN} characters")));
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            out.push(t.to_owned());
        }
    }
    if out.len() > MAX_TAGS {
        return Err(invalid(format!("at most {MAX_TAGS} tags")));
    }
    Ok(out)
}

/// Optional source string (`""` ⇒ `None`).
pub fn source(value: Option<&str>) -> MemoryResult<Option<String>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) if s.chars().count() > MAX_SOURCE_LEN => {
            Err(invalid(format!("source must be at most {MAX_SOURCE_LEN} characters")))
        }
        Some(s) => Ok(Some(s.to_owned())),
    }
}

/// Optional rule key (`""` ⇒ `None`).
pub fn key(value: Option<&str>) -> MemoryResult<Option<String>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(k) if k.chars().count() > MAX_KEY_LEN => {
            Err(invalid(format!("key must be at most {MAX_KEY_LEN} characters")))
        }
        Some(k) => Ok(Some(k.to_owned())),
    }
}

/// Normalized resolution key of a rule: explicit key, else `title:<title>`.
#[must_use]
pub fn rule_key(key: Option<&str>, title: &str) -> String {
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => k.to_lowercase(),
        None => format!("title:{}", title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()),
    }
}

/// Checks a long free-text field.
pub fn text(field: &str, value: &str) -> MemoryResult<String> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(invalid(format!("{field} must be at most {} KiB", MAX_TEXT_BYTES / 1024)));
    }
    Ok(value.trim().to_owned())
}

/// Checks related file paths.
pub fn files(values: &[String]) -> MemoryResult<Vec<String>> {
    if values.len() > MAX_LINKS {
        return Err(invalid(format!("at most {MAX_LINKS} related files")));
    }
    let mut out: Vec<String> = Vec::new();
    for f in values {
        let f = f.trim();
        if f.is_empty() {
            continue;
        }
        if f.len() > MAX_PATH_LEN {
            return Err(invalid(format!("file paths must be at most {MAX_PATH_LEN} bytes")));
        }
        if !out.iter().any(|x| x == f) {
            out.push(f.to_owned());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_bodies() {
        assert_eq!(title("  A  ").unwrap(), "A");
        assert!(title("   ").is_err());
        assert!(title(&"x".repeat(201)).is_err());
        assert!(body(&"x".repeat(MAX_BODY_BYTES + 1)).is_err());
        assert!(body("ok").is_ok());
    }

    #[test]
    fn tags_are_normalized() {
        let t = tags(&["Rust".into(), " rust ".into(), "".into(), "db".into()]).unwrap();
        assert_eq!(t, vec!["Rust", "db"]);
        assert!(tags(&["x".repeat(41)]).is_err());
        let many: Vec<String> = (0..33).map(|i| format!("t{i}")).collect();
        assert!(tags(&many).is_err());
    }

    #[test]
    fn keys() {
        assert_eq!(key(Some("  ")).unwrap(), None);
        assert_eq!(rule_key(Some(" Indent "), "whatever"), "indent");
        assert_eq!(rule_key(None, "Use  Tabs"), "title:use tabs");
        assert_eq!(source(Some("")).unwrap(), None);
        assert!(source(Some(&"s".repeat(501))).is_err());
    }

    #[test]
    fn file_lists() {
        assert_eq!(files(&["a.rs".into(), "a.rs".into(), " ".into()]).unwrap(), vec!["a.rs"]);
        assert!(files(&vec!["a".into(); 201]).is_err());
    }
}

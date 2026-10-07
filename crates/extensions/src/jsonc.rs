//! JSON with comments (JSONC), as used by VS Code themes, language
//! configurations, snippets and icon themes: `//` and `/* */` comments and
//! trailing commas are removed before parsing with `serde_json`.

use serde_json::Value;

use crate::error::{ExtError, ExtResult};

/// Removes comments and trailing commas (string contents are preserved).
#[must_use]
pub fn strip(input: &str) -> String {
    let src = input.strip_prefix('\u{feff}').unwrap_or(input);
    let bytes: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let n = bytes.len();
    while i < n {
        let c = bytes[i];
        if c == '"' {
            // Copy a string literal verbatim (handles escapes).
            out.push(c);
            i += 1;
            while i < n {
                let d = bytes[i];
                out.push(d);
                i += 1;
                if d == '\\' && i < n {
                    out.push(bytes[i]);
                    i += 1;
                } else if d == '"' {
                    break;
                }
            }
            continue;
        }
        if c == '/' && i + 1 < n && bytes[i + 1] == '/' {
            while i < n && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && bytes[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                // Keep line structure for error positions.
                if bytes[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        if c == ',' {
            // Trailing comma: next significant char closes an object/array.
            let mut j = i + 1;
            loop {
                while j < n && bytes[j].is_whitespace() {
                    j += 1;
                }
                if j + 1 < n && bytes[j] == '/' && bytes[j + 1] == '/' {
                    while j < n && bytes[j] != '\n' {
                        j += 1;
                    }
                    continue;
                }
                if j + 1 < n && bytes[j] == '/' && bytes[j + 1] == '*' {
                    j += 2;
                    while j + 1 < n && !(bytes[j] == '*' && bytes[j + 1] == '/') {
                        j += 1;
                    }
                    j = (j + 2).min(n);
                    continue;
                }
                break;
            }
            if j < n && (bytes[j] == '}' || bytes[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Parses JSONC into a JSON value.
pub fn parse(input: &str) -> ExtResult<Value> {
    serde_json::from_str(&strip(input)).map_err(|e| ExtError::Invalid(format!("invalid JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let src = "\u{feff}{\n // line\n \"a\": 1, /* block\n more */ \"b\": [1, 2,],\n \"c\": \"// not a comment, /* nor this */\",\n}";
        let v = parse(src).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"], "// not a comment, /* nor this */");
    }

    #[test]
    fn keeps_escaped_quotes() {
        let v = parse(r#"{"a": "x\"//y", "b": "\\"}"#).unwrap();
        assert_eq!(v["a"], "x\"//y");
        assert_eq!(v["b"], "\\");
    }

    #[test]
    fn trailing_comma_before_comment_and_brace() {
        let v = parse("[1, 2, // c\n]").unwrap();
        assert_eq!(v, serde_json::json!([1, 2]));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("{a:}").is_err());
    }
}

//! VS Code snippet files (`contributes.snippets`): JSONC objects of
//! `name → { prefix, body, description }` where each field may be a string
//! or an array of strings. Bodies stay in TextMate snippet syntax, which
//! Monaco's snippet engine understands.

use orkestr_protocol::methods::extensions::Snippet;
use serde_json::Value;

use crate::error::{ExtError, ExtResult};

/// Max snippets per file.
const MAX_SNIPPETS: usize = 5_000;

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => Vec::new(),
    }
}

/// Parses a snippets file.
pub fn parse(text: &str) -> ExtResult<Vec<Snippet>> {
    let v = crate::jsonc::parse(text)?;
    let obj = v.as_object().ok_or_else(|| ExtError::Invalid("a snippets file must be a JSON object".into()))?;
    let mut out = Vec::new();
    for (name, s) in obj {
        if out.len() >= MAX_SNIPPETS {
            break;
        }
        let Some(o) = s.as_object() else { continue };
        let body = strings(o.get("body"));
        if body.is_empty() {
            continue;
        }
        let prefixes: Vec<String> = strings(o.get("prefix")).into_iter().filter(|p| !p.trim().is_empty()).collect();
        if prefixes.is_empty() {
            continue;
        }
        let description = {
            let d = strings(o.get("description"));
            (!d.is_empty()).then(|| d.join("\n"))
        };
        out.push(Snippet { name: name.clone(), prefixes, body: body.join("\n"), description });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strings_and_arrays() {
        let s = parse(
            r#"{
            // comment
            "For Loop": { "prefix": ["for", "fori"], "body": ["for ${1:i} in ${2:iter} {", "\t$0", "}"], "description": "loop" },
            "Main": { "prefix": "main", "body": "fn main() {\n\t$0\n}" },
            "Broken": { "prefix": "x" },
            "NoPrefix": { "body": "x" },
        }"#,
        )
        .unwrap();
        assert_eq!(s.len(), 2);
        let f = s.iter().find(|x| x.name == "For Loop").unwrap();
        assert_eq!(f.prefixes, vec!["for", "fori"]);
        assert_eq!(f.body, "for ${1:i} in ${2:iter} {\n\t$0\n}");
        assert_eq!(f.description.as_deref(), Some("loop"));
        let m = s.iter().find(|x| x.name == "Main").unwrap();
        assert_eq!(m.description, None);
    }

    #[test]
    fn rejects_non_object() {
        assert!(parse("[]").is_err());
    }
}

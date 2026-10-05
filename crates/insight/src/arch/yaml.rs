//! A tiny, forgiving YAML subset reader for docker-compose files and OpenAPI
//! documents: block maps, block lists (including lists of maps), flow lists
//! / maps of scalars, quoted scalars, comments and block scalars (skipped as
//! text). Anchors, tags and multi-documents are not interpreted. It never
//! fails: malformed input yields whatever structure could be recognised.

/// A parsed YAML value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Yaml {
    /// Scalar (always text; quotes removed).
    Scalar(String),
    /// Mapping in source order.
    Map(Vec<(String, Yaml)>),
    /// Sequence.
    List(Vec<Yaml>),
}

impl Yaml {
    /// Value of a key (maps only).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Scalar text.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Scalar(s) => Some(s),
            _ => None,
        }
    }

    /// Map entries (empty for non-maps).
    #[must_use]
    pub fn entries(&self) -> &[(String, Yaml)] {
        match self {
            Yaml::Map(m) => m,
            _ => &[],
        }
    }

    /// List items (empty for non-lists).
    #[must_use]
    pub fn items(&self) -> &[Yaml] {
        match self {
            Yaml::List(l) => l,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone)]
struct Line {
    indent: usize,
    text: String,
}

/// Parses a document.
#[must_use]
pub fn parse(src: &str) -> Yaml {
    let mut lines: Vec<Line> = Vec::new();
    for raw in src.lines() {
        let raw = raw.trim_end_matches('\r');
        let stripped = strip_comment(raw);
        if stripped.trim().is_empty() {
            // Keep blank lines out; block scalars lose blank lines (text is ignored anyway).
            continue;
        }
        let t = stripped.trim_start();
        if t == "---" || t == "..." || t.starts_with("%YAML") {
            continue;
        }
        let indent = stripped.len() - t.len();
        lines.push(Line { indent, text: t.trim_end().to_owned() });
    }
    let mut i = 0;
    if lines.is_empty() {
        return Yaml::Map(Vec::new());
    }
    let indent = lines[0].indent;
    parse_block(&mut lines, &mut i, indent)
}

fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && (idx == 0 || bytes[idx - 1].is_ascii_whitespace()) => {
                return &line[..idx];
            }
            _ => {}
        }
    }
    line
}

fn is_list_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

fn parse_block(lines: &mut [Line], i: &mut usize, indent: usize) -> Yaml {
    if *i >= lines.len() {
        return Yaml::Scalar(String::new());
    }
    if is_list_item(&lines[*i].text) {
        parse_list(lines, i, indent)
    } else {
        parse_map(lines, i, indent)
    }
}

fn parse_list(lines: &mut [Line], i: &mut usize, indent: usize) -> Yaml {
    let mut items = Vec::new();
    while *i < lines.len() && lines[*i].indent == indent && is_list_item(&lines[*i].text) {
        let rest = lines[*i].text.get(1..).unwrap_or("").trim_start().to_owned();
        if rest.is_empty() {
            *i += 1;
            if *i < lines.len() && lines[*i].indent > indent {
                let child_indent = lines[*i].indent;
                items.push(parse_block(lines, i, child_indent));
            } else {
                items.push(Yaml::Scalar(String::new()));
            }
            continue;
        }
        if split_key(&rest).is_some() && !rest.starts_with('{') && !rest.starts_with('[') {
            // `- key: value` → a map whose first entry sits after the dash.
            let offset = lines[*i].text.len() - rest.len();
            lines[*i] = Line { indent: indent + offset, text: rest };
            let child_indent = indent + offset;
            items.push(parse_map(lines, i, child_indent));
            continue;
        }
        items.push(scalar_or_flow(&rest));
        *i += 1;
        // Continuation lines of a plain multi-line scalar are skipped.
        while *i < lines.len() && lines[*i].indent > indent && !is_list_item(&lines[*i].text) {
            *i += 1;
        }
    }
    Yaml::List(items)
}

fn parse_map(lines: &mut [Line], i: &mut usize, indent: usize) -> Yaml {
    let mut entries: Vec<(String, Yaml)> = Vec::new();
    while *i < lines.len() {
        let line_indent = lines[*i].indent;
        if line_indent < indent {
            break;
        }
        if line_indent > indent {
            // Malformed (over-indented) line: skip it.
            *i += 1;
            continue;
        }
        if is_list_item(&lines[*i].text) {
            break;
        }
        let text = lines[*i].text.clone();
        let Some((key, value)) = split_key(&text) else {
            *i += 1;
            continue;
        };
        *i += 1;
        let value = value.trim();
        let node = if value.is_empty() {
            if *i < lines.len() && lines[*i].indent > indent {
                let child_indent = lines[*i].indent;
                parse_block(lines, i, child_indent)
            } else if *i < lines.len() && lines[*i].indent == indent && is_list_item(&lines[*i].text) {
                parse_list(lines, i, indent)
            } else {
                Yaml::Scalar(String::new())
            }
        } else if value.starts_with('|') || value.starts_with('>') {
            let mut text = Vec::new();
            while *i < lines.len() && lines[*i].indent > indent {
                text.push(lines[*i].text.clone());
                *i += 1;
            }
            Yaml::Scalar(text.join("\n"))
        } else {
            let v = scalar_or_flow(value);
            while *i < lines.len() && lines[*i].indent > indent {
                *i += 1;
            }
            v
        };
        entries.push((unquote(key), node));
    }
    Yaml::Map(entries)
}

/// Splits `key: value` (colon followed by space or end, outside quotes).
fn split_key(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut depth = 0i32;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'[' | b'{' if !in_single && !in_double => depth += 1,
            b']' | b'}' if !in_single && !in_double => depth -= 1,
            b':' if !in_single && !in_double && depth == 0 => {
                let next = bytes.get(idx + 1);
                if next.is_none() || next == Some(&b' ') || next == Some(&b'\t') {
                    let key = text[..idx].trim();
                    if key.is_empty() {
                        return None;
                    }
                    return Some((key, &text[idx + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn scalar_or_flow(value: &str) -> Yaml {
    let v = value.trim();
    if v.starts_with('[') && v.ends_with(']') && v.len() >= 2 {
        let inner = &v[1..v.len() - 1];
        return Yaml::List(
            split_flow(inner).into_iter().filter(|s| !s.is_empty()).map(|s| scalar_or_flow(&s)).collect(),
        );
    }
    if v.starts_with('{') && v.ends_with('}') && v.len() >= 2 {
        let inner = &v[1..v.len() - 1];
        let mut entries = Vec::new();
        for part in split_flow(inner) {
            if let Some((k, val)) = part.split_once(':') {
                entries.push((unquote(k.trim()), scalar_or_flow(val)));
            }
        }
        return Yaml::Map(entries);
    }
    Yaml::Scalar(unquote(v))
}

fn split_flow(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    for c in inner.chars() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '[' | '{' if !in_single && !in_double => depth += 1,
            ']' | '}' if !in_single && !in_double => depth -= 1,
            ',' if depth == 0 && !in_single && !in_double => {
                out.push(cur.trim().to_owned());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// Removes surrounding quotes.
#[must_use]
pub fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) {
        s[1..s.len() - 1].to_owned()
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSE: &str = r#"
version: "3.9"
services:
  api:
    build:
      context: ./api   # comment
    ports:
      - "8080:8080"
    depends_on:
      - db
      - cache
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_PASSWORD: example
  cache:
    image: 'redis:7'
    depends_on: [db]
  worker:
    build: ./worker
    command: >
      run --flag
      --other
    depends_on:
      db:
        condition: service_healthy
"#;

    #[test]
    fn parses_compose() {
        let y = parse(COMPOSE);
        let services = y.get("services").unwrap();
        let names: Vec<&str> = services.entries().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, vec!["api", "db", "cache", "worker"]);
        let api = services.get("api").unwrap();
        assert_eq!(api.get("build").unwrap().get("context").unwrap().as_str(), Some("./api"));
        let deps: Vec<&str> = api.get("depends_on").unwrap().items().iter().filter_map(Yaml::as_str).collect();
        assert_eq!(deps, vec!["db", "cache"]);
        assert_eq!(services.get("db").unwrap().get("image").unwrap().as_str(), Some("postgres:16-alpine"));
        assert_eq!(services.get("cache").unwrap().get("image").unwrap().as_str(), Some("redis:7"));
        assert_eq!(services.get("cache").unwrap().get("depends_on").unwrap().items().len(), 1);
        let worker = services.get("worker").unwrap();
        assert_eq!(worker.get("build").unwrap().as_str(), Some("./worker"));
        assert_eq!(worker.get("depends_on").unwrap().entries()[0].0, "db");
    }

    #[test]
    fn lists_of_maps_and_same_indent_lists() {
        let y = parse("paths:\n  /users:\n    get:\n      summary: x\n    post:\n      summary: y\ntags:\n- name: a\n  description: b\n- name: c\n");
        let users = y.get("paths").unwrap().get("/users").unwrap();
        assert_eq!(users.entries().len(), 2);
        let tags = y.get("tags").unwrap().items();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].get("description").unwrap().as_str(), Some("b"));
        assert_eq!(tags[1].get("name").unwrap().as_str(), Some("c"));
    }

    #[test]
    fn garbage_does_not_panic() {
        let _ = parse("::\n - - -\n  : x\n\t\tfoo: [a, {b: c}\n'unterminated: \"x");
        let _ = parse("");
    }
}

//! Stack-trace parsing (Rust panics + backtraces, JS/TS stacks, Python
//! tracebacks, and generic `path.ext:line[:col]` references such as Go and
//! Java frames) and confined resolution of `file:line` references to project
//! files for code-snippet evidence.

use std::path::{Component, Path, PathBuf};

/// Source language of a frame (from the file extension / trace syntax).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Rust.
    Rust,
    /// JavaScript / TypeScript.
    JavaScript,
    /// Python.
    Python,
    /// Go.
    Go,
    /// Java / Kotlin.
    Java,
    /// Anything else with a recognised source extension.
    Other,
}

/// One `file:line` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Path as printed (prefixes like `file://` stripped).
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: Option<u32>,
    /// Function, when the syntax names it.
    pub function: Option<String>,
    /// Language.
    pub lang: Lang,
}

impl Frame {
    /// `file:line`.
    #[must_use]
    pub fn reference(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

const SOURCE_EXTS: &[(&str, Lang)] = &[
    ("rs", Lang::Rust),
    ("js", Lang::JavaScript),
    ("mjs", Lang::JavaScript),
    ("cjs", Lang::JavaScript),
    ("jsx", Lang::JavaScript),
    ("ts", Lang::JavaScript),
    ("tsx", Lang::JavaScript),
    ("mts", Lang::JavaScript),
    ("cts", Lang::JavaScript),
    ("vue", Lang::JavaScript),
    ("svelte", Lang::JavaScript),
    ("py", Lang::Python),
    ("go", Lang::Go),
    ("java", Lang::Java),
    ("kt", Lang::Java),
    ("rb", Lang::Other),
    ("php", Lang::Other),
    ("cs", Lang::Other),
    ("c", Lang::Other),
    ("cc", Lang::Other),
    ("cpp", Lang::Other),
    ("h", Lang::Other),
    ("hpp", Lang::Other),
    ("swift", Lang::Other),
];

fn lang_of(path: &str) -> Option<Lang> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    SOURCE_EXTS.iter().find(|(e, _)| *e == ext).map(|(_, l)| *l)
}

fn clean_path(p: &str) -> String {
    let mut s = p.trim();
    for prefix in ["webpack:///", "webpack://", "file:///", "file://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            // `file:///C:/x` → `C:/x`; `file:///home/x` → `/home/x`.
            if prefix.starts_with("file") && !(s.len() > 1 && s.as_bytes()[1] == b':') {
                return format!("/{}", s.trim_start_matches('/'));
            }
            break;
        }
    }
    s.to_owned()
}

/// Parses `path.ext:line[:col]` (Windows drive letters allowed).
fn parse_ref(token: &str) -> Option<(String, u32, Option<u32>)> {
    let t = token.trim_end_matches([':', ',', '.', ';']);
    let mut nums: Vec<u32> = Vec::new();
    let mut rest = t;
    for _ in 0..2 {
        match rest.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => {
                nums.push(tail.parse().ok()?);
                rest = head;
            }
            _ => break,
        }
    }
    let (line, col) = match nums.as_slice() {
        [l] => (*l, None),
        [c, l] => (*l, Some(*c)),
        _ => return None,
    };
    let path = clean_path(rest);
    if line == 0 || path.is_empty() || lang_of(&path).is_none() {
        return None;
    }
    Some((path, line, col))
}

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '\'' | '"' | '<' | '>' | '`' | '{' | '}' | '|')
}

fn python_frame(line: &str) -> Option<Frame> {
    let start = line.find("File \"")? + 6;
    let rest = &line[start..];
    let end = rest.find('"')?;
    let file = clean_path(&rest[..end]);
    let after = &rest[end + 1..];
    let ln = after.split_once("line ")?.1;
    let digits: String = ln.chars().take_while(char::is_ascii_digit).collect();
    let line_no: u32 = digits.parse().ok()?;
    let function = ln.split_once(", in ").map(|(_, f)| f.trim().to_owned()).filter(|f| !f.is_empty());
    Some(Frame { file, line: line_no, column: None, function, lang: Lang::Python })
}

/// JS: `at fn (path:1:2)` / `at async fn (path:1:2)` → `fn`.
fn js_function(line: &str) -> Option<String> {
    let at = line.trim_start().strip_prefix("at ")?;
    let paren = at.find(" (")?;
    let name = at[..paren].trim().trim_start_matches("async ").trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Rust backtrace: `  12: crate::module::func` → `crate::module::func`.
fn rust_backtrace_function(line: &str) -> Option<String> {
    let (idx, name) = line.trim_start().split_once(": ")?;
    (!idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit()) && !name.trim().is_empty())
        .then(|| name.trim().to_owned())
}

/// Parses every frame of `text` (deduplicated by `file:line`, in order,
/// at most `max`).
#[must_use]
pub fn parse(text: &str, max: usize) -> Vec<Frame> {
    let mut out: Vec<Frame> = Vec::new();
    let push = |f: Frame, out: &mut Vec<Frame>| {
        if out.len() < max && !out.iter().any(|o| o.file == f.file && o.line == f.line) {
            out.push(f);
        }
    };
    let mut prev_fn: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(f) = python_frame(line) {
            push(f, &mut out);
            prev_fn = None;
            continue;
        }
        let js_fn = js_function(line);
        let mut found = false;
        for token in line.split(is_delim).filter(|t| t.contains(':')) {
            if let Some((file, ln, col)) = parse_ref(token) {
                let lang = lang_of(&file).unwrap_or(Lang::Other);
                let function = match lang {
                    Lang::JavaScript => js_fn.clone(),
                    Lang::Rust => prev_fn.clone(),
                    _ => None,
                };
                push(Frame { file, line: ln, column: col, function, lang }, &mut out);
                found = true;
            }
        }
        prev_fn = if found { None } else { rust_backtrace_function(line) };
    }
    out
}

/// True for frames that point into toolchains / dependencies.
#[must_use]
pub fn is_external(file: &str) -> bool {
    let f = file.replace('\\', "/");
    [
        "/rustc/",
        "/.cargo/registry/",
        "/.rustup/",
        "node_modules/",
        "node:internal",
        "site-packages/",
        "/lib/python",
        "<frozen",
    ]
    .iter()
    .any(|m| f.contains(m))
}

fn normalized(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_owned();
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

fn safe_rel(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        Some(p.to_path_buf())
    } else {
        None
    }
}

/// Resolves a frame path to a file inside `root` (relative path returned).
/// Absolute paths are relativized when under `root`; otherwise the longest
/// existing path suffix wins (traces from a build machine / container).
/// Never escapes `root` (`..` and symlinks out are refused).
#[must_use]
pub fn resolve(root: &Path, file: &str) -> Option<PathBuf> {
    if is_external(file) {
        return None;
    }
    let canon_root = std::fs::canonicalize(root).ok()?;
    let croot = normalized(&canon_root);
    let check = |rel: PathBuf| -> Option<PathBuf> {
        let full = std::fs::canonicalize(canon_root.join(&rel)).ok()?;
        (full.is_file() && normalized(&full).starts_with(&format!("{croot}/"))).then_some(rel)
    };
    let unified = file.replace('\\', "/");
    let trimmed = unified.trim_start_matches("./");
    // Under the root (absolute).
    for r in [normalized(root), croot.clone()] {
        let cand = if cfg!(windows) { trimmed.to_ascii_lowercase() } else { trimmed.to_owned() };
        if let Some(rest) = cand.strip_prefix(&format!("{r}/")) {
            // Keep the original case of the remainder.
            let rel = &trimmed[trimmed.len() - rest.len()..];
            if let Some(hit) = safe_rel(rel).and_then(check) {
                return Some(hit);
            }
        }
    }
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    if parts.contains(&"..") {
        return None;
    }
    let start = usize::from(parts.first().is_some_and(|p| p.ends_with(':')));
    for k in start..parts.len() {
        let rel = parts[k..].join("/");
        if let Some(hit) = safe_rel(&rel).and_then(check) {
            return Some(hit);
        }
    }
    None
}

/// Numbered snippet of `rel` around `line` (`radius` lines each side; the
/// target line is marked `>`), capped at `max_bytes`.
#[must_use]
pub fn snippet(root: &Path, rel: &Path, line: u32, radius: u32, max_bytes: usize) -> Option<String> {
    let full = root.join(rel);
    let meta = std::fs::metadata(&full).ok()?;
    if meta.len() > 4 * 1024 * 1024 {
        return None;
    }
    let text = std::fs::read_to_string(&full).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if line == 0 || line as usize > lines.len() {
        return None;
    }
    let from = line.saturating_sub(radius).max(1);
    let to = (line + radius).min(u32::try_from(lines.len()).unwrap_or(u32::MAX));
    let mut out = String::new();
    for n in from..=to {
        let mark = if n == line { '>' } else { ' ' };
        out.push_str(&format!("{mark}{n:>5} | {}\n", lines[(n - 1) as usize]));
        if out.len() > max_bytes {
            break;
        }
    }
    Some(crate::collectors::cap(&out, max_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_panic_new_and_old_format_and_backtrace() {
        let t = "thread 'main' panicked at src/handlers/user.rs:42:17:\ncalled `Option::unwrap()` on a `None` value\nstack backtrace:\n   0: rust_begin_unwind\n             at /rustc/abc/library/std/src/panicking.rs:665:5\n   3: demo::handlers::user::load\n             at ./src/handlers/user.rs:42:17\n   4: demo::main\n             at ./src/main.rs:7:5\n";
        let f = parse(t, 10);
        assert_eq!(
            f[0],
            Frame { file: "src/handlers/user.rs".into(), line: 42, column: Some(17), function: None, lang: Lang::Rust }
        );
        assert!(f.iter().any(|x| x.file.contains("panicking.rs")));
        let main = f.iter().find(|x| x.file == "./src/main.rs").unwrap();
        assert_eq!(main.function.as_deref(), Some("demo::main"));
        assert_eq!(main.line, 7);
        let old = parse("thread 'main' panicked at 'boom', src/lib.rs:3:5", 5);
        assert_eq!((old[0].file.as_str(), old[0].line), ("src/lib.rs", 3));
        assert!(is_external("/rustc/abc/library/std/src/panicking.rs"));
    }

    #[test]
    fn js_ts_stack() {
        let t = "TypeError: Cannot read properties of undefined (reading 'id')\n    at getUser (/app/src/users.ts:17:22)\n    at async Server.handle (file:///app/src/server.mjs:40:5)\n    at /app/node_modules/express/lib/router.js:10:1\n    at C:\\work\\app\\src\\index.js:3:9";
        let f = parse(t, 10);
        assert_eq!(f[0].file, "/app/src/users.ts");
        assert_eq!((f[0].line, f[0].column), (17, Some(22)));
        assert_eq!(f[0].function.as_deref(), Some("getUser"));
        assert_eq!(f[1].file, "/app/src/server.mjs");
        assert_eq!(f[1].function.as_deref(), Some("Server.handle"));
        assert!(is_external(&f[2].file));
        assert_eq!(f[3].file, "C:\\work\\app\\src\\index.js");
        assert_eq!(f[3].lang, Lang::JavaScript);
    }

    #[test]
    fn python_traceback() {
        let t = "Traceback (most recent call last):\n  File \"/srv/app/main.py\", line 12, in <module>\n    run()\n  File \"/srv/app/svc/orders.py\", line 88, in total\n    return sum(x.price for x in items) / count\nZeroDivisionError: division by zero";
        let f = parse(t, 10);
        assert_eq!(f.len(), 2);
        assert_eq!(
            f[1],
            Frame {
                file: "/srv/app/svc/orders.py".into(),
                line: 88,
                column: None,
                function: Some("total".into()),
                lang: Lang::Python
            }
        );
    }

    #[test]
    fn go_and_java_and_noise() {
        let f = parse("goroutine 1 [running]:\nmain.handler(...)\n\t/src/app/main.go:21 +0x1d\nat com.acme.Foo.bar(Foo.java:42)\nhttp://localhost:8080/x 12:30:01 ratio 3:4", 10);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].file.as_str(), f[0].line, f[0].lang), ("/src/app/main.go", 21, Lang::Go));
        assert_eq!((f[1].file.as_str(), f[1].line), ("Foo.java", 42));
        assert!(parse("no frames here: 10:20", 5).is_empty());
        assert_eq!(parse(&"src/a.rs:1:1 ".repeat(5), 10).len(), 1, "deduplicated");
    }

    #[test]
    fn resolves_inside_root_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src/handlers")).unwrap();
        std::fs::write(root.join("src/handlers/user.rs"), "a\nb\nc\nd\ne\n").unwrap();
        std::fs::write(dir.path().join("secret.rs"), "x").unwrap();
        assert_eq!(resolve(&root, "./src/handlers/user.rs").unwrap(), PathBuf::from("src/handlers/user.rs"));
        assert_eq!(resolve(&root, "/build/ci/src/handlers/user.rs").unwrap(), PathBuf::from("src/handlers/user.rs"));
        let abs = root.join("src/handlers/user.rs");
        assert_eq!(resolve(&root, &abs.to_string_lossy()).unwrap(), PathBuf::from("src/handlers/user.rs"));
        assert!(resolve(&root, "../secret.rs").is_none());
        assert!(resolve(&root, &dir.path().join("secret.rs").to_string_lossy()).is_none());
        assert!(resolve(&root, "/rustc/x/src/handlers/user.rs").is_none());
        let s = snippet(&root, Path::new("src/handlers/user.rs"), 3, 1, 1000).unwrap();
        assert_eq!(s, "     2 | b\n>    3 | c\n     4 | d\n");
        assert!(snippet(&root, Path::new("src/handlers/user.rs"), 99, 1, 1000).is_none());
    }
}

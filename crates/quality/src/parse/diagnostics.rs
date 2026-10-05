//! Compiler / linter diagnostics.

use once_cell::sync::Lazy;
use orkestr_protocol::methods::quality::{Diagnostic, DiagnosticSeverity};
use regex::Regex;

use super::{clip, norm_path};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

// rustc / clippy: `error[E0308]: msg` + ` --> file:line:col`
static RUST_HEAD: Lazy<Regex> = Lazy::new(|| re(r"^(error|warning)(?:\[([\w:]+)\])?: (.+)$"));
static RUST_LOC: Lazy<Regex> = Lazy::new(|| re(r"^\s*--> (.+?):(\d+):(\d+)$"));
// tsc / MSBuild: `file(10,5): error TS2322: msg [proj]`
static PAREN: Lazy<Regex> =
    Lazy::new(|| re(r"^\s*(.+?)\((\d+),(\d+)\): (error|warning) ([A-Za-z]+\d+): (.+?)(?: \[[^\]]+\])?$"));
// tsc --pretty: `file:10:5 - error TS2322: msg`
static TSC_PRETTY: Lazy<Regex> = Lazy::new(|| re(r"^(.+?):(\d+):(\d+) - (error|warning) (TS\d+): (.+)$"));
// ESLint stylish: `  10:5  error  msg  rule`
static ESLINT_ROW: Lazy<Regex> =
    Lazy::new(|| re(r"^\s+(\d+):(\d+)\s+(error|warning)\s+(.+?)(?:\s{2,}(\S+))?\s*$"));
// javac via Maven: `[ERROR] /p/File.java:[10,5] msg`
static JAVAC: Lazy<Regex> = Lazy::new(|| re(r"^\[(ERROR|WARNING)\] (.+?\.\w+):\[(\d+),(\d+)\] (.+)$"));
// Generic: `path.ext:line[:col]: [severity:] msg`
static GENERIC: Lazy<Regex> = Lazy::new(|| {
    re(r"^([^\s:][^:]*?\.[A-Za-z0-9]+|[A-Za-z]:[\\/][^:]*?\.[A-Za-z0-9]+):(\d+):(?:(\d+):)?\s+(?:(error|warning|note|info)\s*:?\s+)?(.+)$")
});
static TRAILING_CODE: Lazy<Regex> = Lazy::new(|| re(r"^(.*?)\s+\[([\w./-]+)\]$"));
static LEADING_CODE: Lazy<Regex> = Lazy::new(|| re(r"^([A-Z]{1,4}\d{2,5})\s+(.+)$"));

fn sev(s: &str) -> DiagnosticSeverity {
    match s.to_ascii_lowercase().as_str() {
        "warning" => DiagnosticSeverity::Warning,
        "note" | "info" => DiagnosticSeverity::Info,
        _ => DiagnosticSeverity::Error,
    }
}

fn is_rust_noise(msg: &str) -> bool {
    msg.starts_with("aborting due to")
        || msg.starts_with("could not compile")
        || msg.contains("warnings emitted")
        || msg.contains("warning emitted")
        || msg.contains(") generated ")
        || msg.starts_with("build failed")
        || msg.starts_with("unused manifest key")
        || msg.starts_with("test failed")
        || msg.starts_with("process didn't exit successfully")
}

/// Parses diagnostics. `allow_generic` enables the catch-all
/// `file:line[:col]: msg` form.
#[must_use]
pub fn parse_diagnostics(text: &str, allow_generic: bool) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Diagnostic> = Vec::new();
    let push = |d: Diagnostic, out: &mut Vec<Diagnostic>| {
        if !out.iter().any(|x| x.file == d.file && x.line == d.line && x.column == d.column && x.message == d.message) {
            out.push(d);
        }
    };
    let mut eslint_file: Option<String> = None;
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim_end();
        if let Some(c) = RUST_HEAD.captures(l) {
            let message = c[3].trim().to_owned();
            if is_rust_noise(&message) {
                continue;
            }
            let loc = lines.iter().skip(i + 1).take(3).find_map(|n| RUST_LOC.captures(n));
            push(
                Diagnostic {
                    severity: sev(&c[1]),
                    message: clip(&message),
                    file: loc.as_ref().map(|m| norm_path(&m[1])),
                    line: loc.as_ref().and_then(|m| m[2].parse().ok()),
                    column: loc.as_ref().and_then(|m| m[3].parse().ok()),
                    code: c.get(2).map(|m| m.as_str().to_owned()),
                },
                &mut out,
            );
            continue;
        }
        if let Some(c) = PAREN.captures(l).or_else(|| TSC_PRETTY.captures(l)) {
            push(
                Diagnostic {
                    severity: sev(&c[4]),
                    message: clip(&c[6]),
                    file: Some(norm_path(&c[1])),
                    line: c[2].parse().ok(),
                    column: c[3].parse().ok(),
                    code: Some(c[5].to_owned()),
                },
                &mut out,
            );
            continue;
        }
        if let Some(c) = JAVAC.captures(l) {
            push(
                Diagnostic {
                    severity: sev(&c[1]),
                    message: clip(&c[5]),
                    file: Some(norm_path(&c[2])),
                    line: c[3].parse().ok(),
                    column: c[4].parse().ok(),
                    code: None,
                },
                &mut out,
            );
            continue;
        }
        if let Some(c) = ESLINT_ROW.captures(l) {
            if let Some(file) = &eslint_file {
                push(
                    Diagnostic {
                        severity: sev(&c[3]),
                        message: clip(&c[4]),
                        file: Some(file.clone()),
                        line: c[1].parse().ok(),
                        column: c[2].parse().ok(),
                        code: c.get(5).map(|m| m.as_str().to_owned()),
                    },
                    &mut out,
                );
                continue;
            }
        }
        // ESLint stylish file header: an unindented path line followed by rows.
        if !l.is_empty()
            && !l.starts_with(char::is_whitespace)
            && !l.contains(": ")
            && l.contains('.')
            && lines.get(i + 1).is_some_and(|n| ESLINT_ROW.is_match(n))
        {
            eslint_file = Some(norm_path(l));
            continue;
        }
        if !l.starts_with(char::is_whitespace) {
            eslint_file = None;
        }
        if allow_generic {
            if let Some(c) = GENERIC.captures(l) {
                let mut message = c[5].trim().to_owned();
                let mut code = None;
                if let Some(t) = TRAILING_CODE.captures(&message) {
                    code = Some(t[2].to_owned());
                    message = t[1].trim().to_owned();
                } else if let Some(t) = LEADING_CODE.captures(&message) {
                    code = Some(t[1].to_owned());
                    message = t[2].trim().to_owned();
                }
                if message.is_empty() {
                    continue;
                }
                push(
                    Diagnostic {
                        severity: c.get(4).map_or(DiagnosticSeverity::Error, |m| sev(m.as_str())),
                        message: clip(&message),
                        file: Some(norm_path(&c[1])),
                        line: c[2].parse().ok(),
                        column: c.get(3).and_then(|m| m.as_str().parse().ok()),
                        code,
                    },
                    &mut out,
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustc_and_clippy() {
        let out = "   Compiling x v0.1.0
error[E0308]: mismatched types
  --> src/main.rs:4:18
   |
4  |     let x: i32 = \"a\";
warning: unused variable: `y`
 --> src\\lib.rs:2:9
error: this `if` has identical blocks
   --> crates/a/src/b.rs:10:5
error: aborting due to 2 previous errors
warning: `x` (lib) generated 1 warning
error: could not compile `x` (bin \"x\") due to 2 previous errors
";
        let d = parse_diagnostics(out, true);
        assert_eq!(d.len(), 3, "{d:#?}");
        assert_eq!(d[0].code.as_deref(), Some("E0308"));
        assert_eq!(d[0].file.as_deref(), Some("src/main.rs"));
        assert_eq!((d[0].line, d[0].column), (Some(4), Some(18)));
        assert_eq!(d[1].severity, DiagnosticSeverity::Warning);
        assert_eq!(d[1].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(d[2].file.as_deref(), Some("crates/a/src/b.rs"));
    }

    #[test]
    fn tsc_both_formats_and_msbuild() {
        let out = "src/a.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/b.tsx:3:1 - error TS1005: ';' expected.
C:\\p\\Program.cs(7,13): error CS1002: ; expected [C:\\p\\App.csproj]
";
        let d = parse_diagnostics(out, true);
        assert_eq!(d.len(), 3, "{d:#?}");
        assert_eq!(d[0].code.as_deref(), Some("TS2322"));
        assert_eq!(d[0].line, Some(10));
        assert_eq!(d[1].file.as_deref(), Some("src/b.tsx"));
        assert_eq!(d[2].code.as_deref(), Some("CS1002"));
        assert_eq!(d[2].message, "; expected");
        assert_eq!(d[2].file.as_deref(), Some("C:/p/Program.cs"));
    }

    #[test]
    fn eslint_stylish() {
        let out = "
/repo/src/app.ts
  3:7   error    'x' is assigned a value but never used  no-unused-vars
  10:1  warning  Unexpected console statement            no-console

✖ 2 problems (1 error, 1 warning)
";
        let d = parse_diagnostics(out, true);
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!(d[0].file.as_deref(), Some("/repo/src/app.ts"));
        assert_eq!(d[0].code.as_deref(), Some("no-unused-vars"));
        assert_eq!(d[0].message, "'x' is assigned a value but never used");
        assert_eq!(d[1].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn generic_forms() {
        let out = "a.py:1:1: F401 'os' imported but unused
pkg/m.py:3: error: Incompatible types in assignment  [assignment]
./main.go:12:2: undefined: foo
lib.c:5:3: warning: unused variable 'z'
Finished in 2s: done
";
        let d = parse_diagnostics(out, true);
        assert_eq!(d.len(), 4, "{d:#?}");
        assert_eq!(d[0].code.as_deref(), Some("F401"));
        assert_eq!(d[1].code.as_deref(), Some("assignment"));
        assert_eq!(d[1].column, None);
        assert_eq!(d[2].file.as_deref(), Some("main.go"));
        assert_eq!(d[3].severity, DiagnosticSeverity::Warning);
        assert!(parse_diagnostics(out, false).is_empty());
    }

    #[test]
    fn javac() {
        let d = parse_diagnostics("[ERROR] /p/src/A.java:[10,5] cannot find symbol\n", false);
        assert_eq!(d[0].line, Some(10));
        assert_eq!(d[0].message, "cannot find symbol");
    }
}

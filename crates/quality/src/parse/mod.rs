//! Output parsers: test summaries (libtest, Jest, Vitest, pytest, `go test`,
//! `dotnet test`, Maven Surefire, Gradle) and diagnostics (rustc/clippy, tsc,
//! MSBuild, ESLint stylish, javac, and the generic `file:line[:col]: msg`
//! form used by go vet, ruff, flake8, mypy, gcc).
//!
//! Parsers are tolerant: every parser runs on the ANSI-stripped output and
//! contributes what it recognises; diagnostics are de-duplicated and capped.

mod diagnostics;
mod tests_summary;

use orkestr_protocol::methods::quality::{CheckKind, Diagnostic, DiagnosticSeverity, TestFailure};

pub use diagnostics::parse_diagnostics;
pub use tests_summary::{parse_tests, TestSummary};

/// Max diagnostics kept per check.
pub const MAX_DIAGNOSTICS: usize = 200;
/// Max failures kept per check.
pub const MAX_FAILURES: usize = 100;
/// Max length of one message.
pub const MAX_MESSAGE: usize = 2000;

/// Everything recognised in a check's output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Test counts (when a test summary was found).
    pub tests: Option<TestSummary>,
    /// Failing tests.
    pub failures: Vec<TestFailure>,
    /// Diagnostics (errors first).
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    /// Number of error diagnostics.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == DiagnosticSeverity::Error).count()
    }
}

/// Removes ANSI escape sequences (colors, cursor movement) and `\r`.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: parameters/intermediates until a final byte 0x40..=0x7e.
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: until BEL or ESC \.
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Truncates to [`MAX_MESSAGE`] characters (on a char boundary).
#[must_use]
pub fn clip(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() <= MAX_MESSAGE {
        return t.to_owned();
    }
    let mut out: String = t.chars().take(MAX_MESSAGE).collect();
    out.push('…');
    out
}

/// Normalizes a reported path: `\` → `/`, strips a leading `./`.
#[must_use]
pub fn norm_path(p: &str) -> String {
    let s = p.trim().replace('\\', "/");
    s.strip_prefix("./").map(str::to_owned).unwrap_or(s)
}

/// Parses a check's combined output.
#[must_use]
pub fn parse(kind: CheckKind, output: &str) -> Parsed {
    let text = strip_ansi(output);
    let (tests, mut failures) = parse_tests(&text);
    // Generic `file:line: msg` lines inside test output are mostly traceback
    // noise; only use them for non-test checks or when no summary was found.
    let allow_generic = !(kind.is_test() && tests.is_some());
    let mut diagnostics = parse_diagnostics(&text, allow_generic);
    failures.truncate(MAX_FAILURES);
    diagnostics.sort_by_key(|d| match d.severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Info => 2,
    });
    diagnostics.truncate(MAX_DIAGNOSTICS);
    Parsed { tests, failures, diagnostics }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: x\r\n"), "error: x\n");
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{7}b"), "ab");
    }

    #[test]
    fn clip_and_norm() {
        assert_eq!(norm_path(".\\src\\a.rs"), "src/a.rs");
        let long = "x".repeat(MAX_MESSAGE + 10);
        assert_eq!(clip(&long).chars().count(), MAX_MESSAGE + 1);
    }

    #[test]
    fn test_kind_skips_generic_traceback_lines() {
        let out = "tests/test_a.py:12: AssertionError\n=== 1 failed, 2 passed in 0.10s ===\n";
        let p = parse(CheckKind::Test, out);
        assert!(p.diagnostics.is_empty());
        let p = parse(CheckKind::Lint, "src/a.py:3:1: F401 'os' imported but unused\n");
        assert_eq!(p.diagnostics.len(), 1);
    }
}

//! Test-runner summaries and failing tests.

use once_cell::sync::Lazy;
use orkestr_protocol::methods::quality::TestFailure;
use regex::Regex;

use super::{clip, norm_path};

/// Aggregated test counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestSummary {
    /// Passed.
    pub passed: u32,
    /// Failed (incl. errors).
    pub failed: u32,
    /// Skipped / ignored / todo.
    pub skipped: u32,
}

impl TestSummary {
    fn add(&mut self, o: TestSummary) {
        self.passed = self.passed.saturating_add(o.passed);
        self.failed = self.failed.saturating_add(o.failed);
        self.skipped = self.skipped.saturating_add(o.skipped);
    }
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

fn num(s: Option<regex::Match<'_>>) -> u32 {
    s.and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

// libtest
static LIBTEST_RESULT: Lazy<Regex> =
    Lazy::new(|| re(r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored"));
static LIBTEST_FAILED: Lazy<Regex> = Lazy::new(|| re(r"^test (\S+) \.\.\. FAILED$"));
static LIBTEST_SECTION: Lazy<Regex> = Lazy::new(|| re(r"^---- (\S+) stdout ----$"));
static PANIC_NEW: Lazy<Regex> = Lazy::new(|| re(r"panicked at (.+?):(\d+):(\d+):$"));
static PANIC_OLD: Lazy<Regex> = Lazy::new(|| re(r"panicked at '(.*)', (.+?):(\d+):(\d+)"));
// node:test (`node --test`): spec reporter (`ℹ pass 3`) and TAP (`# pass 3`)
static NODE_COUNT: Lazy<Regex> = Lazy::new(|| re(r"^\s*(?:ℹ|#) (tests|pass|fail|cancelled|skipped|todo) (\d+)\s*$"));
static NODE_SPEC_FAIL: Lazy<Regex> = Lazy::new(|| re(r"^\s*✖ (.+?)(?: \([\d.]+m?s\))?\s*$"));
static NODE_SPEC_AT: Lazy<Regex> = Lazy::new(|| re(r"^\s*test at (.+?):(\d+):(\d+)\s*$"));
static NODE_TAP_NOT_OK: Lazy<Regex> = Lazy::new(|| re(r"^\s*not ok \d+ - (.+?)(?:\s+#.*)?$"));
static NODE_TAP_LOCATION: Lazy<Regex> = Lazy::new(|| re(r"^\s*location: '(.+?):(\d+):(\d+)'\s*$"));
// jest / vitest
static JEST_TESTS: Lazy<Regex> = Lazy::new(|| re(r"^\s*Tests:?\s+(.*\d+ (?:passed|failed|skipped|todo|total).*)$"));
static COUNT_WORD: Lazy<Regex> =
    Lazy::new(|| re(r"(\d+) (passed|failed|skipped|todo|pending|errors?|xfailed|xpassed|deselected)"));
static JEST_BULLET: Lazy<Regex> = Lazy::new(|| re(r"^\s*● (.+)$"));
static JS_STACK_LOC: Lazy<Regex> = Lazy::new(|| re(r"\(?([^\s()]+\.[cm]?[jt]sx?):(\d+):(\d+)\)?\s*$"));
static VITEST_FAIL: Lazy<Regex> = Lazy::new(|| re(r"^\s*FAIL\s+(\S+)\s+>\s+(.+?)\s*$"));
// pytest
static PYTEST_SUMMARY: Lazy<Regex> = Lazy::new(|| re(r"^=*\s*((?:\d+ \w+(?:, )?)+)(?: in [\d.]+s.*)?\s*=*$"));
static PYTEST_FAILED: Lazy<Regex> = Lazy::new(|| re(r"^(FAILED|ERROR) (\S+?)(?: - (.*))?$"));
static PY_LOC: Lazy<Regex> = Lazy::new(|| re(r"^(\S+\.py):(\d+): \w+"));
// go
static GO_RESULT: Lazy<Regex> = Lazy::new(|| re(r"^\s*--- (PASS|FAIL|SKIP): (\S+) \("));
static GO_MSG: Lazy<Regex> = Lazy::new(|| re(r"^\s+(\S+\.go):(\d+): (.*)$"));
static GO_PKG_OK: Lazy<Regex> = Lazy::new(|| re(r"^ok\s+\S+\s"));
static GO_PKG_FAIL: Lazy<Regex> = Lazy::new(|| re(r"^FAIL\s+\S+\s"));
// dotnet
static DOTNET_SUMMARY: Lazy<Regex> =
    Lazy::new(|| re(r"(?:Passed|Failed)!\s+-\s+Failed:\s+(\d+),\s+Passed:\s+(\d+),\s+Skipped:\s+(\d+)"));
static DOTNET_FAILED: Lazy<Regex> = Lazy::new(|| re(r"^\s*Failed (\S+) \["));
// maven / gradle
static SUREFIRE: Lazy<Regex> =
    Lazy::new(|| re(r"Tests run: (\d+), Failures: (\d+), Errors: (\d+), Skipped: (\d+)(.*)$"));
static SUREFIRE_FAIL: Lazy<Regex> = Lazy::new(|| re(r"^\[ERROR\]\s{2,}(\S+?)(?::(\d+))?\s+(.+)$"));
static GRADLE_SUMMARY: Lazy<Regex> = Lazy::new(|| re(r"(\d+) tests? completed, (\d+) failed(?:, (\d+) skipped)?"));
static GRADLE_FAILED: Lazy<Regex> = Lazy::new(|| re(r"^(\S+) > (.+) FAILED$"));

/// A runner-specific parser: summary + failures when the output matches.
type RunnerParser = fn(&[&str]) -> Option<(TestSummary, Vec<TestFailure>)>;

/// Parses test summaries and failures. Returns `None` counts when no runner
/// summary was recognised (failures may still be found).
#[must_use]
pub fn parse_tests(text: &str) -> (Option<TestSummary>, Vec<TestFailure>) {
    let lines: Vec<&str> = text.lines().collect();
    let parsers: [RunnerParser; 8] = [libtest, node_test, jest_vitest, pytest, gotest, dotnet, surefire, gradle];
    for p in parsers {
        if let Some((s, f)) = p(&lines) {
            return (Some(s), f);
        }
    }
    (None, Vec::new())
}

fn next_message(lines: &[&str], from: usize, max_lines: usize) -> String {
    let msg: Vec<&str> = lines.iter().skip(from).map(|l| l.trim()).filter(|l| !l.is_empty()).take(max_lines).collect();
    clip(&msg.join("\n"))
}

fn libtest(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = TestSummary::default();
    let mut found = false;
    for l in lines {
        if let Some(c) = LIBTEST_RESULT.captures(l) {
            found = true;
            total.add(TestSummary { passed: num(c.get(1)), failed: num(c.get(2)), skipped: num(c.get(3)) });
        }
    }
    if !found {
        return None;
    }
    let mut failures: Vec<TestFailure> = Vec::new();
    for l in lines {
        if let Some(c) = LIBTEST_FAILED.captures(l) {
            let name = c[1].to_owned();
            if !failures.iter().any(|f| f.name == name) {
                failures.push(TestFailure { name, message: String::new(), file: None, line: None });
            }
        }
    }
    // `---- name stdout ----` sections carry the panic message + location.
    let mut i = 0;
    while i < lines.len() {
        if let Some(c) = LIBTEST_SECTION.captures(lines[i]) {
            let name = c[1].to_owned();
            let mut body = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && !LIBTEST_SECTION.is_match(lines[j]) && lines[j].trim() != "failures:" {
                body.push(lines[j]);
                j += 1;
            }
            let (mut file, mut line, mut message) = (None, None, String::new());
            for (k, b) in body.iter().enumerate() {
                if let Some(p) = PANIC_NEW.captures(b) {
                    file = Some(norm_path(&p[1]));
                    line = p[2].parse().ok();
                    message = next_message(&body, k + 1, 6);
                    break;
                }
                if let Some(p) = PANIC_OLD.captures(b) {
                    message = clip(&p[1]);
                    file = Some(norm_path(&p[2]));
                    line = p[3].parse().ok();
                    break;
                }
            }
            if message.is_empty() {
                message = next_message(&body, 0, 6);
            }
            match failures.iter_mut().find(|f| f.name == name) {
                Some(f) => {
                    f.message = message;
                    f.file = file;
                    f.line = line;
                }
                None => failures.push(TestFailure { name, message, file, line }),
            }
            i = j;
        } else {
            i += 1;
        }
    }
    Some((total, failures))
}

fn counts(s: &str) -> TestSummary {
    let mut t = TestSummary::default();
    for c in COUNT_WORD.captures_iter(s) {
        let n = num(c.get(1));
        match &c[2] {
            "passed" | "xpassed" => t.passed += n,
            "failed" | "error" | "errors" => t.failed += n,
            _ => t.skipped += n,
        }
    }
    t
}

/// Node's built-in runner (`node --test`, also behind `npm test`): the
/// summary block (`ℹ pass 1` / `# pass 1`) and the failing tests — the spec
/// reporter's "failing tests" section (`test at file:line:col`, `✖ name`,
/// message) or TAP `not ok` entries with their YAML `location` / `error`.
fn node_test(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = TestSummary::default();
    let mut found = false;
    for l in lines {
        if let Some(c) = NODE_COUNT.captures(l) {
            let n = num(c.get(2));
            match &c[1] {
                "pass" => total.passed = total.passed.saturating_add(n),
                "fail" | "cancelled" => total.failed = total.failed.saturating_add(n),
                "skipped" | "todo" => total.skipped = total.skipped.saturating_add(n),
                _ => {}
            }
            found = true;
        }
    }
    if !found {
        return None;
    }
    let mut failures = node_spec_failures(lines);
    if failures.is_empty() {
        failures = node_tap_failures(lines);
    }
    Some((total, failures))
}

fn node_location(path: &str) -> String {
    // TAP's YAML escapes backslashes (`C:\\dir\\a.mjs`).
    norm_path(&path.trim_start_matches("file:///").replace(r"\\", r"\"))
}

/// Spec reporter: details live under "✖ failing tests:".
fn node_spec_failures(lines: &[&str]) -> Vec<TestFailure> {
    let mut failures: Vec<TestFailure> = Vec::new();
    let Some(start) = lines.iter().position(|l| l.trim_start().starts_with("✖ failing tests")) else {
        return failures;
    };
    let entry_start = |l: &str| NODE_SPEC_AT.is_match(l) || NODE_SPEC_FAIL.is_match(l);
    let mut loc: Option<(String, Option<u32>)> = None;
    let mut i = start + 1;
    while i < lines.len() {
        if let Some(c) = NODE_SPEC_AT.captures(lines[i]) {
            loc = Some((node_location(&c[1]), c[2].parse().ok()));
            i += 1;
            continue;
        }
        let Some(c) = NODE_SPEC_FAIL.captures(lines[i]) else {
            i += 1;
            continue;
        };
        let name = c[1].trim().to_owned();
        let mut msg = Vec::new();
        let mut j = i + 1;
        // Message: up to the stack trace / error properties.
        while j < lines.len() && !entry_start(lines[j]) {
            let t = lines[j].trim();
            if t.starts_with("at ") || t.starts_with('{') || t.ends_with('{') {
                break;
            }
            if !t.is_empty() && msg.len() < 8 {
                msg.push(t);
            }
            j += 1;
        }
        while j < lines.len() && !entry_start(lines[j]) {
            j += 1;
        }
        let (file, line) = loc.take().map_or((None, None), |(f, l)| (Some(f), l));
        if !failures.iter().any(|f| f.name == name) {
            failures.push(TestFailure { name, message: clip(&msg.join("\n")), file, line });
        }
        i = j;
    }
    failures
}

/// TAP reporter: `not ok N - name` followed by a YAML block.
fn node_tap_failures(lines: &[&str]) -> Vec<TestFailure> {
    let mut failures: Vec<TestFailure> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(c) = NODE_TAP_NOT_OK.captures(l) else { continue };
        let name = c[1].trim().to_owned();
        let (mut file, mut line, mut msg) = (None, None, Vec::<String>::new());
        let mut in_error = false;
        let mut error_indent = 0usize;
        for next in lines.iter().skip(i + 1).take(60) {
            let t = next.trim();
            if t == "..." || NODE_TAP_NOT_OK.is_match(next) {
                break;
            }
            let indent = next.len() - next.trim_start().len();
            if in_error && (t.is_empty() || indent > error_indent) {
                if !t.is_empty() && msg.len() < 8 {
                    msg.push(t.to_owned());
                }
                continue;
            }
            in_error = false;
            if let Some(loc) = NODE_TAP_LOCATION.captures(next) {
                file = Some(node_location(&loc[1]));
                line = loc[2].parse().ok();
            } else if let Some(rest) = t.strip_prefix("error:") {
                let rest = rest.trim();
                if rest.starts_with('|') || rest.starts_with('>') {
                    in_error = true;
                    error_indent = indent;
                } else if !rest.is_empty() {
                    msg.push(rest.trim_matches('\'').to_owned());
                }
            }
        }
        if !failures.iter().any(|f| f.name == name) {
            failures.push(TestFailure { name, message: clip(&msg.join("\n")), file, line });
        }
    }
    failures
}

fn jest_vitest(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = None::<TestSummary>;
    for l in lines {
        if let Some(c) = JEST_TESTS.captures(l) {
            total.get_or_insert_with(TestSummary::default).add(counts(&c[1]));
        }
    }
    let total = total?;
    let mut failures: Vec<TestFailure> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let (name, mut file) = if let Some(c) = VITEST_FAIL.captures(l) {
            (c[2].replace(" > ", " › "), Some(norm_path(&c[1])))
        } else if let Some(c) = JEST_BULLET.captures(l) {
            (c[1].trim().to_owned(), None)
        } else {
            continue;
        };
        if failures.iter().any(|f| f.name == name) {
            continue;
        }
        // Message: next non-empty lines up to the stack / next failure.
        let mut msg = Vec::new();
        let mut line = None;
        for next in lines.iter().skip(i + 1).take(40) {
            let t = next.trim();
            if JEST_BULLET.is_match(next)
                || VITEST_FAIL.is_match(next)
                || JEST_TESTS.is_match(next)
                || t.starts_with("Test Suites:")
                || t.starts_with("Test Files")
            {
                break;
            }
            if let Some(loc) = JS_STACK_LOC.captures(t) {
                if t.starts_with("at ") || t.starts_with('❯') || t.starts_with('>') {
                    if line.is_none() && !loc[1].contains("node_modules") {
                        file = Some(norm_path(&loc[1]));
                        line = loc[2].parse().ok();
                    }
                    continue;
                }
            }
            if !t.is_empty() && msg.len() < 6 && !t.starts_with('⎯') {
                msg.push(t);
            }
        }
        failures.push(TestFailure { name, message: clip(&msg.join("\n")), file, line });
    }
    Some((total, failures))
}

fn pytest(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = None;
    for l in lines.iter().rev() {
        let t = l.trim();
        if !(t.contains(" passed") || t.contains(" failed") || t.contains(" error") || t.contains(" skipped")) {
            continue;
        }
        if !(t.contains(" in ") || t.starts_with('=')) {
            continue;
        }
        if let Some(c) = PYTEST_SUMMARY.captures(t) {
            total = Some(counts(&c[1]));
            break;
        }
    }
    let total = total?;
    let mut failures: Vec<TestFailure> = Vec::new();
    for l in lines {
        if let Some(c) = PYTEST_FAILED.captures(l.trim_end()) {
            let id = c[2].to_owned();
            let file = id.split("::").next().map(norm_path);
            failures.push(TestFailure {
                name: id,
                message: c.get(3).map(|m| clip(m.as_str())).unwrap_or_default(),
                file,
                line: None,
            });
        }
    }
    // Attach line numbers from tracebacks (`path.py:12: AssertionError`).
    for l in lines {
        if let Some(c) = PY_LOC.captures(l) {
            let f = norm_path(&c[1]);
            if let Some(fail) = failures.iter_mut().find(|x| x.line.is_none() && x.file.as_deref() == Some(&f)) {
                fail.line = c[2].parse().ok();
            }
        }
    }
    Some((total, failures))
}

fn gotest(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = TestSummary::default();
    let mut found = false;
    let mut failures: Vec<TestFailure> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(c) = GO_RESULT.captures(l) {
            found = true;
            match &c[1] {
                "PASS" => total.passed += 1,
                "SKIP" => total.skipped += 1,
                _ => {
                    total.failed += 1;
                    let mut fail =
                        TestFailure { name: c[2].to_owned(), message: String::new(), file: None, line: None };
                    let mut msgs = Vec::new();
                    for next in lines.iter().skip(i + 1).take(30) {
                        let Some(m) = GO_MSG.captures(next) else { break };
                        if fail.file.is_none() {
                            fail.file = Some(norm_path(&m[1]));
                            fail.line = m[2].parse().ok();
                        }
                        msgs.push(m[3].trim().to_owned());
                    }
                    fail.message = clip(&msgs.join("\n"));
                    failures.push(fail);
                }
            }
        } else if GO_PKG_OK.is_match(l) || GO_PKG_FAIL.is_match(l) {
            found = true;
        }
    }
    found.then_some((total, failures))
}

fn dotnet(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = None::<TestSummary>;
    for l in lines {
        if let Some(c) = DOTNET_SUMMARY.captures(l) {
            total.get_or_insert_with(TestSummary::default).add(TestSummary {
                failed: num(c.get(1)),
                passed: num(c.get(2)),
                skipped: num(c.get(3)),
            });
        }
    }
    let total = total?;
    let mut failures = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(c) = DOTNET_FAILED.captures(l) {
            let message = lines
                .iter()
                .skip(i + 1)
                .take(8)
                .map(|x| x.trim())
                .skip_while(|x| x.is_empty() || *x == "Error Message:")
                .take(3)
                .collect::<Vec<_>>()
                .join("\n");
            failures.push(TestFailure { name: c[1].to_owned(), message: clip(&message), file: None, line: None });
        }
    }
    Some((total, failures))
}

fn surefire(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = None::<TestSummary>;
    for l in lines {
        if let Some(c) = SUREFIRE.captures(l) {
            // Per-class lines carry "Time elapsed"; the module totals do not.
            if c[5].contains("Time elapsed") {
                continue;
            }
            let run = num(c.get(1));
            let failed = num(c.get(2)).saturating_add(num(c.get(3)));
            let skipped = num(c.get(4));
            total.get_or_insert_with(TestSummary::default).add(TestSummary {
                passed: run.saturating_sub(failed).saturating_sub(skipped),
                failed,
                skipped,
            });
        }
    }
    let total = total?;
    let mut failures = Vec::new();
    let mut in_section = false;
    for l in lines {
        let t = l.trim();
        if t.ends_with("Failures:") || t.ends_with("Errors:") {
            in_section = true;
            continue;
        }
        if in_section {
            if let Some(c) = SUREFIRE_FAIL.captures(l) {
                failures.push(TestFailure {
                    name: c[1].to_owned(),
                    message: clip(&c[3]),
                    file: None,
                    line: c.get(2).and_then(|m| m.as_str().parse().ok()),
                });
            } else if !t.starts_with("[ERROR]") || t.contains("Tests run:") {
                in_section = false;
            }
        }
    }
    Some((total, failures))
}

fn gradle(lines: &[&str]) -> Option<(TestSummary, Vec<TestFailure>)> {
    let mut total = None;
    for l in lines {
        if let Some(c) = GRADLE_SUMMARY.captures(l) {
            let run = num(c.get(1));
            let failed = num(c.get(2));
            let skipped = num(c.get(3));
            total = Some(TestSummary { passed: run.saturating_sub(failed).saturating_sub(skipped), failed, skipped });
        }
    }
    let total = total?;
    let failures = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            GRADLE_FAILED.captures(l.trim()).map(|c| TestFailure {
                name: format!("{} > {}", &c[1], &c[2]),
                message: next_message(lines, i + 1, 2),
                file: None,
                line: None,
            })
        })
        .collect();
    Some((total, failures))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE_SPEC: &str = "> test
> node --test

✔ ok one (0.4708ms)
✖ greets by name (0.6178ms)
﹣ skipped one (0.0755ms) # SKIP
ℹ tests 3
ℹ suites 0
ℹ pass 1
ℹ fail 1
ℹ cancelled 0
ℹ skipped 1
ℹ todo 0
ℹ duration_ms 62.0741

✖ failing tests:

test at tests\\a.test.mjs:4:1
✖ greets by name (0.6178ms)
  AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:
  + actual - expected

  + 'Hello, X'
  - 'Hello, X!'
             ^

      at TestContext.<anonymous> (file:///C:/tmp/nt/tests/a.test.mjs:4:39)
      at Test.runInAsyncScope (node:async_hooks:227:14) {
    generatedMessage: true,
    code: 'ERR_ASSERTION',
  }
";

    const NODE_TAP: &str = "TAP version 13
# Subtest: ok one
ok 1 - ok one
  ---
  duration_ms: 0.5
  ...
# Subtest: greets by name
not ok 2 - greets by name
  ---
  duration_ms: 0.7
  location: 'C:\\\\tmp\\\\nt\\\\tests\\\\a.test.mjs:4:1'
  failureType: 'testCodeFailure'
  error: |-
    Expected values to be strictly equal:

    'Hello, X' !== 'Hello, X!'
  code: 'ERR_ASSERTION'
  ...
1..2
# tests 2
# suites 0
# pass 1
# fail 1
# cancelled 0
# skipped 0
# todo 0
";

    #[test]
    fn node_test_spec_reporter() {
        let (s, f) = parse_tests(NODE_SPEC);
        assert_eq!(s, Some(TestSummary { passed: 1, failed: 1, skipped: 1 }));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].name, "greets by name");
        assert_eq!(f[0].file.as_deref(), Some("tests/a.test.mjs"));
        assert_eq!(f[0].line, Some(4));
        assert!(f[0].message.contains("Expected values to be strictly equal"), "{}", f[0].message);
        assert!(f[0].message.contains("'Hello, X!'"), "{}", f[0].message);
        assert!(!f[0].message.contains("at TestContext"), "{}", f[0].message);
    }

    #[test]
    fn node_test_tap_reporter() {
        let (s, f) = parse_tests(NODE_TAP);
        assert_eq!(s, Some(TestSummary { passed: 1, failed: 1, skipped: 0 }));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].name, "greets by name");
        assert_eq!(f[0].file.as_deref(), Some("C:/tmp/nt/tests/a.test.mjs"));
        assert_eq!(f[0].line, Some(4));
        assert!(f[0].message.contains("'Hello, X' !== 'Hello, X!'"), "{}", f[0].message);
        assert!(!f[0].message.contains("ERR_ASSERTION"), "{}", f[0].message);
    }

    #[test]
    fn node_test_all_passing() {
        let (s, f) = parse_tests("✔ a (1ms)\nℹ tests 2\nℹ pass 2\nℹ fail 0\nℹ skipped 0\n");
        assert_eq!(s, Some(TestSummary { passed: 2, failed: 0, skipped: 0 }));
        assert!(f.is_empty());
    }

    const LIBTEST: &str = "running 3 tests
test a::ok ... ok
test a::bad ... FAILED
test a::skip ... ignored

failures:

---- a::bad stdout ----

thread 'a::bad' panicked at src/lib.rs:12:9:
assertion `left == right` failed
  left: 1
 right: 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    a::bad

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    #[test]
    fn libtest_counts_and_failure() {
        let (s, f) = parse_tests(LIBTEST);
        assert_eq!(s, Some(TestSummary { passed: 3, failed: 1, skipped: 1 }));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "a::bad");
        assert_eq!(f[0].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(f[0].line, Some(12));
        assert!(f[0].message.starts_with("assertion `left == right` failed"), "{}", f[0].message);
    }

    #[test]
    fn libtest_old_panic_format() {
        let out = "---- t stdout ----\nthread 't' panicked at 'boom', src/x.rs:3:5\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;\n";
        let (_, f) = parse_tests(out);
        assert_eq!(f[0].message, "boom");
        assert_eq!(f[0].line, Some(3));
    }

    #[test]
    fn jest_summary_and_failures() {
        let out = "FAIL src/sum.test.js
  ● math › adds numbers

    expect(received).toBe(expected) // Object.is equality

    Expected: 4
    Received: 5

      at Object.<anonymous> (src/sum.test.js:5:17)

Test Suites: 1 failed, 1 passed, 2 total
Tests:       1 failed, 1 skipped, 7 passed, 9 total
Snapshots:   0 total
";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 7, failed: 1, skipped: 1 }));
        assert_eq!(f[0].name, "math › adds numbers");
        assert_eq!(f[0].file.as_deref(), Some("src/sum.test.js"));
        assert_eq!(f[0].line, Some(5));
        assert!(f[0].message.contains("Expected: 4"));
    }

    #[test]
    fn vitest_summary_and_failures() {
        let out = " ❯ src/a.test.ts (2 tests | 1 failed) 5ms

⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/a.test.ts > suite > does things
AssertionError: expected 1 to be 2 // Object.is equality
 ❯ src/a.test.ts:7:15

 Test Files  1 failed (1)
      Tests  1 failed | 1 passed | 2 skipped (4)
   Start at  10:00:00
";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 1, failed: 1, skipped: 2 }));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "suite › does things");
        assert_eq!(f[0].file.as_deref(), Some("src/a.test.ts"));
        assert_eq!(f[0].line, Some(7));
        assert!(f[0].message.starts_with("AssertionError"));
    }

    #[test]
    fn pytest_summary_and_failures() {
        let out = "============================= test session starts =============================
collected 4 items

tests/test_a.py .F.s                                                     [100%]

================================== FAILURES ===================================
___________________________________ test_b ____________________________________

    def test_b():
>       assert 1 == 2
E       assert 1 == 2

tests/test_a.py:7: AssertionError
=========================== short test summary info ===========================
FAILED tests/test_a.py::test_b - assert 1 == 2
=================== 1 failed, 2 passed, 1 skipped in 0.12s ====================
";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 2, failed: 1, skipped: 1 }));
        assert_eq!(f[0].name, "tests/test_a.py::test_b");
        assert_eq!(f[0].file.as_deref(), Some("tests/test_a.py"));
        assert_eq!(f[0].line, Some(7));
        assert_eq!(f[0].message, "assert 1 == 2");
        // -q form
        let (s, _) = parse_tests("..F\n1 failed, 2 passed in 0.05s\n");
        assert_eq!(s, Some(TestSummary { passed: 2, failed: 1, skipped: 0 }));
    }

    #[test]
    fn go_test() {
        let out = "=== RUN   TestAdd
--- PASS: TestAdd (0.00s)
=== RUN   TestSub
    calc_test.go:14: got 1, want 2
--- FAIL: TestSub (0.00s)
    calc_test.go:14: got 1, want 2
--- SKIP: TestSlow (0.00s)
FAIL
FAIL	example.com/calc	0.004s
";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 1, failed: 1, skipped: 1 }));
        assert_eq!(f[0].name, "TestSub");
        assert_eq!(f[0].file.as_deref(), Some("calc_test.go"));
        assert_eq!(f[0].line, Some(14));
        assert_eq!(f[0].message, "got 1, want 2");
    }

    #[test]
    fn dotnet_test() {
        let out = "  Failed Calc.Tests.SubTest [12 ms]
  Error Message:
   Assert.Equal() Failure
Failed!  - Failed:     1, Passed:     4, Skipped:     0, Total:     5, Duration: 30 ms - Calc.Tests.dll (net8.0)
";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 4, failed: 1, skipped: 0 }));
        assert_eq!(f[0].name, "Calc.Tests.SubTest");
        assert!(f[0].message.contains("Assert.Equal() Failure"));
    }

    #[test]
    fn maven_surefire() {
        let out =
            "[INFO] Tests run: 3, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! - in CalcTest
[ERROR] Failures:
[ERROR]   CalcTest.testSub:14 expected:<2> but was:<1>
[INFO]
[ERROR] Tests run: 5, Failures: 1, Errors: 1, Skipped: 1
";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 2, failed: 2, skipped: 1 }));
        assert_eq!(f[0].name, "CalcTest.testSub");
        assert_eq!(f[0].line, Some(14));
    }

    #[test]
    fn gradle_summary() {
        let out = "CalcTest > testSub FAILED\n    org.opentest4j.AssertionFailedError at CalcTest.java:14\n\n5 tests completed, 1 failed, 1 skipped\n";
        let (s, f) = parse_tests(out);
        assert_eq!(s, Some(TestSummary { passed: 3, failed: 1, skipped: 1 }));
        assert_eq!(f[0].name, "CalcTest > testSub");
    }

    #[test]
    fn nothing_recognised() {
        assert_eq!(parse_tests("hello\nworld\n"), (None, vec![]));
    }
}

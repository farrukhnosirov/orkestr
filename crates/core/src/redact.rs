//! Log redaction (contract §3 "secrets redacted", §6 "never in logs/events").
//!
//! Applied to everything the core logs — both the stdout/JSON `tracing`
//! output and the `Log` events forwarded to the event bus:
//!
//! * **Registered secrets** — exact values the core knows are secret (its own
//!   RPC bearer token) are replaced wherever they appear.
//! * **Sensitive field names** — structured fields such as `token`,
//!   `api_key`, `password`, `authorization` are replaced entirely.
//! * **Well-known patterns** — `Bearer …`, `key=value` pairs with sensitive
//!   keys, provider key prefixes (`sk-…`, `ghp_…`, `AKIA…`, `AIza…`, `xox?-…`).
//!
//! This is a conservative baseline; `orkestr-security::Redactor` (Phase 1)
//! extends it for tool I/O and agent transcripts.

use std::sync::OnceLock;

use parking_lot::RwLock;
use regex::Regex;

/// Replacement marker.
pub const REDACTED: &str = "[REDACTED]";

/// Minimum length of a registered secret (shorter values would cause
/// excessive false positives).
const MIN_SECRET_LEN: usize = 8;

static SECRETS: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();

fn secrets() -> &'static RwLock<Vec<String>> {
    SECRETS.get_or_init(|| RwLock::new(Vec::new()))
}

fn patterns() -> &'static [(Regex, &'static str)] {
    PATTERNS.get_or_init(|| {
        let raw: [(&str, &'static str); 5] = [
            // `Authorization: Bearer abc` (scheme included in the redaction).
            (
                r#"(?i)\b(authorization"?\s*[:=]\s*"?)(?:(?:bearer|basic|token)\s+)?[^\s"',;&}]+"#,
                "${1}[REDACTED]",
            ),
            // Free-standing `Bearer abc`.
            (r"(?i)\b(bearer\s+)[A-Za-z0-9\-._~+/]+=*", "${1}[REDACTED]"),
            // `api_key=…`, `"token": "…"`, `password: …`.
            (
                r#"(?i)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|token|secret|client[_-]?secret|password|passwd)"?\s*[:=]\s*"?)[^\s"',;&}]+"#,
                "${1}[REDACTED]",
            ),
            // Provider / platform key formats.
            (
                r"\b(?:sk-(?:ant-|proj-)?[A-Za-z0-9_\-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_\-]{35}|xox[abprs]-[A-Za-z0-9\-]{10,})",
                "[REDACTED]",
            ),
            // `user:password@host` in URLs.
            (r"(?i)\b([a-z][a-z0-9+.\-]*://[^/\s:@]+:)[^@\s/]+@", "${1}[REDACTED]@"),
        ];
        raw.into_iter()
            .map(|(re, rep)| (Regex::new(re).expect("redaction patterns are valid"), rep))
            .collect()
    })
}

/// Registers an exact secret value to scrub from all future log output.
pub fn register_secret(value: &str) {
    let value = value.trim();
    if value.len() < MIN_SECRET_LEN {
        return;
    }
    let mut list = secrets().write();
    if !list.iter().any(|s| s == value) {
        list.push(value.to_owned());
    }
}

/// Forgets a registered secret (e.g. when the core that owned it shuts down).
pub fn unregister_secret(value: &str) {
    secrets().write().retain(|s| s != value.trim());
}

/// Whether a structured field name denotes a secret.
#[must_use]
pub fn is_sensitive_field(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["token", "secret", "password", "passwd", "api_key", "apikey", "authorization", "credential", "private_key"]
        .iter()
        .any(|k| n.contains(k))
}

/// Returns `text` with secrets replaced by [`REDACTED`].
#[must_use]
pub fn redact(text: &str) -> String {
    let mut out = text.to_owned();
    for secret in secrets().read().iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    for (re, rep) in patterns() {
        if re.is_match(&out) {
            out = re.replace_all(&out, *rep).into_owned();
        }
    }
    out
}

/// `io::Write` adapter that redacts each formatted record before writing it.
///
/// `tracing-subscriber`'s fmt layer formats a whole record into a buffer and
/// writes it with a single `write_all`, so per-call redaction sees complete
/// lines.
#[derive(Debug)]
pub struct RedactingWriter<W: std::io::Write> {
    inner: W,
}

impl<W: std::io::Write> std::io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match std::str::from_utf8(buf) {
            Ok(text) => self.inner.write_all(redact(text).as_bytes())?,
            Err(_) => self.inner.write_all(buf)?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// `MakeWriter` producing redacting stdout writers.
#[derive(Debug, Clone, Copy, Default)]
pub struct RedactingStdout;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RedactingStdout {
    type Writer = RedactingWriter<std::io::Stdout>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter { inner: std::io::stdout() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_patterns() {
        let cases = [
            ("Authorization: Bearer abc.def-ghi", "Authorization: [REDACTED]"),
            ("sent bearer abc.def-ghi", "sent bearer [REDACTED]"),
            ("api_key=12345678 rest", "api_key=[REDACTED] rest"),
            (r#"{"token": "zzz"}"#, r#"{"token": "[REDACTED]"}"#),
            ("key sk-ant-REDACTEDREDACTED01234 used", "key [REDACTED] used"),
            ("ghp_abcdefghijklmnopqrstuvwx", "[REDACTED]"),
            ("AKIAABCDEFGHIJKLMNOP", "[REDACTED]"),
            ("https://user:hunter2@example.com/x", "https://user:[REDACTED]@example.com/x"),
            ("password: hunter2", "password: [REDACTED]"),
        ];
        for (input, expected) in cases {
            assert_eq!(redact(input), expected, "{input}");
        }
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        for s in ["orkestr core started", "url=ws://127.0.0.1:5000/rpc", "tokens used: 512", "task 42 completed"] {
            assert_eq!(redact(s), s);
        }
    }

    #[test]
    fn registered_secrets_are_scrubbed() {
        let secret = "0123456789abcdef0123456789abcdef-unit-test";
        register_secret(secret);
        assert_eq!(redact(&format!("x {secret} y")), "x [REDACTED] y");
        unregister_secret(secret);
        assert!(redact(secret).contains(secret));
        register_secret("short"); // ignored
        assert_eq!(redact("short"), "short");
    }

    #[test]
    fn sensitive_fields() {
        assert!(is_sensitive_field("token"));
        assert!(is_sensitive_field("OPENAI_API_KEY"));
        assert!(is_sensitive_field("client_secret"));
        assert!(!is_sensitive_field("url"));
        assert!(!is_sensitive_field("startup_ms"));
    }
}

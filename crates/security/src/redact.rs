//! Secret redaction (contract §3 "secrets redacted", §6 "never in logs/events",
//! ADR-0011 §Redaction).
//!
//! A process-global registry of **exact secret values** plus pattern
//! detectors. Used by the core's `tracing` writer, the `Log` event forwarder,
//! audit details and any tool output that is persisted or published.
//!
//! * **Registered secrets** — values the core knows are secret: its own RPC
//!   bearer token and every value stored in or resolved from the
//!   [`SecretService`](crate::secrets::SecretService) (also their base64 form).
//! * **Sensitive field names** — structured fields such as `token`,
//!   `api_key`, `password`, `authorization` are replaced entirely.
//! * **Well-known patterns** — `Bearer …`, `key=value` pairs with sensitive
//!   keys, provider key prefixes (`sk-…`, `ghp_…`, `AKIA…`, `AIza…`,
//!   `xox?-…`), JWTs, PEM private keys, URL credentials.
//!
//! Registered values are kept in memory only; [`clear_secrets`] drops them
//! (vault lock). Redaction is best effort for unknown secret formats.

use std::sync::OnceLock;

use base64::Engine as _;
use parking_lot::RwLock;
use regex::Regex;
use zeroize::Zeroize;

/// Replacement marker.
pub const REDACTED: &str = "[REDACTED]";

/// Minimum length of a registered secret (shorter values would cause
/// excessive false positives).
pub const MIN_SECRET_LEN: usize = 8;

struct Registry {
    values: Vec<String>,
}

impl Drop for Registry {
    fn drop(&mut self) {
        for v in &mut self.values {
            v.zeroize();
        }
    }
}

static SECRETS: OnceLock<RwLock<Registry>> = OnceLock::new();
static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();

fn secrets() -> &'static RwLock<Registry> {
    SECRETS.get_or_init(|| RwLock::new(Registry { values: Vec::new() }))
}

fn patterns() -> &'static [(Regex, &'static str)] {
    PATTERNS.get_or_init(|| {
        let raw: [(&str, &'static str); 7] = [
            // PEM private keys (multi-line).
            (r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----", "[REDACTED]"),
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
            // JWTs.
            (r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}", "[REDACTED]"),
            // `user:password@host` in URLs.
            (r"(?i)\b([a-z][a-z0-9+.\-]*://[^/\s:@]+:)[^@\s/]+@", "${1}[REDACTED]@"),
        ];
        raw.into_iter()
            .map(|(re, rep)| (Regex::new(re).expect("redaction patterns are valid"), rep))
            .collect()
    })
}

fn insert(list: &mut Vec<String>, value: &str) {
    if value.len() >= MIN_SECRET_LEN && !list.iter().any(|s| s == value) {
        list.push(value.to_owned());
        // Longest first so overlapping secrets are fully replaced.
        list.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }
}

/// Registers an exact secret value (and its base64 form) to scrub from all
/// future output.
pub fn register_secret(value: &str) {
    let value = value.trim();
    if value.len() < MIN_SECRET_LEN {
        return;
    }
    let mut reg = secrets().write();
    insert(&mut reg.values, value);
    let b64 = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
    insert(&mut reg.values, b64.trim_end_matches('='));
}

/// Forgets a registered secret.
pub fn unregister_secret(value: &str) {
    let value = value.trim();
    let b64 = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
    let b64 = b64.trim_end_matches('=');
    let mut reg = secrets().write();
    reg.values.retain_mut(|s| {
        let keep = s != value && s != b64;
        if !keep {
            s.zeroize();
        }
        keep
    });
}

/// Drops every registered value (e.g. on vault lock). Patterns still apply.
pub fn clear_secrets() {
    let mut reg = secrets().write();
    for v in &mut reg.values {
        v.zeroize();
    }
    reg.values.clear();
}

/// Number of registered values (diagnostics/tests).
#[must_use]
pub fn registered_count() -> usize {
    secrets().read().values.len()
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
    for secret in secrets().read().values.iter() {
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

/// Redacts every string in a JSON value; values of sensitive keys are
/// replaced entirely.
#[must_use]
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => Value::String(redact(s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_json).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let v = if is_sensitive_field(k) && !v.is_null() {
                        Value::String(REDACTED.into())
                    } else {
                        redact_json(v)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Object-safe redaction interface for engines that take a redactor.
pub trait Redactor: Send + Sync {
    /// Returns `text` with secrets replaced by `[REDACTED]`.
    fn redact(&self, text: &str) -> String;
}

/// The process-global redactor.
#[derive(Debug, Default, Clone, Copy)]
pub struct GlobalRedactor;

impl Redactor for GlobalRedactor {
    fn redact(&self, text: &str) -> String {
        redact(text)
    }
}

/// Longest text a [`StreamRedactor`] holds back before it emits anyway.
pub const STREAM_HOLD_MAX: usize = 16 * 1024;

/// Text that may still be the first half of a secret: a key name / auth
/// scheme at the end of the emitted part (`Bearer `, `api_key = `, …).
fn stream_hold_pattern() -> &'static Regex {
    static HOLD: OnceLock<Regex> = OnceLock::new();
    HOLD.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?:authorization|bearer|basic|token|api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|secret|client[_-]?secret|password|passwd)"?\s*[:=]?\s*"?\s*(?:(?:bearer|basic|token)\s+)?$"#,
        )
        .expect("hold pattern is valid")
    })
}

/// Redacts a streamed text (model output deltas) without ever cutting a
/// secret in two: redacting each chunk alone lets a key split across chunks
/// (`sk-ab` + `cd12…`) through both pattern and registered-secret matching.
///
/// The unfinished last word is held back, together with a preceding key
/// name or auth scheme and an unterminated PEM block; everything before it
/// is redacted and emitted. [`StreamRedactor::flush`] emits the rest at the
/// end of the stream. The emitted parts concatenate to the same text a
/// whole-text [`redact`] gives (secrets are never split across a cut).
#[derive(Debug, Default)]
pub struct StreamRedactor {
    tail: String,
}

impl StreamRedactor {
    /// Creates an empty stream redactor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a chunk; returns the redacted text that is safe to emit now
    /// (possibly empty).
    pub fn push(&mut self, chunk: &str) -> String {
        self.tail.push_str(chunk);
        let cut = stream_safe_cut(&self.tail);
        if cut == 0 {
            return String::new();
        }
        let rest = self.tail.split_off(cut);
        let ready = std::mem::replace(&mut self.tail, rest);
        redact(&ready)
    }

    /// Emits everything still held back (end of the message).
    pub fn flush(&mut self) -> String {
        let rest = std::mem::take(&mut self.tail);
        if rest.is_empty() {
            rest
        } else {
            redact(&rest)
        }
    }
}

/// Byte offset up to which `s` can be redacted and emitted on its own.
fn stream_safe_cut(s: &str) -> usize {
    if s.len() > STREAM_HOLD_MAX {
        return s.len();
    }
    // Up to the start of the last (possibly unfinished) word.
    let mut cut = s.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    // An unterminated PEM block stays together.
    if let Some(b) = s[..cut].rfind("-----BEGIN") {
        if !s[b..cut].contains("-----END") {
            cut = b;
        }
    }
    // A key name / auth scheme right before the cut stays with its value.
    if let Some(m) = stream_hold_pattern().find(&s[..cut]) {
        cut = m.start();
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streamed(chunks: &[&str]) -> String {
        let mut r = StreamRedactor::new();
        let mut out: String = chunks.iter().map(|c| r.push(c)).collect();
        out.push_str(&r.flush());
        out
    }

    #[test]
    fn stream_redaction_catches_secrets_split_across_chunks() {
        let cases: [&[&str]; 6] = [
            &["key sk-ab", "cd1234567890abcdefgh", " used"],
            &["Authorization: Bea", "rer abc.def", "-ghi\nnext"],
            &["api_key", "=", "123", "45678 rest"],
            &["sent bearer ", "abc.def-ghi done"],
            &["-----BEGIN RSA PRIVATE KEY-----\nMI", "IE\n-----END RSA PRIVATE KEY-----\nok"],
            &["plain words ", "only, nothing ", "secret-looking here"],
        ];
        for chunks in cases {
            let whole: String = chunks.concat();
            let out = streamed(chunks);
            assert_eq!(out, redact(&whole), "{chunks:?}");
        }
        assert!(!streamed(&["key sk-ab", "cd1234567890abcdefgh", " used"]).contains("cd1234567890"));
        let secret = "zz9-stream-registered-secret-value";
        register_secret(secret);
        let out = streamed(&["x zz9-stream-regis", "tered-secret-value y"]);
        unregister_secret(secret);
        assert_eq!(out, "x [REDACTED] y");
    }

    #[test]
    fn stream_redaction_emits_finished_words_without_waiting_for_the_end() {
        let mut r = StreamRedactor::new();
        assert_eq!(r.push("Hello wor"), "Hello ");
        assert_eq!(r.push("ld, again"), "world, ");
        assert_eq!(r.flush(), "again");
    }

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
            ("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk end", "jwt [REDACTED] end"),
            ("-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----", "[REDACTED]"),
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
    fn registered_secrets_are_scrubbed_including_base64() {
        let secret = "0123456789abcdef0123456789abcdef-unit-test";
        register_secret(secret);
        assert_eq!(redact(&format!("x {secret} y")), "x [REDACTED] y");
        let b64 = base64::engine::general_purpose::STANDARD.encode(secret);
        assert!(!redact(&b64).contains(b64.trim_end_matches('=')));
        unregister_secret(secret);
        assert!(redact(secret).contains(secret));
        register_secret("short"); // ignored
        assert_eq!(redact("short"), "short");
    }

    #[test]
    fn json_redaction() {
        let v =
            serde_json::json!({"apiKey": "abc", "nested": {"msg": "Bearer xyz123"}, "n": 1, "list": ["password=x1"]});
        let r = redact_json(&v);
        assert_eq!(r["apiKey"], REDACTED);
        assert_eq!(r["nested"]["msg"], "Bearer [REDACTED]");
        assert_eq!(r["n"], 1);
        assert_eq!(r["list"][0], "password=[REDACTED]");
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

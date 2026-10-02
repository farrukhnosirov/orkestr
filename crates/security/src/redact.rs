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

//! Environment sanitization for spawned shells and commands.
//!
//! Child processes start from the core's environment **minus** anything that
//! could leak Orkestr's own credentials or well-known secrets (contract §6):
//! every `ORKESTR_*` variable (the RPC bearer token, test hooks, …) except a
//! small allow-list, and variables whose names look like secrets
//! (`*_TOKEN`, `*_API_KEY`, `*SECRET*`, `*PASSWORD*`, …). Secrets reach a
//! process only when a caller injects them explicitly via [`EnvVar`].

use std::collections::BTreeMap;
use std::ffi::OsString;

/// An explicitly injected environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    /// Name.
    pub name: String,
    /// Value (may be a secret resolved by the caller; never logged).
    pub value: String,
}

impl EnvVar {
    /// Convenience constructor.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self { name: name.into(), value: value.into() }
    }
}

/// `ORKESTR_*` variables that are safe to pass through.
const ORKESTR_ALLOWED: &[&str] = &["ORKESTR_TERMINAL_ID", "ORKESTR_PROJECT_ID", "ORKESTR_AGENT_ID"];

/// Exact names that are always removed.
const DENY_EXACT: &[&str] = &[
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
    "NPM_TOKEN",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_CLIENT_SECRET",
    "DOCKER_PASSWORD",
    "SSH_AUTH_SOCK_ORKESTR",
];

/// Name fragments (upper-cased) marking a variable as secret.
const DENY_FRAGMENTS: &[&str] = &["SECRET", "PASSWORD", "PASSWD", "PRIVATE_KEY"];

/// Name suffixes (upper-cased) marking a variable as secret.
const DENY_SUFFIXES: &[&str] = &["_TOKEN", "_API_KEY", "_APIKEY", "_ACCESS_KEY", "_AUTH"];

/// Whether `name` must not be inherited by child processes.
#[must_use]
pub fn is_sensitive_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("ORKESTR_") {
        return !ORKESTR_ALLOWED.contains(&upper.as_str());
    }
    DENY_EXACT.contains(&upper.as_str())
        || DENY_FRAGMENTS.iter().any(|f| upper.contains(f))
        || DENY_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

/// Validates an injected variable name (`[A-Za-z_][A-Za-z0-9_]*`, ≤ 128).
#[must_use]
pub fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 128
}

/// Builds the child environment: sanitized `base`, then terminal defaults,
/// then explicit `extra` (which wins and may contain secrets on purpose).
///
/// Keys are compared case-insensitively on Windows semantics (the last
/// spelling wins) so `Path` and `PATH` never both appear.
#[must_use]
pub fn build_env<I>(base: I, defaults: &[(&str, &str)], extra: &[EnvVar]) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut map: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    let key = |k: &str| if cfg!(windows) { k.to_ascii_uppercase() } else { k.to_owned() };
    for (k, v) in base {
        let Some(name) = k.to_str() else { continue };
        if name.is_empty() || name.starts_with('=') || is_sensitive_env(name) {
            continue;
        }
        map.insert(key(name), (k.clone(), v));
    }
    for (k, v) in defaults {
        map.insert(key(k), (OsString::from(*k), OsString::from(*v)));
    }
    for var in extra {
        map.insert(key(&var.name), (OsString::from(&var.name), OsString::from(&var.value)));
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
    }

    #[test]
    fn sensitive_names() {
        for n in [
            "ORKESTR_TOKEN",
            "orkestr_core_token",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "MY_SERVICE_API_KEY",
            "DB_PASSWORD",
            "AWS_SECRET_ACCESS_KEY",
            "CLIENT_SECRET",
            "SOME_PRIVATE_KEY_PATH",
        ] {
            assert!(is_sensitive_env(n), "{n} should be sensitive");
        }
        for n in ["PATH", "HOME", "USERPROFILE", "TERM", "LANG", "ORKESTR_TERMINAL_ID", "TOKENIZERS_PARALLELISM"] {
            assert!(!is_sensitive_env(n), "{n} should pass");
        }
    }

    #[test]
    fn build_env_strips_secrets_and_applies_overrides() {
        let base = os(&[("PATH", "/bin"), ("ORKESTR_TOKEN", "t0p"), ("GITHUB_TOKEN", "ghp"), ("TERM", "dumb")]);
        let env = build_env(base, &[("TERM", "xterm-256color")], &[EnvVar::new("API_TOKEN", "injected")]);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_string_lossy().into_owned());
        assert_eq!(get("PATH").as_deref(), Some("/bin"));
        assert_eq!(get("TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(get("ORKESTR_TOKEN"), None);
        assert_eq!(get("GITHUB_TOKEN"), None);
        assert_eq!(get("API_TOKEN").as_deref(), Some("injected"), "explicit injection wins");
    }

    #[test]
    fn env_name_validation() {
        assert!(is_valid_env_name("FOO_1"));
        assert!(is_valid_env_name("_x"));
        assert!(!is_valid_env_name("1FOO"));
        assert!(!is_valid_env_name("A=B"));
        assert!(!is_valid_env_name(""));
    }
}

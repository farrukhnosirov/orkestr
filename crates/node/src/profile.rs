//! Simulated / overridden node profiles (`ORKESTR_NODE_PROFILE=<json>`, Phase 6).
//! **Owner: WP-A.**

use std::collections::BTreeMap;

use orkestr_domain::Node;
use orkestr_protocol::methods::nodes::NodeProfile;

/// Env variable carrying the profile JSON.
pub const PROFILE_ENV: &str = "ORKESTR_NODE_PROFILE";
/// Heartbeat interval override (ms).
pub const HEARTBEAT_ENV: &str = "ORKESTR_NODE_HEARTBEAT_MS";
/// Lease TTL override (ms).
pub const LEASE_TTL_ENV: &str = "ORKESTR_NODE_LEASE_TTL_MS";

/// Source of environment variables (process env or a fixed map in tests).
#[derive(Debug, Clone, Default)]
pub struct EnvSource {
    vars: Option<BTreeMap<String, String>>,
}

impl EnvSource {
    /// The process environment.
    #[must_use]
    pub fn process() -> Self {
        Self { vars: None }
    }

    /// A fixed set of variables.
    #[must_use]
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self { vars: Some(pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect()) }
    }

    /// Reads a variable.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        match &self.vars {
            Some(m) => m.get(key).cloned(),
            None => std::env::var(key).ok(),
        }
    }
}

/// Parses `ORKESTR_NODE_PROFILE` (inline JSON or `@path/to/file.json`).
/// `None` when unset or invalid (invalid profiles are logged).
#[must_use]
pub fn load_profile(env: &EnvSource) -> Option<NodeProfile> {
    // Step 0 skeleton: WP-A adds `@file` support and validation.
    let raw = env.get(PROFILE_ENV)?;
    match serde_json::from_str(&raw) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "invalid {PROFILE_ENV}");
            None
        }
    }
}

/// Applies a profile on top of a detected node.
#[must_use]
pub fn apply_profile(node: Node, profile: &NodeProfile) -> Node {
    // Step 0 skeleton: WP-A implements the overrides.
    let _ = profile;
    node
}

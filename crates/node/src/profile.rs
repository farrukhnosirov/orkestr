//! Simulated / overridden node profiles (`ORKESTR_NODE_PROFILE=<json>`, Phase 6).
//! **Owner: WP-A.**

use std::collections::BTreeMap;

use orkestr_domain::{GpuInfo, Node};
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
/// `None` when unset, empty or invalid (invalid profiles are logged).
#[must_use]
pub fn load_profile(env: &EnvSource) -> Option<NodeProfile> {
    let raw = env.get(PROFILE_ENV)?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let text = if let Some(path) = raw.strip_prefix('@') {
        match std::fs::read_to_string(path.trim()) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, path, "cannot read {PROFILE_ENV} file");
                return None;
            }
        }
    } else {
        raw.to_owned()
    };
    match serde_json::from_str::<NodeProfile>(&text) {
        Ok(p) => {
            tracing::info!(profile = ?p, "node profile override active ({PROFILE_ENV})");
            Some(p)
        }
        Err(e) => {
            tracing::warn!(error = %e, "invalid {PROFILE_ENV}");
            None
        }
    }
}

/// Applies a profile on top of a detected node: every set field replaces the
/// detected value; tool shims make their tool available (added to `tools`).
#[must_use]
pub fn apply_profile(mut node: Node, profile: &NodeProfile) -> Node {
    if let Some(n) = &profile.name {
        node.name.clone_from(n);
    }
    if let Some(os) = profile.os {
        node.os = os;
    }
    if let Some(arch) = profile.arch {
        node.arch = arch;
    }
    if let Some(c) = profile.cpu_cores {
        node.resources.cpu_cores = c.max(1);
    }
    if let Some(m) = profile.memory_total_bytes {
        node.resources.memory_total_bytes = m;
        node.resources.memory_used_bytes = node.resources.memory_used_bytes.min(m);
    }
    if let Some(gpus) = &profile.gpus {
        node.resources.gpus =
            gpus.iter().map(|g| GpuInfo { name: g.name.clone(), vram_bytes: g.vram_bytes }).collect();
    }
    if let Some(tools) = &profile.tools {
        node.tools.clone_from(tools);
    }
    if let Some(shims) = &profile.tool_shims {
        for tool in shims.keys() {
            if !node.tools.contains(tool) {
                node.tools.push(tool.clone());
            }
        }
    }
    if let Some(models) = &profile.models {
        node.local_models.clone_from(models);
    }
    node
}

/// Labels of a profile (empty without one).
#[must_use]
pub fn profile_labels(profile: Option<&NodeProfile>) -> BTreeMap<String, String> {
    profile.and_then(|p| p.labels.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{Arch, NodeId, NodeResources, NodeStatus, OsKind};

    fn detected() -> Node {
        Node {
            id: NodeId::new(),
            name: "win".into(),
            hostname: "win".into(),
            os: OsKind::Windows,
            os_version: "11".into(),
            arch: Arch::X64,
            core_version: "0.1.0".into(),
            status: NodeStatus::Online,
            resources: NodeResources {
                cpu_cores: 16,
                cpu_usage_percent: 3.0,
                memory_total_bytes: 64 << 30,
                memory_used_bytes: 20 << 30,
                disk_total_bytes: 1 << 40,
                disk_free_bytes: 1 << 39,
                gpus: Vec::new(),
            },
            tools: vec!["git".into(), "cargo".into()],
            local_models: Vec::new(),
            is_local: true,
            last_heartbeat_at: None,
        }
    }

    const MAC: &str = r#"{"name":"mac","os":"MacOs","arch":"Arm64","cpuCores":10,"memoryTotalBytes":17179869184,
        "tools":["git","node"],"labels":{"site":"lab"},"toolShims":{"xcodebuild":"node fake-xcodebuild.mjs"},
        "gpus":[{"name":"Apple M2","vendor":"Apple","vramBytes":null,"driverVersion":null,"source":"profile"}],
        "models":["llama3.1:8b"],"maxConcurrentJobs":2}"#;

    #[test]
    fn profile_override_applies() {
        let env = EnvSource::from_pairs([(PROFILE_ENV, MAC)]);
        let p = load_profile(&env).expect("profile");
        let n = apply_profile(detected(), &p);
        assert_eq!(n.name, "mac");
        assert_eq!(n.hostname, "win", "hostname is not overridden");
        assert_eq!(n.os, OsKind::MacOs);
        assert_eq!(n.arch, Arch::Arm64);
        assert_eq!(n.resources.cpu_cores, 10);
        assert_eq!(n.resources.memory_total_bytes, 16 << 30);
        assert_eq!(n.resources.memory_used_bytes, 16 << 30, "used clamped to total");
        assert_eq!(n.resources.gpus.len(), 1);
        assert_eq!(n.tools, vec!["git".to_owned(), "node".into(), "xcodebuild".into()]);
        assert_eq!(n.local_models, vec!["llama3.1:8b".to_owned()]);
        assert_eq!(profile_labels(Some(&p)).get("site").map(String::as_str), Some("lab"));
        assert_eq!(p.max_concurrent_jobs, Some(2));
    }

    #[test]
    fn empty_profile_keeps_detection_and_file_form_works() {
        let n = apply_profile(detected(), &NodeProfile::default());
        assert_eq!(n, detected_with_id(&n));
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("p.json");
        std::fs::write(&f, r#"{"os":"Linux","gpus":[{"name":"RTX 4090","vendor":"NVIDIA","vramBytes":25769803776,"driverVersion":null,"source":"profile"}]}"#).unwrap();
        let p = load_profile(&EnvSource::from_pairs([(PROFILE_ENV, format!("@{}", f.display()))])).unwrap();
        let n = apply_profile(detected(), &p);
        assert_eq!(n.os, OsKind::Linux);
        assert_eq!(n.resources.gpus[0].vram_bytes, Some(24 << 30));
        assert_eq!(n.tools, vec!["git".to_owned(), "cargo".into()]);
        assert!(load_profile(&EnvSource::from_pairs([(PROFILE_ENV, "{not json")])).is_none());
        assert!(load_profile(&EnvSource::from_pairs([(PROFILE_ENV, "@/no/such/file.json")])).is_none());
        assert!(load_profile(&EnvSource::from_pairs([(PROFILE_ENV, " ")])).is_none());
    }

    fn detected_with_id(n: &Node) -> Node {
        Node { id: n.id, ..detected() }
    }
}

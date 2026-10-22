//! Worker-side job execution (Phase 6, node-protocol.md §6–7). **Owner: WP-C.**
//!
//! The worker core receives `jobs.offer`, verifies the signed policy snapshot,
//! materializes the job worktree under `ORKESTR_HOME/jobs/<id>`, runs the
//! command (tool shims apply) or agent, streams `JobProgress`, and checkpoints
//! at safe points / on `jobs.drain`. Every call is fenced by the lease epoch.

use async_trait::async_trait;
use orkestr_domain::{JobId, LeaseEpoch};
use std::collections::BTreeMap;

use orkestr_protocol::methods::jobs::{
    JobCheckpoint, JobOffer, JobProgressPayload, PolicySnapshot, SideEffectKind, SideEffectRecord,
};
use orkestr_protocol::methods::nodes::NodeProfile;
use tokio::sync::{mpsc, watch};

/// Job errors.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// Unknown job.
    #[error("job {0} not found")]
    NotFound(JobId),
    /// Epoch is not the current one for the job (fencing, SR-81).
    #[error("stale epoch {got} for job {job} (current {current})")]
    StaleEpoch {
        /// Job.
        job: JobId,
        /// Epoch sent.
        got: LeaseEpoch,
        /// Current epoch.
        current: LeaseEpoch,
    },
    /// Offer rejected (capacity, trust, policy signature).
    #[error("offer rejected: {0}")]
    Rejected(String),
    /// Execution failure.
    #[error("job failed: {0}")]
    Failed(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Control handle given to an executor for one job.
#[derive(Debug, Clone)]
pub struct JobControl {
    /// Progress sink (published as `JobProgress` events on topic `jobs`).
    pub progress: mpsc::UnboundedSender<JobProgressPayload>,
    /// Flips to `true` when the job must stop (cancel / emergency stop).
    pub cancelled: watch::Receiver<bool>,
}

/// Runs jobs on the worker.
#[async_trait]
pub trait JobExecutor: Send + Sync {
    /// Starts an accepted offer (returns once started; progress via `ctl`).
    async fn start(&self, offer: JobOffer, ctl: JobControl) -> Result<(), JobError>;
    /// Runs to the next safe point, stops, and returns the final checkpoint.
    async fn drain(&self, job: JobId, epoch: LeaseEpoch) -> Result<JobCheckpoint, JobError>;
    /// Cancels a job (`immediate` kills instead of stopping at a safe point).
    async fn cancel(&self, job: JobId, epoch: LeaseEpoch, immediate: bool) -> Result<(), JobError>;
}

// --- pure helpers shared by the worker host and the coordinator (WP-C) --------

/// Domain-separation prefix of the signed policy snapshot.
pub const POLICY_SIG_CONTEXT: &str = "orkestr-job-policy-v1";

/// Bytes the coordinator signs (P-256 CIK) for a job's policy snapshot: the
/// snapshot with `sig` cleared, bound to the job and the lease epoch so a
/// snapshot cannot be replayed onto another job or an older lease.
#[must_use]
pub fn policy_signing_bytes(job: JobId, epoch: LeaseEpoch, policy: &PolicySnapshot) -> Vec<u8> {
    let mut unsigned = policy.clone();
    unsigned.sig.clear();
    let body = serde_json::to_string(&unsigned).unwrap_or_default();
    format!("{POLICY_SIG_CONTEXT}\n{job}\n{}\n{body}", epoch.0).into_bytes()
}

/// Fencing check for a job-side call carrying `got` against the current epoch.
///
/// # Errors
/// [`JobError::StaleEpoch`] when `got != current`.
pub fn check_epoch(job: JobId, current: LeaseEpoch, got: LeaseEpoch) -> Result<(), JobError> {
    if got == current {
        Ok(())
    } else {
        Err(JobError::StaleEpoch { job, got, current })
    }
}

/// Program → replacement command line from the node profile's `toolShims`
/// (e.g. `xcodebuild → node scripts/smoke/fixtures/fake-xcodebuild.mjs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolShims {
    map: BTreeMap<String, Vec<String>>,
}

/// Splits a shim command line into program + arguments: whitespace separates
/// words; `"…"` / `'…'` group (paths with spaces); inside double quotes `\"`
/// and `\\` are escapes (JSON-quoted paths); elsewhere a backslash is literal
/// (Windows paths).
#[must_use]
pub fn split_command_line(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('"') => match c {
                '"' => quote = None,
                '\\' if matches!(chars.peek(), Some('"' | '\\')) => cur.push(chars.next().unwrap_or(c)),
                _ => cur.push(c),
            },
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_word = true;
                }
                c if c.is_whitespace() => {
                    if in_word {
                        out.push(std::mem::take(&mut cur));
                        in_word = false;
                    }
                }
                _ => {
                    cur.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

impl ToolShims {
    /// Shims of a profile (none without a profile).
    #[must_use]
    pub fn from_profile(profile: Option<&NodeProfile>) -> Self {
        let mut map = BTreeMap::new();
        if let Some(shims) = profile.and_then(|p| p.tool_shims.as_ref()) {
            for (tool, cmd) in shims {
                let parts = split_command_line(cmd);
                if !parts.is_empty() {
                    map.insert(tool.clone(), parts);
                }
            }
        }
        Self { map }
    }

    /// Explicit shims (tests).
    #[must_use]
    pub fn from_map(map: BTreeMap<String, Vec<String>>) -> Self {
        Self { map }
    }

    /// Resolves `program args…` through the shims (unchanged when unshimmed).
    #[must_use]
    pub fn resolve(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        match self.map.get(program) {
            Some(parts) => {
                let mut full: Vec<String> = parts[1..].to_vec();
                full.extend(args.iter().cloned());
                (parts[0].clone(), full)
            }
            None => (program.to_owned(), args.to_vec()),
        }
    }

    /// Whether `program` is shimmed.
    #[must_use]
    pub fn is_shimmed(&self, program: &str) -> bool {
        self.map.contains_key(program)
    }
}

const NON_IDEMPOTENT_WORDS: &[&str] = &[
    "deploy",
    "publish",
    "release",
    "push",
    "upload",
    "send",
    "migrate",
    "drop",
    "delete",
    "pay",
    "charge",
    "kubectl",
    "terraform",
    "helm",
];
const IDEMPOTENT_PROGRAMS: &[&str] = &[
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "node",
    "go",
    "make",
    "cmake",
    "gradle",
    "mvn",
    "xcodebuild",
    "swift",
    "dotnet",
    "python",
    "python3",
    "pytest",
    "tsc",
    "vitest",
    "jest",
    "echo",
    "cmd",
    "sh",
    "bash",
    "pwsh",
    "powershell",
];

/// Conservative side-effect classification of an action / command line
/// (node-protocol.md §7.2): deploy/publish/push-like words → non-idempotent;
/// known build/test tools → idempotent; anything else → unknown.
#[must_use]
pub fn classify_side_effect(program: &str, args: &[String]) -> SideEffectKind {
    let words = std::iter::once(program).chain(args.iter().map(String::as_str)).map(str::to_ascii_lowercase);
    for w in words {
        if NON_IDEMPOTENT_WORDS.iter().any(|n| w.split(|c: char| !c.is_ascii_alphanumeric()).any(|p| p == *n)) {
            return SideEffectKind::NonIdempotent;
        }
    }
    let base =
        std::path::Path::new(program).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if IDEMPOTENT_PROGRAMS.contains(&base.as_str()) {
        SideEffectKind::Idempotent
    } else {
        SideEffectKind::Unknown
    }
}

/// Whether re-executing past `effects` needs a user decision (any unknown or
/// non-idempotent action, §7.2).
#[must_use]
pub fn needs_user_decision(effects: &[SideEffectRecord]) -> bool {
    effects.iter().any(|e| e.kind != SideEffectKind::Idempotent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_command_lines_split_with_quotes() {
        assert_eq!(split_command_line("node  fake.mjs -x"), ["node", "fake.mjs", "-x"]);
        // JSON-quoted Windows path with spaces (the acceptance fixtures do this)
        assert_eq!(
            split_command_line(r#"node "C:\\Program Files\\o k\\fake.mjs" --a"#),
            ["node", r"C:\Program Files\o k\fake.mjs", "--a"]
        );
        assert_eq!(split_command_line(r"node C:\x\y.mjs"), ["node", r"C:\x\y.mjs"]);
        assert_eq!(split_command_line(r#"sh 'a b' """#), ["sh", "a b", ""]);
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn shims_resolve_and_classify() {
        let profile = NodeProfile {
            tool_shims: Some(BTreeMap::from([("xcodebuild".to_owned(), "node fake.mjs".to_owned())])),
            ..NodeProfile::default()
        };
        let shims = ToolShims::from_profile(Some(&profile));
        let (p, a) = shims.resolve("xcodebuild", &["-scheme".into(), "App".into()]);
        assert_eq!(p, "node");
        assert_eq!(a, vec!["fake.mjs", "-scheme", "App"]);
        assert_eq!(shims.resolve("cargo", &[]).0, "cargo");
        assert_eq!(classify_side_effect("cargo", &["build".into()]), SideEffectKind::Idempotent);
        assert_eq!(classify_side_effect("./deploy.sh", &[]), SideEffectKind::NonIdempotent);
        assert_eq!(classify_side_effect("git", &["push".into()]), SideEffectKind::NonIdempotent);
        assert_eq!(classify_side_effect("curl", &[]), SideEffectKind::Unknown);
    }

    #[test]
    fn epoch_fencing_and_signing_bytes() {
        let job = JobId::new();
        assert!(check_epoch(job, LeaseEpoch(2), LeaseEpoch(2)).is_ok());
        assert!(matches!(check_epoch(job, LeaseEpoch(2), LeaseEpoch(1)), Err(JobError::StaleEpoch { .. })));
        let mut p = PolicySnapshot { approval_mode: "ask".into(), ..PolicySnapshot::default() };
        let a = policy_signing_bytes(job, LeaseEpoch(1), &p);
        p.sig = "x".into();
        assert_eq!(a, policy_signing_bytes(job, LeaseEpoch(1), &p), "sig is excluded");
        assert_ne!(a, policy_signing_bytes(job, LeaseEpoch(2), &p), "epoch is bound");
    }
}

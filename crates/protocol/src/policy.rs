//! Per-method access policy for remote callers (Phase 4, ADR-0030).
//!
//! Every RPC method in [`crate::methods::registry()`] has a [`MethodPolicy`]:
//! the minimum [`Role`] a remote device needs, whether the method is
//! reachable only from the local machine, whether a fresh passkey assertion
//! (step-up) is required, and the rate-limit bucket it draws from.
//!
//! [`policy_for`] returns `None` for unknown methods; the dispatcher treats
//! `None` as **forbidden** (fail closed). Local callers (`Principal::Local` in
//! the core) are not restricted by these policies.
//!
//! The table is built from explicit per-method overrides plus per-area
//! defaults keyed on the verb (read-like verbs → `Viewer`/`Read`, everything
//! else → `Operator`/`Write`). Anything that touches host credentials, the
//! host desktop or the remote-access configuration itself is `local_only`.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Role of a paired remote device (ordered: `Viewer < Operator < Admin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Role {
    /// Read-only access.
    Viewer,
    /// Can run tasks, write files, resolve approvals.
    Operator,
    /// Can also manage devices and security settings.
    Admin,
}

/// Whether a fresh passkey assertion is required before the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StepUp {
    /// Never required.
    Never,
    /// Always required.
    Always,
    /// Required when the concrete call is dangerous (e.g. a dangerous
    /// approval, a destructive VCS/rollback operation); the handler decides.
    WhenDangerous,
}

/// Rate-limit bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RateClass {
    /// Cheap reads.
    Read,
    /// Mutations.
    Write,
    /// Calls that start costly work (agents, LLM calls, builds).
    Expensive,
    /// Authentication / pairing / step-up attempts.
    Auth,
    /// Emergency controls: never throttled below a usable floor.
    Emergency,
    /// Node protocol traffic from a coordinator (heartbeats, job control, blob
    /// chunks; Phase 6): high budget, coordinator principals only.
    Node,
}

/// Access policy of one method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MethodPolicy {
    /// Minimum role of a remote caller.
    pub min_role: Role,
    /// Only callable from the local machine.
    pub local_only: bool,
    /// Step-up requirement.
    pub step_up: StepUp,
    /// Rate-limit bucket.
    pub rate_class: RateClass,
    /// Worker-side node protocol method: only a paired **coordinator** principal
    /// may call it (Phase 6, ADR-0033); the handler also checks the job's trust need.
    #[serde(default)]
    pub coordinator_only: bool,
}

impl MethodPolicy {
    const fn new(min_role: Role, step_up: StepUp, rate_class: RateClass) -> Self {
        Self { min_role, local_only: false, step_up, rate_class, coordinator_only: false }
    }
    const fn read() -> Self {
        Self::new(Role::Viewer, StepUp::Never, RateClass::Read)
    }
    const fn write() -> Self {
        Self::new(Role::Operator, StepUp::Never, RateClass::Write)
    }
    const fn expensive() -> Self {
        Self::new(Role::Operator, StepUp::Never, RateClass::Expensive)
    }
    const fn dangerous() -> Self {
        Self::new(Role::Operator, StepUp::WhenDangerous, RateClass::Write)
    }
    const fn admin_step_up() -> Self {
        Self::new(Role::Admin, StepUp::Always, RateClass::Write)
    }
    /// Stopping work is always safe: every role may call it (remote-access.md §3).
    const fn emergency() -> Self {
        Self::new(Role::Viewer, StepUp::Never, RateClass::Emergency)
    }
    const fn local() -> Self {
        Self {
            min_role: Role::Admin,
            local_only: true,
            step_up: StepUp::Always,
            rate_class: RateClass::Write,
            coordinator_only: false,
        }
    }
    /// Worker side of the node protocol (called by the coordinator core).
    const fn worker() -> Self {
        Self {
            min_role: Role::Operator,
            local_only: false,
            step_up: StepUp::Never,
            rate_class: RateClass::Node,
            coordinator_only: true,
        }
    }
}

/// Verbs that only read state.
const READ_VERBS: &[&str] = &[
    "list",
    "get",
    "status",
    "health",
    "info",
    "graph",
    "history",
    "diff",
    "log",
    "show",
    "branches",
    "worktrees",
    "stat",
    "read",
    "search",
    "find",
    "summary",
    "byDimension",
    "timeseries",
    "records",
    "budgetsList",
    "budgetsCheck",
    "effectiveRules",
    "decisionsList",
    "decisionsGet",
    "knowledgeList",
    "knowledgeSearch",
    "checks",
    "resumeSummary",
    "resources",
    "transcript",
    "handoffList",
    "stats",
    "config",
    "runs",
    "getRun",
    "gates",
    "getProfile",
    "detect",
    "evaluate",
    "map",
    "timeline",
    "step",
    "policy",
    "preview",
    "explain",
    "models",
    "agents",
    "getConfig",
    "themes",
    "device",
    "snapshot",
    "logs",
    "listExtensions",
    "commands",
    "tasks",
    "details",
    "icon",
    "readme",
    "theme",
    "iconTheme",
    "readAsset",
    "languageConfig",
    "snippets",
    "getPrefs",
    "servers",
    "sessions",
    "capabilities",
    "usage",
    "contextUsage",
    "mcpStatus",
    "kinds",
    "getManager",
    "getRouting",
    "shells",
    "mergePreview",
    "check",
    "getMode",
    "replay",
    "checkUpdate",
    "vapidKey",
    "listConflicts",
    "resume",
];

/// Explicit per-method policies (checked before area defaults).
fn explicit(method: &str) -> Option<MethodPolicy> {
    let p = match method {
        // --- remote access (Phase 4) -------------------------------------------------------
        "remote.enable" | "devices.startPairing" | "devices.approve" => MethodPolicy::local(),
        "remote.disable" | "remote.revokeAllSessions" => MethodPolicy::emergency(),
        "devices.revoke" => MethodPolicy::admin_step_up(),
        "devices.registerStepUp" => MethodPolicy::new(Role::Operator, StepUp::Never, RateClass::Auth),
        // --- sync (Phase 4) ---------------------------------------------------------------
        "sync.configure" | "sync.rotateKeys" => MethodPolicy::local(),
        "sync.now" | "sync.continueOn" | "sync.resolveConflict" => MethodPolicy::write(),
        // --- push (Phase 4) ---------------------------------------------------------------
        "push.subscribe" | "push.unsubscribe" | "push.registerNative" => {
            MethodPolicy::new(Role::Viewer, StepUp::Never, RateClass::Write)
        }
        "push.test" => MethodPolicy::write(),
        // --- multi-node (Phase 6) -------------------------------------------------------------
        "nodes.enroll" | "nodes.revoke" => MethodPolicy::admin_step_up(),
        "nodes.drain" | "nodes.setPolicy" | "jobs.submit" | "jobs.migrate" | "jobs.cancel" => MethodPolicy::write(),
        "models.import" | "models.distribute" => MethodPolicy::expensive(),
        "node.hello" | "node.heartbeat" | "jobs.offer" | "jobs.drain" | "node.jobCancel" | "node.blobHas"
        | "node.blobPut" | "node.blobGet" | "node.modelInstall" => MethodPolicy::worker(),
        // --- emergency ----------------------------------------------------------------------
        "emergency.status" => MethodPolicy::read(),
        // the audit log reveals every resource name: admins only
        "audit.list" => MethodPolicy::new(Role::Admin, StepUp::Never, RateClass::Read),
        "emergency.reset" => MethodPolicy::local(),
        // --- security-sensitive ---------------------------------------------------------------
        "providers.setCredential"
        | "providers.clearCredential"
        | "providers.create"
        | "providers.update"
        | "providers.delete" => MethodPolicy::local(),
        "permissions.set" | "permissions.delete" | "permissions.setMode" => MethodPolicy::admin_step_up(),
        "approvals.resolve" => MethodPolicy::dangerous(),
        "settings.update" => MethodPolicy::admin_step_up(),
        // host desktop / host binaries
        "files.openExternal"
        | "claudeCode.loginInTerminal"
        | "claudeCode.setBinaryPath"
        | "claudeCode.openInTerminal"
        | "nodes.power" => MethodPolicy::local(),
        // destructive
        "vcs.discard"
        | "vcs.deleteBranch"
        | "vcs.removeWorktree"
        | "vcs.merge"
        | "files.delete"
        | "snapshots.rollbackWorkspace"
        | "snapshots.rollbackTask"
        | "snapshots.restoreFile"
        | "snapshots.delete"
        | "merge.execute"
        | "projects.delete"
        | "tasks.delete" => MethodPolicy::dangerous(),
        // expensive
        "manager.chat"
        | "manager.run"
        | "manager.plan"
        | "agents.spawn"
        | "reviews.request"
        | "reviews.adversarial"
        | "consensus.run"
        | "quality.run"
        | "pipeline.start"
        | "voice.transcribe"
        | "voice.synthesize"
        | "voice.recordCommand"
        | "memory.knowledgeReindex"
        | "architecture.refresh"
        | "pm.draft"
        | "terminal.run"
        | "terminal.execLine"
        | "claudeCode.send"
        | "claudeCode.start"
        | "tasks.retry" => MethodPolicy::expensive(),
        // stopping work is always allowed to operators, never throttled
        "agents.stop" | "manager.cancel" | "quality.cancel" | "claudeCode.interrupt" | "claudeCode.stop" => {
            MethodPolicy::emergency()
        }
        _ => return None,
    };
    Some(p)
}

/// Area defaults.
fn area_default(area: &str, verb: &str) -> MethodPolicy {
    match area {
        // credentials, host processes and host-wide installs stay local
        "secrets" | "vscode" | "extensions" | "lsp" | "claudeEngine" => MethodPolicy::local(),
        // emergency stop controls
        "emergency" => MethodPolicy::emergency(),
        // worker side of the node protocol (Phase 6)
        "node" => MethodPolicy::worker(),
        _ if READ_VERBS.contains(&verb) => MethodPolicy::read(),
        _ => MethodPolicy::write(),
    }
}

fn known_methods() -> &'static HashSet<&'static str> {
    static KNOWN: OnceLock<HashSet<&'static str>> = OnceLock::new();
    KNOWN.get_or_init(|| crate::methods::registry().iter().map(|m| m.name).collect())
}

/// Policy of `method`; `None` for unknown methods (→ forbidden, fail closed).
#[must_use]
pub fn policy_for(method: &str) -> Option<MethodPolicy> {
    if !known_methods().contains(method) {
        return None;
    }
    if let Some(p) = explicit(method) {
        return Some(p);
    }
    let (area, verb) = method.split_once('.')?;
    Some(area_default(area, verb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_method_has_a_policy() {
        for m in crate::methods::registry() {
            assert!(policy_for(m.name).is_some(), "no policy for {}", m.name);
        }
    }

    #[test]
    fn unknown_methods_fail_closed() {
        assert_eq!(policy_for("nope.nothing"), None);
        assert_eq!(policy_for(""), None);
    }

    #[test]
    fn phase4_methods_follow_the_plan() {
        let local = ["remote.enable", "devices.startPairing", "devices.approve", "sync.configure", "sync.rotateKeys"];
        for m in local {
            assert!(policy_for(m).unwrap().local_only, "{m} must be local-only");
        }
        for m in ["remote.disable", "remote.revokeAllSessions", "emergency.stopAll"] {
            assert_eq!(policy_for(m).unwrap().rate_class, RateClass::Emergency, "{m}");
        }
        for m in ["remote.status", "devices.list", "sync.status", "sync.resume", "sync.listConflicts", "push.vapidKey"]
        {
            let p = policy_for(m).unwrap();
            assert_eq!(p.min_role, Role::Viewer, "{m}");
            assert!(!p.local_only, "{m}");
        }
        for m in ["sync.now", "sync.continueOn", "sync.resolveConflict", "push.test", "devices.registerStepUp"] {
            assert_eq!(policy_for(m).unwrap().min_role, Role::Operator, "{m}");
        }
        for m in ["emergency.stopAll", "agents.stop", "remote.disable"] {
            assert_eq!(policy_for(m).unwrap().min_role, Role::Viewer, "{m}: stopping is allowed to every role");
        }
        let native = policy_for("push.registerNative").unwrap();
        assert_eq!(
            (native.min_role, native.step_up, native.rate_class),
            (Role::Viewer, StepUp::Never, RateClass::Write)
        );
        assert!(!native.local_only);
        let revoke = policy_for("devices.revoke").unwrap();
        assert_eq!((revoke.min_role, revoke.step_up), (Role::Admin, StepUp::Always));
    }

    #[test]
    fn phase6_methods_follow_the_plan() {
        let worker = [
            "node.hello",
            "node.heartbeat",
            "jobs.offer",
            "jobs.drain",
            "node.jobCancel",
            "node.blobHas",
            "node.blobPut",
            "node.blobGet",
            "node.modelInstall",
        ];
        for m in worker {
            let p = policy_for(m).unwrap();
            assert!(p.coordinator_only, "{m} must be coordinator-only");
            assert!(!p.local_only, "{m}");
            assert_eq!(p.rate_class, RateClass::Node, "{m}");
        }
        for m in ["nodes.enroll", "nodes.revoke"] {
            let p = policy_for(m).unwrap();
            assert_eq!((p.min_role, p.step_up), (Role::Admin, StepUp::Always), "{m}");
        }
        let ui = [
            "nodes.drain",
            "nodes.setPolicy",
            "jobs.submit",
            "jobs.migrate",
            "jobs.cancel",
            "models.import",
            "models.distribute",
        ];
        for m in ui {
            let p = policy_for(m).unwrap();
            assert_eq!(p.min_role, Role::Operator, "{m}");
            assert!(!p.coordinator_only && !p.local_only, "{m}");
        }
        for m in ["nodes.list", "jobs.list", "jobs.get", "models.list"] {
            assert_eq!(policy_for(m).unwrap().min_role, Role::Viewer, "{m}");
        }
        // no other method is coordinator-only
        for m in crate::methods::registry() {
            assert_eq!(policy_for(m.name).unwrap().coordinator_only, worker.contains(&m.name), "{}", m.name);
        }
    }

    #[test]
    fn credentials_are_local_only() {
        for m in ["secrets.set", "secrets.list", "providers.setCredential", "providers.clearCredential"] {
            assert!(policy_for(m).unwrap().local_only, "{m}");
        }
    }

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Viewer < Role::Operator && Role::Operator < Role::Admin);
    }
}

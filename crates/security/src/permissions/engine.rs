//! The permission engine (contract §6, permission model §4–5).
//!
//! `check(actor, capability, resource, ctx) -> Decision` is **pure and
//! synchronous** (ADR-0010): it reads an immutable snapshot of the rules,
//! the emergency flags and the guard classification — no I/O except the
//! filesystem canonicalization done by path confinement.
//!
//! Resolution:
//! 0. emergency overrides (Stop All, network tools disabled, vault locked);
//! 1. path confinement (escape ⇒ hard Deny);
//! 2. dangerous-action guard (Forbidden ⇒ hard Deny, Dangerous ⇒ must ask);
//! 3. user/system actors: Allow unless the guard asks/denies (users confirm
//!    dangerous operations in Orkestr-owned flows);
//! 4. rules: narrowest scope present among matching rules wins, Deny > Ask >
//!    Allow at that scope; no matching rule ⇒ approval-mode default;
//! 5. combine: Deny ⇒ Deny; Dangerous ⇒ Ask (never rememberable, even in
//!    FullAuto and even with an explicit Allow rule); Ask ⇒ Ask; Allow.

use std::path::PathBuf;
use std::sync::Arc;

use globset::{GlobBuilder, GlobMatcher};
use orkestr_domain::{
    now, Actor, AgentId, ApprovalMode, Capability, DangerCategory, PermissionEffect, PermissionRuleRecord,
    PermissionScope, ProjectId, RiskLevel, TaskId,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::emergency::{EmergencyFlags, SharedFlags};
use crate::guard::{Action, DangerClass, DangerousActionGuard, ShellKind};
use crate::paths::confine_relative;

/// What an action touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Resource {
    /// No specific resource (capability-level check).
    None,
    /// A filesystem path (relative to the worktree root, or absolute).
    Path {
        /// Path.
        path: String,
        /// For `Delete`: recursive.
        #[serde(default)]
        recursive: bool,
    },
    /// A shell command.
    Command {
        /// Command line.
        command: String,
        /// Shell.
        shell: ShellKind,
        /// Working directory.
        #[serde(default)]
        cwd: Option<String>,
    },
    /// Git arguments (without `git`).
    Git {
        /// Arguments.
        args: Vec<String>,
    },
    /// A URL (network / browser).
    Url {
        /// URL.
        url: String,
    },
    /// A secret by name.
    Secret {
        /// Secret name.
        name: String,
    },
    /// A SQL statement.
    Sql {
        /// Statement.
        statement: String,
    },
    /// A deployment.
    Deploy {
        /// Environment.
        environment: String,
        /// Protected (staging/production).
        protected: bool,
    },
    /// Anything else (free-form, matched by rule globs).
    Other {
        /// Value.
        value: String,
    },
}

impl Resource {
    /// Human-readable one-line summary (not redacted).
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Resource::None => String::new(),
            Resource::Path { path, .. } => path.clone(),
            Resource::Command { command, .. } => command.clone(),
            Resource::Git { args } => format!("git {}", args.join(" ")),
            Resource::Url { url } => url.clone(),
            Resource::Secret { name } => name.clone(),
            Resource::Sql { statement } => statement.clone(),
            Resource::Deploy { environment, .. } => environment.clone(),
            Resource::Other { value } => value.clone(),
        }
    }
}

/// Context of a check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckContext {
    /// Project of the action.
    pub project_id: Option<ProjectId>,
    /// Acting agent.
    pub agent_id: Option<AgentId>,
    /// Task being worked on.
    pub task_id: Option<TaskId>,
    /// Approval mode (agent override already applied by the caller).
    pub approval_mode: ApprovalMode,
    /// Confinement root (the agent's worktree).
    pub worktree_root: Option<PathBuf>,
    /// The target environment is protected (staging/production).
    pub protected_environment: bool,
}

/// Why approval is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskReason {
    /// Machine code (`mode_default`, `rule`, `dangerous`, `secret_file`, …).
    pub code: String,
    /// Human message.
    pub message: String,
    /// Risk.
    pub risk: RiskLevel,
    /// Guard category when dangerous.
    pub danger: Option<DangerCategory>,
    /// Whether "remember this decision" may be offered (never for dangerous actions).
    pub rememberable: bool,
    /// Matching rule.
    pub rule_id: Option<String>,
}

/// Why an action is denied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenyReason {
    /// Machine code (`emergency_stop`, `path_escape`, `forbidden`, `rule`, …).
    pub code: String,
    /// Human message.
    pub message: String,
    /// Matching rule.
    pub rule_id: Option<String>,
    /// Hard deny: no rule or approval can override it.
    pub hard: bool,
    /// Guard category when forbidden by the guard.
    pub danger: Option<DangerCategory>,
}

/// Decision of a permission check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "reason")]
pub enum Decision {
    /// Proceed.
    Allow,
    /// Ask the user.
    Ask(AskReason),
    /// Refuse.
    Deny(DenyReason),
}

impl Decision {
    /// Whether this is `Allow`.
    #[must_use]
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
    /// Whether this is `Ask`.
    #[must_use]
    pub fn is_ask(&self) -> bool {
        matches!(self, Decision::Ask(_))
    }
    /// Whether this is `Deny`.
    #[must_use]
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny(_))
    }

    fn deny(code: &str, message: impl Into<String>, hard: bool) -> Self {
        Decision::Deny(DenyReason { code: code.into(), message: message.into(), rule_id: None, hard, danger: None })
    }
}

/// The permission engine (contract §6). Every agent tool call goes through it.
pub trait PermissionEngine: Send + Sync {
    /// Decides whether `actor` may use `capability` on `resource`.
    fn check(&self, actor: &Actor, capability: Capability, resource: &Resource, ctx: &CheckContext) -> Decision;
}

struct CompiledRule {
    record: PermissionRuleRecord,
    matcher: Option<GlobMatcher>,
}

/// Compiles a rule resource pattern (`None`/`*` = any).
pub fn compile_pattern(pattern: Option<&str>) -> Result<Option<GlobMatcher>, String> {
    match pattern.map(str::trim) {
        None | Some("") | Some("*") | Some("**") => Ok(None),
        Some(p) => GlobBuilder::new(&p.replace('\\', "/"))
            .case_insensitive(true)
            .literal_separator(false)
            .backslash_escape(false)
            .build()
            .map(|g| Some(g.compile_matcher()))
            .map_err(|e| format!("invalid resource pattern `{p}`: {e}")),
    }
}

/// The rule-based engine.
pub struct RuleEngine {
    guard: Arc<DangerousActionGuard>,
    flags: Arc<SharedFlags>,
    rules: RwLock<Arc<Vec<CompiledRule>>>,
}

impl std::fmt::Debug for RuleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleEngine").field("rules", &self.rules.read().len()).finish_non_exhaustive()
    }
}

fn risk_of(cap: Capability, ctx: &CheckContext) -> RiskLevel {
    match cap {
        Capability::Read => RiskLevel::Low,
        Capability::Write
        | Capability::Terminal
        | Capability::Git
        | Capability::DbRead
        | Capability::Network
        | Capability::Browser => RiskLevel::Medium,
        Capability::Delete | Capability::DbWrite | Capability::Docker | Capability::Ssh => RiskLevel::High,
        Capability::Deploy if ctx.protected_environment => RiskLevel::Critical,
        Capability::Deploy => RiskLevel::High,
        Capability::Secrets | Capability::ComputerControl => RiskLevel::Critical,
    }
}

fn danger_risk(cat: DangerCategory) -> RiskLevel {
    use DangerCategory as C;
    match cat {
        C::ProtectedPathDelete
        | C::ProductionChange
        | C::SecretChange
        | C::DataDestruction
        | C::PrivilegeEscalation
        | C::DiskDestruction
        | C::RemoteCodeExecution
        | C::CredentialAccess
        | C::SelfTamper => RiskLevel::Critical,
        _ => RiskLevel::High,
    }
}

fn effect_rank(e: PermissionEffect) -> u8 {
    match e {
        PermissionEffect::Allow => 0,
        PermissionEffect::Ask => 1,
        PermissionEffect::Deny => 2,
    }
}

fn scope_applies(scope: &PermissionScope, ctx: &CheckContext) -> bool {
    match scope {
        PermissionScope::Global => true,
        PermissionScope::Project { project_id } => ctx.project_id == Some(*project_id),
        PermissionScope::Agent { agent_id } => ctx.agent_id == Some(*agent_id),
        PermissionScope::Task { task_id } => ctx.task_id == Some(*task_id),
    }
}

/// Approval-mode default effect when no rule matches (permission model §5,
/// tightened by the Phase 1 assignment: Network/Deploy/Secrets/Ssh/Docker/
/// DbWrite are never granted by default, ComputerControl is denied).
#[must_use]
pub fn mode_default(mode: ApprovalMode, cap: Capability, inside_root: bool, protected_env: bool) -> PermissionEffect {
    use ApprovalMode as M;
    use PermissionEffect as E;
    match cap {
        Capability::Read => {
            if inside_root {
                E::Allow
            } else {
                E::Ask
            }
        }
        Capability::Write | Capability::Terminal | Capability::Git => match mode {
            M::Ask => E::Ask,
            M::SemiAuto | M::FullAuto => {
                if inside_root {
                    E::Allow
                } else {
                    E::Ask
                }
            }
        },
        Capability::Delete => match mode {
            M::FullAuto if inside_root => E::Allow,
            _ => E::Ask,
        },
        Capability::DbRead => match mode {
            M::SemiAuto | M::FullAuto if !protected_env => E::Allow,
            _ => E::Ask,
        },
        Capability::Browser => match mode {
            M::FullAuto => E::Allow,
            _ => E::Ask,
        },
        Capability::Network
        | Capability::Deploy
        | Capability::Secrets
        | Capability::Ssh
        | Capability::Docker
        | Capability::DbWrite => E::Ask,
        Capability::ComputerControl => E::Deny,
    }
}

impl RuleEngine {
    /// Creates an engine with no rules.
    #[must_use]
    pub fn new(guard: Arc<DangerousActionGuard>, flags: Arc<SharedFlags>) -> Self {
        Self { guard, flags, rules: RwLock::new(Arc::new(Vec::new())) }
    }

    /// The guard.
    #[must_use]
    pub fn guard(&self) -> &DangerousActionGuard {
        &self.guard
    }

    /// Current emergency flags.
    #[must_use]
    pub fn flags(&self) -> EmergencyFlags {
        self.flags.get()
    }

    /// Replaces the rule snapshot. Invalid patterns are skipped (logged);
    /// [`compile_pattern`] validates them before they are stored.
    pub fn set_rules(&self, records: Vec<PermissionRuleRecord>) {
        let compiled: Vec<CompiledRule> = records
            .into_iter()
            .filter_map(|record| match compile_pattern(record.rule.resource.as_deref()) {
                Ok(matcher) => Some(CompiledRule { record, matcher }),
                Err(e) => {
                    tracing::warn!(rule = %record.id, error = %e, "skipping permission rule with invalid pattern");
                    None
                }
            })
            .collect();
        *self.rules.write() = Arc::new(compiled);
    }

    /// Number of rules in the snapshot.
    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.read().len()
    }

    /// Effective approval mode after emergency caps.
    #[must_use]
    pub fn effective_mode(&self, mode: ApprovalMode) -> ApprovalMode {
        if self.flags.get().full_auto_disabled && mode == ApprovalMode::FullAuto {
            ApprovalMode::SemiAuto
        } else {
            mode
        }
    }
}

impl PermissionEngine for RuleEngine {
    #[allow(clippy::too_many_lines)]
    fn check(&self, actor: &Actor, capability: Capability, resource: &Resource, ctx: &CheckContext) -> Decision {
        let flags = self.flags.get();
        let is_ai = matches!(actor, Actor::Agent(_) | Actor::Manager);

        // 0. Emergency overrides.
        if is_ai && flags.all_stopped {
            return Decision::deny("emergency_stop", "Stop All AI is active", true);
        }
        if is_ai && flags.network_tools_disabled && matches!(capability, Capability::Network | Capability::Browser) {
            return Decision::deny(
                "emergency_network_disabled",
                "network tools are disabled by an emergency control",
                true,
            );
        }
        if flags.vault_locked && capability == Capability::Secrets {
            return Decision::deny("vault_locked", "the secret vault is locked", true);
        }
        let mode = self.effective_mode(ctx.approval_mode);

        // 1–2. Confinement + guard classification; canonical resource key.
        let mut effective_cap = capability;
        let mut inside_root = ctx.worktree_root.is_some();
        let (key, class) = match resource {
            Resource::Path { path, recursive } => {
                let key = match &ctx.worktree_root {
                    Some(root) => match confine_relative(root, path) {
                        Ok(rel) => rel,
                        Err(e) => {
                            // Users may act outside the worktree (they still get guard checks below).
                            if is_ai {
                                return Decision::deny("path_escape", e.to_string(), true);
                            }
                            inside_root = false;
                            path.replace('\\', "/")
                        }
                    },
                    None => {
                        inside_root = false;
                        path.replace('\\', "/")
                    }
                };
                let action = match capability {
                    Capability::Read => Action::ReadFile { path: path.clone() },
                    Capability::Delete => Action::DeletePath { path: path.clone(), recursive: *recursive },
                    _ => Action::WriteFile { path: path.clone() },
                };
                if capability == Capability::Read && DangerousActionGuard::is_secret_file(path) {
                    effective_cap = Capability::Secrets;
                }
                (key, self.guard.classify_action(&action))
            }
            Resource::Command { command, shell, cwd } => {
                if let (Some(root), Some(cwd)) = (&ctx.worktree_root, cwd) {
                    if let Err(e) = confine_relative(root, cwd) {
                        if is_ai {
                            return Decision::deny("path_escape", format!("working directory: {e}"), true);
                        }
                        inside_root = false;
                    }
                }
                (command.clone(), self.guard.classify_in(command, *shell, ctx.worktree_root.as_deref()))
            }
            Resource::Git { args } => (args.join(" "), self.guard.classify_action(&Action::Git { args: args.clone() })),
            Resource::Sql { statement } => {
                (statement.clone(), self.guard.classify_action(&Action::Sql { statement: statement.clone() }))
            }
            Resource::Deploy { environment, protected } => (
                environment.clone(),
                self.guard.classify_action(&Action::Deploy {
                    environment: environment.clone(),
                    protected: *protected || ctx.protected_environment,
                }),
            ),
            Resource::Url { url } => (url.clone(), DangerClass::Safe),
            Resource::Secret { name } => (name.clone(), DangerClass::Safe),
            Resource::Other { value } => (value.clone(), DangerClass::Safe),
            Resource::None => (String::new(), DangerClass::Safe),
        };
        let must_ask = match class {
            DangerClass::Forbidden { category, reason } => {
                return Decision::Deny(DenyReason {
                    code: "forbidden".into(),
                    message: reason,
                    rule_id: None,
                    hard: true,
                    danger: Some(category),
                });
            }
            DangerClass::Dangerous { category, reason } => Some((category, reason)),
            DangerClass::Safe => None,
        };

        // 3. Users / the system: confirmation only for dangerous actions.
        if !is_ai {
            return match must_ask {
                Some((category, reason)) => Decision::Ask(AskReason {
                    code: "dangerous".into(),
                    message: reason,
                    risk: danger_risk(category),
                    danger: Some(category),
                    rememberable: false,
                    rule_id: None,
                }),
                None => Decision::Allow,
            };
        }

        // 4. Rules.
        let snapshot = Arc::clone(&self.rules.read());
        let t = now();
        let matching: Vec<&CompiledRule> = snapshot
            .iter()
            .filter(|r| {
                (r.record.rule.capability == effective_cap || r.record.rule.capability == capability)
                    && r.record.expires_at.is_none_or(|e| e > t)
                    && scope_applies(&r.record.rule.scope, ctx)
                    && r.matcher.as_ref().is_none_or(|m| m.is_match(&key))
            })
            .collect();
        let (effect, rule_id, from_rule) =
            if let Some(narrowest) = matching.iter().map(|r| r.record.rule.scope.specificity()).max() {
                let winner = matching
                    .iter()
                    .filter(|r| r.record.rule.scope.specificity() == narrowest)
                    .max_by_key(|r| effect_rank(r.record.rule.effect))
                    .map(|r| (r.record.rule.effect, r.record.id.clone()));
                match winner {
                    Some((e, id)) => (e, Some(id), true),
                    None => (PermissionEffect::Ask, None, false),
                }
            } else {
                (mode_default(mode, effective_cap, inside_root, ctx.protected_environment), None, false)
            };

        // P6: ComputerControl needs an explicit rule and is then still asked each time.
        if effective_cap == Capability::ComputerControl {
            return match (from_rule, effect) {
                (true, PermissionEffect::Allow | PermissionEffect::Ask) => Decision::Ask(AskReason {
                    code: "computer_control".into(),
                    message: "computer control always requires approval".into(),
                    risk: RiskLevel::Critical,
                    danger: None,
                    rememberable: false,
                    rule_id,
                }),
                _ => Decision::Deny(DenyReason {
                    code: if from_rule { "rule".into() } else { "not_granted".into() },
                    message: "computer control is never granted by default".into(),
                    rule_id,
                    hard: false,
                    danger: None,
                }),
            };
        }

        // 5. Combine.
        if effect == PermissionEffect::Deny {
            return Decision::Deny(DenyReason {
                code: if from_rule { "rule".into() } else { "not_granted".into() },
                message: if from_rule {
                    format!("denied by permission rule for {effective_cap:?}")
                } else {
                    format!("{effective_cap:?} is not granted")
                },
                rule_id,
                hard: false,
                danger: None,
            });
        }
        if let Some((category, reason)) = must_ask {
            return Decision::Ask(AskReason {
                code: "dangerous".into(),
                message: reason,
                risk: danger_risk(category),
                danger: Some(category),
                rememberable: false,
                rule_id,
            });
        }
        if effect == PermissionEffect::Ask {
            let (code, message) = if from_rule {
                ("rule", format!("a permission rule requires approval for {effective_cap:?}"))
            } else if effective_cap == Capability::Secrets && capability == Capability::Read {
                ("secret_file", "reading a secrets file requires approval".to_owned())
            } else {
                ("mode_default", format!("{effective_cap:?} requires approval in {mode:?} mode"))
            };
            return Decision::Ask(AskReason {
                code: code.into(),
                message,
                risk: risk_of(effective_cap, ctx),
                danger: None,
                rememberable: true,
                rule_id,
            });
        }
        Decision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::GuardConfig;
    use orkestr_domain::PermissionRule;

    struct Fx {
        engine: RuleEngine,
        flags: Arc<SharedFlags>,
        root: tempfile::TempDir,
        project: ProjectId,
        agent: AgentId,
        task: TaskId,
    }

    fn fx() -> Fx {
        let flags = Arc::new(SharedFlags::default());
        let engine =
            RuleEngine::new(Arc::new(DangerousActionGuard::with_config(GuardConfig::default())), Arc::clone(&flags));
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        Fx { engine, flags, root, project: ProjectId::new(), agent: AgentId::new(), task: TaskId::new() }
    }

    impl Fx {
        fn ctx(&self, mode: ApprovalMode) -> CheckContext {
            CheckContext {
                project_id: Some(self.project),
                agent_id: Some(self.agent),
                task_id: Some(self.task),
                approval_mode: mode,
                worktree_root: Some(self.root.path().to_path_buf()),
                protected_environment: false,
            }
        }
        fn agent(&self) -> Actor {
            Actor::Agent(self.agent)
        }
        fn rule(
            &self,
            cap: Capability,
            effect: PermissionEffect,
            resource: Option<&str>,
            scope: PermissionScope,
        ) -> PermissionRuleRecord {
            PermissionRuleRecord {
                id: uuid_like(),
                rule: PermissionRule { capability: cap, effect, resource: resource.map(str::to_owned), scope },
                note: None,
                created_at: now(),
                expires_at: None,
            }
        }
        fn set_flags(&self, f: impl FnOnce(&mut EmergencyFlags)) {
            let mut cur = self.flags.get();
            f(&mut cur);
            self.flags.update(|f| *f = cur);
        }
    }

    fn uuid_like() -> String {
        orkestr_domain::AuditEventId::new().to_string()
    }

    fn path(p: &str) -> Resource {
        Resource::Path { path: p.into(), recursive: false }
    }

    fn cmd(c: &str) -> Resource {
        Resource::Command { command: c.into(), shell: ShellKind::Bash, cwd: None }
    }

    #[test]
    fn mode_defaults_table() {
        let f = fx();
        let a = f.agent();
        for mode in [ApprovalMode::Ask, ApprovalMode::SemiAuto, ApprovalMode::FullAuto] {
            let ctx = f.ctx(mode);
            assert!(f.engine.check(&a, Capability::Read, &path("src/a.rs"), &ctx).is_allow(), "{mode:?} read");
            for cap in [
                Capability::Network,
                Capability::Deploy,
                Capability::Secrets,
                Capability::Ssh,
                Capability::Docker,
                Capability::DbWrite,
            ] {
                assert!(
                    f.engine.check(&a, cap, &Resource::None, &ctx).is_ask(),
                    "{mode:?} {cap:?} never granted by default"
                );
            }
            assert!(
                f.engine.check(&a, Capability::ComputerControl, &Resource::None, &ctx).is_deny(),
                "{mode:?} computer control"
            );
        }
        let ask = f.ctx(ApprovalMode::Ask);
        assert!(f.engine.check(&a, Capability::Write, &path("src/a.rs"), &ask).is_ask());
        assert!(f.engine.check(&a, Capability::Terminal, &cmd("cargo test"), &ask).is_ask());
        assert!(f.engine.check(&a, Capability::Git, &Resource::Git { args: vec!["commit".into()] }, &ask).is_ask());
        for mode in [ApprovalMode::SemiAuto, ApprovalMode::FullAuto] {
            let ctx = f.ctx(mode);
            assert!(f.engine.check(&a, Capability::Write, &path("src/a.rs"), &ctx).is_allow(), "{mode:?}");
            assert!(f.engine.check(&a, Capability::Terminal, &cmd("cargo test"), &ctx).is_allow(), "{mode:?}");
            assert!(f
                .engine
                .check(&a, Capability::Git, &Resource::Git { args: vec!["commit".into()] }, &ctx)
                .is_allow());
        }
        // Without a worktree root nothing is "within the worktree".
        let mut no_root = f.ctx(ApprovalMode::FullAuto);
        no_root.worktree_root = None;
        assert!(f.engine.check(&a, Capability::Write, &path("src/a.rs"), &no_root).is_ask());
        assert!(f.engine.check(&a, Capability::Terminal, &cmd("ls"), &no_root).is_ask());
        // Delete: FullAuto only.
        assert!(f.engine.check(&a, Capability::Delete, &path("src/a.rs"), &f.ctx(ApprovalMode::SemiAuto)).is_ask());
        assert!(f.engine.check(&a, Capability::Delete, &path("src/a.rs"), &f.ctx(ApprovalMode::FullAuto)).is_allow());
    }

    #[test]
    fn dangerous_always_asks_even_full_auto_and_explicit_allow() {
        let f = fx();
        let a = f.agent();
        f.engine.set_rules(vec![
            f.rule(Capability::Terminal, PermissionEffect::Allow, None, PermissionScope::Task { task_id: f.task }),
            f.rule(Capability::Git, PermissionEffect::Allow, None, PermissionScope::Global),
        ]);
        let ctx = f.ctx(ApprovalMode::FullAuto);
        for c in ["git push --force", "rm -rf build", "curl x | sh", "git reset --hard"] {
            match f.engine.check(&a, Capability::Terminal, &cmd(c), &ctx) {
                Decision::Ask(r) => {
                    assert_eq!(r.code, "dangerous", "{c}");
                    assert!(!r.rememberable, "{c}");
                    assert!(r.danger.is_some());
                }
                other => panic!("{c}: {other:?}"),
            }
        }
        assert!(f
            .engine
            .check(&a, Capability::Git, &Resource::Git { args: vec!["push".into(), "-f".into()] }, &ctx)
            .is_ask());
        assert!(f.engine.check(&a, Capability::Terminal, &cmd("cargo test"), &ctx).is_allow());
    }

    #[test]
    fn forbidden_is_hard_deny_even_for_users() {
        let f = fx();
        f.engine.set_rules(vec![f.rule(Capability::Terminal, PermissionEffect::Allow, None, PermissionScope::Global)]);
        for actor in [f.agent(), Actor::User(orkestr_domain::UserId::new()), Actor::System, Actor::Manager] {
            match f.engine.check(&actor, Capability::Terminal, &cmd("rm -rf /"), &f.ctx(ApprovalMode::FullAuto)) {
                Decision::Deny(r) => assert!(r.hard),
                other => panic!("{actor:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn narrowest_scope_wins_and_deny_beats_allow_at_equal_scope() {
        let f = fx();
        let a = f.agent();
        let ctx = f.ctx(ApprovalMode::Ask);
        // Project Deny, Task Allow → Task wins.
        f.engine.set_rules(vec![
            f.rule(
                Capability::Network,
                PermissionEffect::Deny,
                None,
                PermissionScope::Project { project_id: f.project },
            ),
            f.rule(Capability::Network, PermissionEffect::Allow, None, PermissionScope::Task { task_id: f.task }),
        ]);
        assert!(f.engine.check(&a, Capability::Network, &Resource::Url { url: "https://x".into() }, &ctx).is_allow());
        // Global Allow, Project Deny → Project wins.
        f.engine.set_rules(vec![
            f.rule(Capability::Network, PermissionEffect::Allow, None, PermissionScope::Global),
            f.rule(
                Capability::Network,
                PermissionEffect::Deny,
                None,
                PermissionScope::Project { project_id: f.project },
            ),
        ]);
        match f.engine.check(&a, Capability::Network, &Resource::None, &ctx) {
            Decision::Deny(r) => {
                assert_eq!(r.code, "rule");
                assert!(!r.hard);
                assert!(r.rule_id.is_some());
            }
            other => panic!("{other:?}"),
        }
        // Same scope: Deny beats Allow, Ask beats Allow.
        f.engine.set_rules(vec![
            f.rule(Capability::Network, PermissionEffect::Allow, None, PermissionScope::Agent { agent_id: f.agent }),
            f.rule(Capability::Network, PermissionEffect::Deny, None, PermissionScope::Agent { agent_id: f.agent }),
        ]);
        assert!(f.engine.check(&a, Capability::Network, &Resource::None, &ctx).is_deny());
        f.engine.set_rules(vec![
            f.rule(Capability::Network, PermissionEffect::Allow, None, PermissionScope::Agent { agent_id: f.agent }),
            f.rule(Capability::Network, PermissionEffect::Ask, None, PermissionScope::Agent { agent_id: f.agent }),
        ]);
        assert!(f.engine.check(&a, Capability::Network, &Resource::None, &ctx).is_ask());
        // Rules for other agents/projects/tasks don't apply.
        f.engine.set_rules(vec![
            f.rule(
                Capability::Network,
                PermissionEffect::Allow,
                None,
                PermissionScope::Agent { agent_id: AgentId::new() },
            ),
            f.rule(
                Capability::Network,
                PermissionEffect::Allow,
                None,
                PermissionScope::Project { project_id: ProjectId::new() },
            ),
            f.rule(
                Capability::Network,
                PermissionEffect::Allow,
                None,
                PermissionScope::Task { task_id: TaskId::new() },
            ),
        ]);
        assert!(f.engine.check(&a, Capability::Network, &Resource::None, &ctx).is_ask());
    }

    #[test]
    fn resource_patterns_and_expiry() {
        let f = fx();
        let a = f.agent();
        let ctx = f.ctx(ApprovalMode::Ask);
        let mut expired = f.rule(Capability::Terminal, PermissionEffect::Allow, None, PermissionScope::Global);
        expired.expires_at = Some(now() - chrono::Duration::minutes(1));
        f.engine.set_rules(vec![
            f.rule(
                Capability::Terminal,
                PermissionEffect::Allow,
                Some("cargo *"),
                PermissionScope::Project { project_id: f.project },
            ),
            f.rule(
                Capability::Write,
                PermissionEffect::Allow,
                Some("src/**"),
                PermissionScope::Project { project_id: f.project },
            ),
            expired,
        ]);
        assert!(f.engine.check(&a, Capability::Terminal, &cmd("cargo test --all"), &ctx).is_allow());
        assert!(f.engine.check(&a, Capability::Terminal, &cmd("npm test"), &ctx).is_ask());
        assert!(f.engine.check(&a, Capability::Write, &path("src/x/y.rs"), &ctx).is_allow());
        assert!(f.engine.check(&a, Capability::Write, &path("README.md"), &ctx).is_ask());
        assert!(compile_pattern(Some("[")).is_err());
    }

    #[test]
    fn path_confinement_denies_escapes_for_agents() {
        let f = fx();
        let a = f.agent();
        let ctx = f.ctx(ApprovalMode::FullAuto);
        for p in ["../outside.txt", "/etc/passwd", "C:\\Windows\\win.ini", "\\\\server\\share", "src/x.txt:ads", "NUL"]
        {
            match f.engine.check(&a, Capability::Write, &path(p), &ctx) {
                Decision::Deny(r) => assert!(r.hard, "{p}"),
                other => panic!("{p}: {other:?}"),
            }
        }
        let c = Resource::Command { command: "ls".into(), shell: ShellKind::Bash, cwd: Some("../..".into()) };
        assert!(f.engine.check(&a, Capability::Terminal, &c, &ctx).is_deny());
        // `.git` internals / secrets files ask even in FullAuto.
        assert!(f.engine.check(&a, Capability::Write, &path(".git/config"), &ctx).is_ask());
        assert!(f.engine.check(&a, Capability::Write, &path(".env"), &ctx).is_ask());
        // Reading a secrets file is treated as `Secrets`.
        match f.engine.check(&a, Capability::Read, &path(".env"), &ctx) {
            Decision::Ask(r) => assert_eq!(r.code, "secret_file"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn emergency_overrides() {
        let f = fx();
        let a = f.agent();
        let ctx = f.ctx(ApprovalMode::FullAuto);
        f.engine.set_rules(vec![
            f.rule(Capability::Network, PermissionEffect::Allow, None, PermissionScope::Global),
            f.rule(Capability::Secrets, PermissionEffect::Allow, None, PermissionScope::Global),
        ]);
        assert!(f.engine.check(&a, Capability::Network, &Resource::None, &ctx).is_allow());
        f.set_flags(|fl| fl.network_tools_disabled = true);
        assert!(f.engine.check(&a, Capability::Network, &Resource::None, &ctx).is_deny());
        assert!(f.engine.check(&a, Capability::Browser, &Resource::None, &ctx).is_deny());
        assert!(f.engine.check(&a, Capability::Secrets, &Resource::None, &ctx).is_allow());
        f.set_flags(|fl| fl.vault_locked = true);
        assert!(f.engine.check(&a, Capability::Secrets, &Resource::None, &ctx).is_deny());
        // FullAuto capped at SemiAuto: Delete asks again.
        assert!(f.engine.check(&a, Capability::Delete, &path("src/a.rs"), &ctx).is_allow());
        f.set_flags(|fl| fl.full_auto_disabled = true);
        assert!(f.engine.check(&a, Capability::Delete, &path("src/a.rs"), &ctx).is_ask());
        assert_eq!(f.engine.effective_mode(ApprovalMode::FullAuto), ApprovalMode::SemiAuto);
        f.set_flags(|fl| fl.all_stopped = true);
        assert!(f.engine.check(&a, Capability::Read, &path("src/a.rs"), &ctx).is_deny());
        assert!(f.engine.check(&Actor::Manager, Capability::Read, &path("src/a.rs"), &ctx).is_deny());
        assert!(f
            .engine
            .check(&Actor::User(orkestr_domain::UserId::new()), Capability::Read, &path("src/a.rs"), &ctx)
            .is_allow());
    }

    #[test]
    fn computer_control_needs_explicit_rule_then_asks() {
        let f = fx();
        let a = f.agent();
        let ctx = f.ctx(ApprovalMode::FullAuto);
        assert!(f.engine.check(&a, Capability::ComputerControl, &Resource::None, &ctx).is_deny());
        f.engine.set_rules(vec![f.rule(
            Capability::ComputerControl,
            PermissionEffect::Allow,
            None,
            PermissionScope::Project { project_id: f.project },
        )]);
        match f.engine.check(&a, Capability::ComputerControl, &Resource::None, &ctx) {
            Decision::Ask(r) => assert!(!r.rememberable),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn users_only_confirm_dangerous_actions() {
        let f = fx();
        let u = Actor::User(orkestr_domain::UserId::new());
        let ctx = f.ctx(ApprovalMode::Ask);
        assert!(f.engine.check(&u, Capability::Write, &path("src/a.rs"), &ctx).is_allow());
        assert!(f.engine.check(&u, Capability::Terminal, &cmd("git push --force"), &ctx).is_ask());
        assert!(f.engine.check(&u, Capability::Write, &path("../elsewhere.txt"), &ctx).is_allow());
    }
}

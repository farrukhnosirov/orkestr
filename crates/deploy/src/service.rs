//! [`DeployService`]: facade used by `core/src/rpc/deploy.rs` (and read by the
//! Bug Detective: [`DeployService::list`], [`DeployService::get`],
//! [`DeployService::logs`], [`DeployService::current`]).
//!
//! Flow of `deploy.start` / `deploy.rollback`: validate → Dangerous Action
//! Guard (protected environment, build / start commands) → either
//! `AwaitingApproval` (approval filed, `ApprovalRequested`) and a waiter that
//! runs the deployment after approval, or run now. A run resolves the
//! environment's vault secrets, streams redacted log lines (buffered, then
//! stored in the object store), publishes `DeploymentStarted`,
//! `DeploymentProgress` (ephemeral) and `DeploymentFinished`, and audits.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{
    now, Actor, ApprovalId, ApprovalStatus, AuditOutcome, Capability, Deployment, DeploymentId, DeploymentStatus,
    DeploymentTarget, Environment, EnvironmentId, EnvironmentKind, ProjectId, RiskLevel, SecretScope,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::{DeploymentFinishedPayload, EventKind};
use orkestr_protocol::methods::deploy::{
    AdapterInfo, CreateEnvironmentParams, DeployLogsResult, DeployParams, DeploymentProgressPayload, DeploymentResult,
    DeploymentStartedPayload, EnvironmentRecord, RollbackDeploymentParams, UpdateEnvironmentParams,
};
use orkestr_protocol::NewEvent;
use orkestr_security::approvals::ApprovalRequest;
use orkestr_security::audit::NewAudit;
use orkestr_security::guard::{Action, DangerClass, ShellKind};
use orkestr_security::secrets::AccessContext;
use orkestr_security::Security;
use orkestr_storage::{Db, FsObjectStore};
use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::adapter::RollbackTarget;
use crate::adapters::{builtin, Builtin};
use crate::logs::LogBook;
use crate::store::{RunKind, Store, StoredDeployment};
use crate::{AdapterAvailability, Credentials, DeployAdapter, DeployError, DeployPlan, DeployRequest, LogSink};

/// Services deployments need.
#[derive(Clone)]
pub struct DeployDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Guard, approvals, vault, audit.
    pub security: Arc<Security>,
    /// Log storage.
    pub objects: Arc<FsObjectStore>,
    /// `<data_dir>/deploy` (static sites, process state).
    pub data_dir: PathBuf,
}

/// How long a deployment waits for its approval.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(24 * 3600);
/// Grace period for an adapter to stop after cancel.
const CANCEL_GRACE: Duration = Duration::from_secs(20);
/// Min interval between `DeploymentProgress` events of one deployment.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

struct Inner {
    deps: DeployDeps,
    store: Store,
    builtin: Builtin,
    extra: RwLock<Vec<Arc<dyn DeployAdapter>>>,
    logs: LogBook,
    active: Mutex<HashMap<DeploymentId, CancellationToken>>,
    recovered: tokio::sync::OnceCell<()>,
}

/// One entry of [`DeployService::recent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentRef {
    /// Deployment.
    pub deployment_id: DeploymentId,
    /// Project.
    pub project_id: ProjectId,
    /// Environment.
    pub environment_id: EnvironmentId,
    /// `<environment> <version>`.
    pub name: String,
    /// Status.
    pub status: DeploymentStatus,
}

/// Deployment service.
#[derive(Clone)]
pub struct DeployService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DeployService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeployService")
            .field("data_dir", &self.inner.deps.data_dir)
            .field("active", &self.inner.active.lock().len())
            .finish_non_exhaustive()
    }
}

/// What a run does.
#[derive(Clone)]
struct RunSpec {
    adapter: Arc<dyn DeployAdapter>,
    env: EnvironmentRecord,
    project_root: PathBuf,
    source_dir: PathBuf,
    rollback: Option<(RollbackTarget, Option<DeploymentId>)>,
}

fn invalid(m: impl Into<String>) -> DeployError {
    DeployError::Invalid(m.into())
}

fn sec_err(e: orkestr_security::SecurityError) -> DeployError {
    match e {
        orkestr_security::SecurityError::NotFound(m) | orkestr_security::SecurityError::SecretNotFound(m) => {
            DeployError::NotFound(m)
        }
        orkestr_security::SecurityError::Forbidden(m) => DeployError::Denied(m),
        orkestr_security::SecurityError::VaultLocked => DeployError::Denied("the vault is locked".into()),
        other => DeployError::Internal(other.to_string()),
    }
}

fn shell_kind() -> ShellKind {
    if cfg!(windows) {
        ShellKind::Cmd
    } else {
        ShellKind::Sh
    }
}

fn is_final(s: DeploymentStatus) -> bool {
    matches!(
        s,
        DeploymentStatus::Succeeded
            | DeploymentStatus::Failed
            | DeploymentStatus::Cancelled
            | DeploymentStatus::RolledBack
    )
}

/// `HEAD` → short git sha of the project, else a timestamp version.
async fn resolve_version(root: &Path, requested: Option<&str>) -> String {
    let req = requested.map(str::trim).filter(|v| !v.is_empty());
    if let Some(v) = req.filter(|v| *v != "HEAD") {
        return v.chars().take(128).collect();
    }
    let root = root.to_path_buf();
    let sha = tokio::task::spawn_blocking(move || {
        let git = orkestr_platform::process::which("git")?;
        let out = orkestr_platform::process::run_with_timeout(
            &git,
            &["-C", &root.to_string_lossy(), "rev-parse", "--short=12", "HEAD"],
            Duration::from_secs(10),
        )
        .ok()?;
        let s = out.stdout.trim().to_owned();
        (out.success && !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())).then_some(s)
    })
    .await
    .ok()
    .flatten();
    sha.unwrap_or_else(|| format!("local-{}", now().format("%Y%m%d%H%M%S")))
}

impl DeployService {
    /// Creates the service with the built-in adapters.
    #[must_use]
    pub fn new(deps: DeployDeps) -> Self {
        let builtin = builtin(&deps.data_dir);
        let inner = Inner {
            store: Store::new(deps.db.clone()),
            logs: LogBook::new(Arc::clone(&deps.objects)),
            builtin,
            extra: RwLock::new(Vec::new()),
            active: Mutex::new(HashMap::new()),
            recovered: tokio::sync::OnceCell::new(),
            deps,
        };
        Self { inner: Arc::new(inner) }
    }

    /// Registers an extra adapter (plugin `DeploymentProvider`s, tests). An
    /// adapter with the same target replaces the earlier one.
    pub fn register_adapter(&self, adapter: Arc<dyn DeployAdapter>) {
        let mut extra = self.inner.extra.write();
        extra.retain(|a| a.target() != adapter.target());
        extra.push(adapter);
    }

    /// Removes an extra adapter registered with [`Self::register_adapter`]
    /// (plugin disabled, killed or uninstalled). Built-ins are never removed.
    /// Returns whether one was removed.
    pub fn unregister_adapter(&self, target: &DeploymentTarget) -> bool {
        let mut extra = self.inner.extra.write();
        let before = extra.len();
        extra.retain(|a| &a.target() != target);
        extra.len() != before
    }

    /// Registered adapters (extras override built-ins with the same target).
    #[must_use]
    pub fn adapters(&self) -> Vec<Arc<dyn DeployAdapter>> {
        let extra = self.inner.extra.read().clone();
        let mut all: Vec<Arc<dyn DeployAdapter>> = self
            .inner
            .builtin
            .all
            .iter()
            .filter(|b| !extra.iter().any(|e| e.target() == b.target()))
            .cloned()
            .collect();
        all.extend(extra);
        all
    }

    fn adapter_for(&self, target: &DeploymentTarget) -> Result<Arc<dyn DeployAdapter>, DeployError> {
        self.adapters().into_iter().find(|a| &a.target() == target).ok_or_else(|| match deferred_target(target) {
            Some(feature) => DeployError::Unimplemented(feature),
            None => DeployError::Unavailable(format!("no deployment adapter for {target:?}")),
        })
    }

    /// Startup recovery (once): interrupted runs become `Failed`, pending
    /// approvals `Cancelled`; static sites are served again.
    async fn recover(&self) {
        let inner = &self.inner;
        inner
            .recovered
            .get_or_init(|| async {
                match inner.store.unfinished().await {
                    Ok(rows) => {
                        for mut d in rows {
                            if inner.active.lock().contains_key(&d.deployment.id) {
                                continue;
                            }
                            if let Some(a) =
                                d.approval_id.filter(|_| d.deployment.status == DeploymentStatus::AwaitingApproval)
                            {
                                let _ = inner.deps.security.approvals.cancel(a, "core restarted").await;
                            }
                            d.deployment.status = if d.deployment.status == DeploymentStatus::Running {
                                DeploymentStatus::Failed
                            } else {
                                DeploymentStatus::Cancelled
                            };
                            d.error = Some("interrupted by a restart of the core".into());
                            d.deployment.finished_at = Some(now());
                            let _ = inner.store.put_dep(&d).await;
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "deployment recovery failed"),
                }
                if let Ok(envs) = inner.store.envs(None).await {
                    for e in envs.iter().filter(|e| e.target == Some(DeploymentTarget::StaticSite)) {
                        inner.builtin.static_site.restore(e.environment.id, &e.adapter_settings).await;
                    }
                }
            })
            .await;
    }

    // ---------------------------------------------------------------- environments

    async fn validate_env(&self, rec: &EnvironmentRecord) -> Result<(), DeployError> {
        let e = &rec.environment;
        let name = e.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(invalid("environment name must be 1-64 characters"));
        }
        if let EnvironmentKind::Custom(c) = &e.kind {
            if c.trim().is_empty() {
                return Err(invalid("custom environment kind needs a name"));
            }
        }
        let others = self.inner.store.envs(Some(e.project_id)).await?;
        if others.iter().any(|o| o.environment.id != e.id && o.environment.name.eq_ignore_ascii_case(name)) {
            return Err(DeployError::Conflict(format!("an environment named `{name}` already exists")));
        }
        for (k, v) in &rec.adapter_settings {
            if k.len() > 64 || v.len() > 4096 {
                return Err(invalid(format!("adapter setting `{k}` is too long")));
            }
            if orkestr_security::redact::is_sensitive_field(k) || orkestr_security::redact::redact(v) != *v {
                return Err(invalid(format!(
                    "adapter setting `{k}` looks like a credential: store it in the vault and add it as a secret reference"
                )));
            }
        }
        for (k, v) in &e.variables {
            if k.trim().is_empty() || k.len() > 128 || v.len() > 8192 {
                return Err(invalid(format!("variable `{k}` is invalid")));
            }
            if orkestr_security::redact::redact(v) != *v {
                return Err(invalid(format!(
                    "variable `{k}` looks like a credential: store it in the vault and add it as a secret reference"
                )));
            }
        }
        for id in &e.secret_refs {
            let r = self
                .inner
                .deps
                .security
                .secrets
                .get_ref(*id)
                .await
                .map_err(sec_err)?
                .ok_or_else(|| DeployError::NotFound(format!("secret {id}")))?;
            let ok = match r.scope {
                SecretScope::Global => true,
                SecretScope::Project { project_id } => project_id == e.project_id,
                SecretScope::Environment { project_id, environment_id } => {
                    project_id == e.project_id && environment_id == e.id
                }
            };
            if !ok {
                return Err(DeployError::Denied(format!("secret `{}` belongs to another project", r.name)));
            }
        }
        if let Some(t) = &rec.target {
            self.adapter_for(t)?;
        }
        Ok(())
    }

    async fn audit(
        &self,
        by: Actor,
        action: &str,
        resource: String,
        project: ProjectId,
        outcome: AuditOutcome,
        details: serde_json::Value,
    ) {
        self.inner
            .deps
            .security
            .audit
            .record(
                NewAudit::new(by, action, resource, outcome)
                    .capability(Capability::Deploy)
                    .project(Some(project))
                    .details(orkestr_security::redact::redact_json(&details)),
            )
            .await;
    }

    async fn with_last(&self, mut rec: EnvironmentRecord) -> Result<EnvironmentRecord, DeployError> {
        rec.last_deployment =
            self.inner.store.deps(None, Some(rec.environment.id), 1).await?.pop().map(|d| d.deployment);
        Ok(rec)
    }

    /// One environment (with its latest deployment).
    pub async fn environment(&self, id: EnvironmentId) -> Result<EnvironmentRecord, DeployError> {
        let rec = self.inner.store.env(id).await?.ok_or_else(|| DeployError::NotFound(format!("environment {id}")))?;
        self.with_last(rec).await
    }

    /// Environments of a project.
    pub async fn list_environments(&self, project: ProjectId) -> Result<Vec<EnvironmentRecord>, DeployError> {
        self.recover().await;
        let mut out = Vec::new();
        for r in self.inner.store.envs(Some(project)).await? {
            out.push(self.with_last(r).await?);
        }
        Ok(out)
    }

    /// Creates an environment. Production is always protected.
    pub async fn create_environment(
        &self,
        p: CreateEnvironmentParams,
        by: Actor,
    ) -> Result<EnvironmentRecord, DeployError> {
        let t = now();
        let protected = p.protected || p.kind == EnvironmentKind::Production;
        let rec = EnvironmentRecord {
            environment: Environment {
                id: EnvironmentId::new(),
                project_id: p.project_id,
                name: p.name.trim().to_owned(),
                kind: p.kind,
                variables: p.variables,
                secret_refs: p.secret_refs,
                urls: p.urls,
                protected,
                permissions: Vec::new(),
                created_at: t,
                updated_at: t,
            },
            target: p.target,
            adapter_settings: p.adapter_settings,
            last_deployment: None,
        };
        self.validate_env(&rec).await?;
        self.inner.store.put_env(&rec).await?;
        self.audit(
            by,
            "environments.create",
            rec.environment.name.clone(),
            rec.environment.project_id,
            AuditOutcome::Executed,
            serde_json::json!({ "environmentId": rec.environment.id, "target": rec.target, "protected": protected }),
        )
        .await;
        Ok(rec)
    }

    /// Updates an environment (absent fields unchanged; Production stays protected).
    pub async fn update_environment(
        &self,
        p: UpdateEnvironmentParams,
        by: Actor,
    ) -> Result<EnvironmentRecord, DeployError> {
        let mut rec = self
            .inner
            .store
            .env(p.environment_id)
            .await?
            .ok_or_else(|| DeployError::NotFound(format!("environment {}", p.environment_id)))?;
        let e = &mut rec.environment;
        if let Some(n) = p.name {
            e.name = n.trim().to_owned();
        }
        if let Some(k) = p.kind {
            e.kind = k;
        }
        if let Some(v) = p.protected {
            e.protected = v;
        }
        if e.kind == EnvironmentKind::Production {
            e.protected = true;
        }
        if let Some(v) = p.variables {
            e.variables = v;
        }
        if let Some(v) = p.secret_refs {
            e.secret_refs = v;
        }
        if let Some(v) = p.urls {
            e.urls = v;
        }
        e.updated_at = now();
        if let Some(t) = p.target {
            rec.target = Some(t);
        }
        if let Some(s) = p.adapter_settings {
            rec.adapter_settings = s;
        }
        self.validate_env(&rec).await?;
        self.inner.store.put_env(&rec).await?;
        self.audit(
            by,
            "environments.update",
            rec.environment.name.clone(),
            rec.environment.project_id,
            AuditOutcome::Executed,
            serde_json::json!({ "environmentId": rec.environment.id, "target": rec.target, "protected": rec.environment.protected }),
        )
        .await;
        self.with_last(rec).await
    }

    /// Deletes an environment (refused while a deployment of it is active).
    pub async fn delete_environment(&self, id: EnvironmentId, by: Actor) -> Result<(), DeployError> {
        let rec = self.inner.store.env(id).await?.ok_or_else(|| DeployError::NotFound(format!("environment {id}")))?;
        let busy = self.inner.store.deps(None, Some(id), 50).await?.iter().any(|d| !is_final(d.deployment.status));
        if busy {
            return Err(DeployError::Conflict(
                "a deployment of this environment is still active; cancel it first".into(),
            ));
        }
        for a in self.adapters() {
            a.release(id);
        }
        self.inner.store.delete_env(id).await?;
        self.audit(
            by,
            "environments.delete",
            rec.environment.name.clone(),
            rec.environment.project_id,
            AuditOutcome::Executed,
            serde_json::json!({ "environmentId": id }),
        )
        .await;
        Ok(())
    }

    // ---------------------------------------------------------------- adapters / plan

    /// Adapters and their availability. Cloud adapters report
    /// `NeedsCredential` until a vault secret with their token name exists.
    pub async fn adapter_infos(&self) -> Result<Vec<AdapterInfo>, DeployError> {
        let names: Vec<String> =
            self.inner.deps.security.secrets.list(None).await.map_err(sec_err)?.into_iter().map(|r| r.name).collect();
        let adapters = self.adapters();
        let avail = tokio::task::spawn_blocking({
            let adapters = adapters.clone();
            move || adapters.iter().map(|a| a.availability()).collect::<Vec<_>>()
        })
        .await
        .map_err(|e| DeployError::Internal(e.to_string()))?;
        Ok(adapters
            .iter()
            .zip(avail)
            .map(|(a, mut availability)| {
                if availability == AdapterAvailability::Ready {
                    if let Some(s) = a.required_secret() {
                        if !names.iter().any(|n| n == s) {
                            availability = AdapterAvailability::NeedsCredential { secret: s.to_owned() };
                        }
                    }
                }
                AdapterInfo {
                    id: a.id().to_owned(),
                    target: a.target(),
                    availability,
                    verification: a.verification().to_owned(),
                    settings_schema: a.settings_schema(),
                }
            })
            .collect())
    }

    async fn secret_names(
        &self,
        env: &Environment,
    ) -> Result<BTreeMap<String, orkestr_domain::SecretRefId>, DeployError> {
        let mut out = BTreeMap::new();
        for id in &env.secret_refs {
            if let Some(r) = self.inner.deps.security.secrets.get_ref(*id).await.map_err(sec_err)? {
                out.insert(r.name, *id);
            }
        }
        Ok(out)
    }

    async fn request(
        &self,
        spec: &RunSpec,
        deployment_id: DeploymentId,
        version: String,
        credentials: Credentials,
    ) -> DeployRequest {
        DeployRequest {
            deployment_id,
            project_id: spec.env.environment.project_id,
            project_root: spec.project_root.clone(),
            source_dir: spec.source_dir.clone(),
            environment: spec.env.environment.clone(),
            target: spec.adapter.target(),
            settings: spec.env.adapter_settings.clone(),
            version,
            credentials,
        }
    }

    async fn spec(
        &self,
        env: EnvironmentId,
        project_root: &Path,
        source_dir: Option<&str>,
    ) -> Result<RunSpec, DeployError> {
        self.recover().await;
        let env =
            self.inner.store.env(env).await?.ok_or_else(|| DeployError::NotFound(format!("environment {env}")))?;
        let target = env.target.clone().ok_or_else(|| invalid("the environment has no deployment target"))?;
        let adapter = self.adapter_for(&target)?;
        let source_dir = match source_dir.map(str::trim).filter(|s| !s.is_empty()) {
            Some(rel) => orkestr_security::confine(project_root, rel)
                .map_err(|e| invalid(format!("source directory `{rel}`: {e}")))?,
            None => project_root.to_path_buf(),
        };
        Ok(RunSpec { adapter, env, project_root: project_root.to_path_buf(), source_dir, rollback: None })
    }

    /// Guard verdict for a request: `Err(Denied)` when forbidden, else
    /// `Some(reason, category)` when an approval is needed.
    fn guard(
        &self,
        spec: &RunSpec,
        req: &DeployRequest,
    ) -> Result<Option<(String, Option<orkestr_domain::DangerCategory>)>, DeployError> {
        let guard = &self.inner.deps.security.guard;
        let mut verdicts = vec![guard.classify_action(&Action::Deploy {
            environment: spec.env.environment.name.clone(),
            protected: spec.env.environment.is_protected(),
        })];
        for c in spec.adapter.commands(req) {
            verdicts.push(guard.classify(&c, shell_kind()));
        }
        let mut need = None;
        for v in verdicts {
            match v {
                DangerClass::Forbidden { reason, .. } => {
                    return Err(DeployError::Denied(format!("refused by the Dangerous Action Guard: {reason}")))
                }
                DangerClass::Dangerous { category, reason } => {
                    if need.is_none() {
                        need = Some((reason, Some(category)));
                    }
                }
                DangerClass::Safe => {}
            }
        }
        Ok(need)
    }

    /// Plans a deployment (redacted previews; secrets are not read).
    pub async fn plan(&self, p: &DeployParams, project_root: &Path) -> Result<DeployPlan, DeployError> {
        let spec = self.spec(p.environment_id, project_root, p.source_dir.as_deref()).await?;
        let version = resolve_version(&spec.source_dir, p.version.as_deref()).await;
        let names = self.secret_names(&spec.env.environment).await?;
        let presence = Credentials(names.keys().map(|k| (k.clone(), String::new())).collect());
        let req = self.request(&spec, DeploymentId::new(), version, presence).await;
        let mut plan = spec.adapter.plan(&req).await?;
        match self.guard(&spec, &req) {
            Ok(Some((reason, _))) => {
                plan.requires_approval = true;
                if !spec.env.environment.is_protected() {
                    plan.warnings.push(format!("needs approval: {reason}"));
                }
            }
            Ok(None) => {}
            Err(e) => plan.warnings.push(e.to_string()),
        }
        if let AdapterAvailability::Unavailable { reason } = spec.adapter.availability() {
            plan.warnings.push(format!("adapter unavailable: {reason}"));
        }
        for r in &mut plan.requests {
            r.target = orkestr_security::redact::redact(&r.target);
            r.body = r.body.as_deref().map(orkestr_security::redact::redact);
        }
        Ok(plan)
    }

    // ---------------------------------------------------------------- start / rollback

    /// Starts a deployment. Protected environments (and dangerous build /
    /// start commands) return `AwaitingApproval` with the approval id; the
    /// deployment runs after approval.
    pub async fn start(
        &self,
        p: &DeployParams,
        project_root: &Path,
        by: Actor,
    ) -> Result<DeploymentResult, DeployError> {
        let spec = self.spec(p.environment_id, project_root, p.source_dir.as_deref()).await?;
        let version = resolve_version(&spec.source_dir, p.version.as_deref()).await;
        let mut stored = self.new_record(&spec, version, by, RunKind::Deploy);
        stored.source_dir = p.source_dir.clone();
        self.launch(spec, stored, by).await
    }

    /// Rolls an environment back to `to_version` or the previous successful version.
    pub async fn rollback(
        &self,
        p: &RollbackDeploymentParams,
        project_root: &Path,
        by: Actor,
    ) -> Result<DeploymentResult, DeployError> {
        let mut spec = self.spec(p.environment_id, project_root, None).await?;
        let history = self.inner.store.deps(None, Some(p.environment_id), 500).await?;
        let current = history.iter().find(|d| d.deployment.status == DeploymentStatus::Succeeded).cloned();
        let target = match p.to_version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => history
                .iter()
                .find(|d| {
                    d.deployment.version.as_deref() == Some(v)
                        && matches!(d.deployment.status, DeploymentStatus::Succeeded | DeploymentStatus::RolledBack)
                })
                .cloned()
                .ok_or_else(|| DeployError::NotFound(format!("no successful deployment of version {v}")))?,
            None => {
                let cur = current.as_ref().ok_or_else(|| DeployError::Conflict("nothing is deployed yet".into()))?;
                history
                    .iter()
                    .filter(|d| {
                        d.deployment.status == DeploymentStatus::Succeeded && d.deployment.id != cur.deployment.id
                    })
                    .find(|d| d.deployment.version != cur.deployment.version)
                    .cloned()
                    .ok_or_else(|| {
                        DeployError::Conflict("there is no earlier successful version to roll back to".into())
                    })?
            }
        };
        let version = target.deployment.version.clone().unwrap_or_default();
        spec.rollback = Some((
            RollbackTarget { deployment: target.deployment.clone(), provider_ref: target.provider_ref.clone() },
            current.map(|c| c.deployment.id),
        ));
        if let Some(rel) = &target.source_dir {
            if let Ok(dir) = orkestr_security::confine(project_root, rel) {
                spec.source_dir = dir;
            }
        }
        let mut stored = self.new_record(&spec, version, by, RunKind::Rollback);
        stored.rollback_of = Some(target.deployment.id);
        stored.provider_ref = target.provider_ref.clone();
        self.launch(spec, stored, by).await
    }

    fn new_record(&self, spec: &RunSpec, version: String, by: Actor, kind: RunKind) -> StoredDeployment {
        StoredDeployment {
            deployment: Deployment {
                id: DeploymentId::new(),
                project_id: spec.env.environment.project_id,
                environment_id: spec.env.environment.id,
                target: spec.adapter.target(),
                status: DeploymentStatus::Pending,
                version: Some(version),
                task_id: None,
                triggered_by: by,
                url: None,
                log_object_id: None,
                started_at: None,
                finished_at: None,
                created_at: now(),
            },
            kind,
            approval_id: None,
            provider_ref: None,
            error: None,
            source_dir: None,
            rollback_of: None,
        }
    }

    async fn launch(
        &self,
        spec: RunSpec,
        mut stored: StoredDeployment,
        by: Actor,
    ) -> Result<DeploymentResult, DeployError> {
        let env = &spec.env.environment;
        let action = if stored.kind == RunKind::Rollback { "deploy.rollback" } else { "deploy.start" };
        if let AdapterAvailability::Unavailable { reason } = spec.adapter.availability() {
            return Err(DeployError::Unavailable(reason));
        }
        let names = self.secret_names(env).await?;
        if let Some(s) = spec.adapter.required_secret() {
            if !names.contains_key(s) {
                return Err(DeployError::Unavailable(format!(
                    "needs credential {s}: add a vault secret named {s} to this environment"
                )));
            }
        }
        if self.inner.store.deps(None, Some(env.id), 20).await?.iter().any(|d| !is_final(d.deployment.status)) {
            return Err(DeployError::Conflict(format!("a deployment of `{}` is already in progress", env.name)));
        }
        let version = stored.deployment.version.clone().unwrap_or_default();
        let presence = Credentials(names.keys().map(|k| (k.clone(), String::new())).collect());
        let preview_req = self.request(&spec, stored.deployment.id, version.clone(), presence).await;
        let verdict = match self.guard(&spec, &preview_req) {
            Ok(v) => v,
            Err(e) => {
                self.audit(
                    by,
                    action,
                    env.name.clone(),
                    env.project_id,
                    AuditOutcome::Denied,
                    serde_json::json!({ "error": e.to_string() }),
                )
                .await;
                return Err(e);
            }
        };
        let token = CancellationToken::new();
        self.inner.active.lock().insert(stored.deployment.id, token.clone());
        let mut approval = None;
        if let Some((reason, danger)) = verdict {
            let plan = spec.adapter.plan(&preview_req).await.ok();
            let preview = plan.map(|p| {
                let mut lines = p.steps.clone();
                lines.extend(p.requests.iter().map(|r| format!("{} {}", r.method, r.target)));
                orkestr_security::redact::redact(&lines.join("\n"))
            });
            let verb = if stored.kind == RunKind::Rollback { "Roll back" } else { "Deploy" };
            let mut ar = ApprovalRequest::new(
                Capability::Deploy,
                format!("{verb} `{}` to {} ({})", version, env.name, spec.adapter.id()),
                reason,
                RiskLevel::High,
            );
            ar.project_id = Some(env.project_id);
            ar.command_preview = preview;
            ar.danger = danger;
            ar.ttl = Some(APPROVAL_TIMEOUT);
            let a = match self.inner.deps.security.approvals.request(ar).await {
                Ok(a) => a,
                Err(e) => {
                    self.inner.active.lock().remove(&stored.deployment.id);
                    return Err(sec_err(e));
                }
            };
            stored.deployment.status = DeploymentStatus::AwaitingApproval;
            stored.approval_id = Some(a.id);
            approval = Some(a.id);
        }
        if let Err(e) = self.inner.store.put_dep(&stored).await {
            self.inner.active.lock().remove(&stored.deployment.id);
            return Err(e);
        }
        self.audit(
            by,
            action,
            env.name.clone(),
            env.project_id,
            AuditOutcome::Allowed,
            serde_json::json!({
                "deploymentId": stored.deployment.id, "version": version, "adapter": spec.adapter.id(),
                "approvalId": approval,
            }),
        )
        .await;
        let result = DeploymentResult { deployment: stored.deployment.clone(), approval_id: approval };
        let this = self.clone();
        tokio::spawn(async move { this.run(spec, stored, approval, token).await });
        Ok(result)
    }

    async fn publish(&self, project: ProjectId, kind: EventKind) {
        if let Err(e) = self.inner.deps.events.publish(NewEvent::new(kind).project(project)).await {
            tracing::warn!(error = %e, "failed to publish deployment event");
        }
    }

    async fn progress(&self, d: &Deployment, step: &str, message: Option<String>) {
        self.publish(
            d.project_id,
            EventKind::DeploymentProgress(DeploymentProgressPayload {
                deployment_id: d.id,
                project_id: d.project_id,
                status: d.status,
                step: step.to_owned(),
                message,
            }),
        )
        .await;
    }

    async fn finish(
        &self,
        mut stored: StoredDeployment,
        status: DeploymentStatus,
        error: Option<String>,
    ) -> StoredDeployment {
        let id = stored.deployment.id;
        if let Some(e) = error {
            stored.error =
                Some(self.inner.logs.push(id, &format!("[orkestr] {e}")).trim_start_matches("[orkestr] ").to_owned());
        }
        self.inner.logs.push(id, &format!("[orkestr] deployment {status:?}"));
        stored.deployment.status = status;
        stored.deployment.finished_at = Some(now());
        match self.inner.logs.finish(id).await {
            Ok(Some(oid)) => stored.deployment.log_object_id = Some(oid),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "failed to store deployment log"),
        }
        if let Err(e) = self.inner.store.put_dep(&stored).await {
            tracing::error!(error = %e, "failed to persist a finished deployment");
        }
        self.inner.active.lock().remove(&id);
        let d = stored.deployment.clone();
        self.progress(&d, "finished", stored.error.clone()).await;
        self.publish(d.project_id, EventKind::DeploymentFinished(DeploymentFinishedPayload { deployment: d.clone() }))
            .await;
        let outcome = match status {
            DeploymentStatus::Succeeded => AuditOutcome::Executed,
            DeploymentStatus::Cancelled => AuditOutcome::Rejected,
            _ => AuditOutcome::Failed,
        };
        let action = if stored.kind == RunKind::Rollback { "deploy.rollback.finished" } else { "deploy.finished" };
        self.audit(
            Actor::System,
            action,
            d.id.to_string(),
            d.project_id,
            outcome,
            serde_json::json!({ "environmentId": d.environment_id, "version": d.version, "status": d.status, "url": d.url, "error": stored.error }),
        )
        .await;
        stored
    }

    async fn run(
        &self,
        spec: RunSpec,
        mut stored: StoredDeployment,
        approval: Option<ApprovalId>,
        token: CancellationToken,
    ) {
        let id = stored.deployment.id;
        self.inner.logs.open(id, Vec::new());
        if let Some(a) = approval {
            self.inner.logs.push(id, "[orkestr] waiting for approval");
            let status = self.inner.deps.security.approvals.wait(a, APPROVAL_TIMEOUT, &token).await;
            match status {
                Ok(ApprovalStatus::Approved) => {
                    self.inner.logs.push(id, "[orkestr] approved");
                }
                Ok(other) => {
                    let msg = format!("approval {other:?}");
                    self.finish(stored, DeploymentStatus::Cancelled, Some(msg)).await;
                    return;
                }
                Err(e) => {
                    self.finish(stored, DeploymentStatus::Failed, Some(format!("approval failed: {e}"))).await;
                    return;
                }
            }
        }
        if token.is_cancelled() {
            self.finish(stored, DeploymentStatus::Cancelled, Some("cancelled".into())).await;
            return;
        }
        // Resolve credentials (audited reads; values registered with the redactor).
        let env = &spec.env.environment;
        let mut creds = BTreeMap::new();
        let names = match self.secret_names(env).await {
            Ok(n) => n,
            Err(e) => {
                self.finish(stored, DeploymentStatus::Failed, Some(e.to_string())).await;
                return;
            }
        };
        for (name, sid) in names {
            match self
                .inner
                .deps
                .security
                .secrets
                .resolve(sid, Actor::System, AccessContext { project_id: Some(env.project_id) })
                .await
            {
                Ok(v) => {
                    creds.insert(name, v.to_string());
                }
                Err(e) => {
                    self.finish(stored, DeploymentStatus::Failed, Some(format!("cannot read secret {name}: {e}")))
                        .await;
                    return;
                }
            }
        }
        self.inner.logs.open(id, creds.values().cloned().collect());
        let version = stored.deployment.version.clone().unwrap_or_default();
        let req = self.request(&spec, id, version.clone(), Credentials(creds)).await;
        stored.deployment.status = DeploymentStatus::Running;
        stored.deployment.started_at = Some(now());
        if let Err(e) = self.inner.store.put_dep(&stored).await {
            self.finish(stored, DeploymentStatus::Failed, Some(e.to_string())).await;
            return;
        }
        self.publish(
            env.project_id,
            EventKind::DeploymentStarted(DeploymentStartedPayload { deployment: stored.deployment.clone() }),
        )
        .await;
        let verb = if stored.kind == RunKind::Rollback { "rollback to" } else { "deploy" };
        self.inner.logs.push(id, &format!("[orkestr] {verb} {version} → {} via {}", env.name, spec.adapter.id()));
        // Log sink: redacted buffer + throttled progress events.
        let sink = {
            let this = self.clone();
            let dep = stored.deployment.clone();
            let last = Arc::new(Mutex::new(Instant::now().checked_sub(PROGRESS_EVERY).unwrap_or_else(Instant::now)));
            LogSink::new(move |line| {
                let red = this.inner.logs.push(dep.id, line);
                let due = {
                    let mut l = last.lock();
                    if l.elapsed() >= PROGRESS_EVERY {
                        *l = Instant::now();
                        true
                    } else {
                        false
                    }
                };
                if due {
                    let this = this.clone();
                    let dep = dep.clone();
                    tokio::spawn(async move { this.progress(&dep, "running", Some(red)).await });
                }
            })
        };
        let fut = async {
            match &spec.rollback {
                Some((target, _)) => spec.adapter.rollback_to(&req, target, sink.clone(), token.clone()).await,
                None => spec.adapter.deploy(&req, sink.clone(), token.clone()).await,
            }
        };
        let result = tokio::select! {
            r = fut => r,
            () = async { token.cancelled().await; tokio::time::sleep(CANCEL_GRACE).await } => Err(DeployError::Cancelled),
        };
        match result {
            Ok(out) => {
                stored.deployment.url = out.url.clone();
                stored.deployment.version = Some(out.version.clone());
                if out.provider_ref.is_some() {
                    stored.provider_ref = out.provider_ref.clone();
                }
                if let Some(u) = &out.url {
                    self.inner.logs.push(id, &format!("[orkestr] live at {u}"));
                }
                if let Some((_, Some(replaced))) = &spec.rollback {
                    if let Ok(Some(mut old)) = self.inner.store.dep(*replaced).await {
                        old.deployment.status = DeploymentStatus::RolledBack;
                        let _ = self.inner.store.put_dep(&old).await;
                    }
                }
                self.finish(stored, DeploymentStatus::Succeeded, None).await;
            }
            Err(DeployError::Cancelled) => {
                self.finish(stored, DeploymentStatus::Cancelled, Some("cancelled".into())).await;
            }
            Err(e) => {
                self.finish(stored, DeploymentStatus::Failed, Some(e.to_string())).await;
            }
        }
    }

    // ---------------------------------------------------------------- control / reads

    /// Cancels a deployment (pending approval or running). Waits briefly for
    /// the run to settle and returns the record.
    pub async fn cancel(&self, id: DeploymentId, by: Actor) -> Result<DeploymentResult, DeployError> {
        let stored =
            self.inner.store.dep(id).await?.ok_or_else(|| DeployError::NotFound(format!("deployment {id}")))?;
        let token = self.inner.active.lock().get(&id).cloned();
        match token {
            Some(t) => {
                t.cancel();
                self.audit(
                    by,
                    "deploy.cancel",
                    id.to_string(),
                    stored.deployment.project_id,
                    AuditOutcome::Executed,
                    serde_json::json!({ "status": stored.deployment.status }),
                )
                .await;
                let deadline = Instant::now() + Duration::from_secs(25);
                while Instant::now() < deadline && self.inner.active.lock().contains_key(&id) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            None if !is_final(stored.deployment.status) => {
                // Orphaned (e.g. after a crash before recovery ran).
                self.finish(stored, DeploymentStatus::Cancelled, Some("cancelled".into())).await;
            }
            None => {}
        }
        self.get(id).await
    }

    /// Cancels every active deployment (emergency stop). Returns how many.
    pub async fn cancel_all(&self) -> usize {
        let tokens: Vec<CancellationToken> = self.inner.active.lock().values().cloned().collect();
        for t in &tokens {
            t.cancel();
        }
        tokens.len()
    }

    /// Ids of active (awaiting approval or running) deployments.
    #[must_use]
    pub fn active(&self) -> Vec<DeploymentId> {
        self.inner.active.lock().keys().copied().collect()
    }

    /// One deployment (with its approval id).
    pub async fn get(&self, id: DeploymentId) -> Result<DeploymentResult, DeployError> {
        let s = self.inner.store.dep(id).await?.ok_or_else(|| DeployError::NotFound(format!("deployment {id}")))?;
        Ok(DeploymentResult { deployment: s.deployment, approval_id: s.approval_id })
    }

    /// Full stored record (error, provider ref, kind).
    pub async fn record(&self, id: DeploymentId) -> Result<StoredDeployment, DeployError> {
        self.inner.store.dep(id).await?.ok_or_else(|| DeployError::NotFound(format!("deployment {id}")))
    }

    /// Deployments newest first (default limit 50, max 500).
    pub async fn list(
        &self,
        project: ProjectId,
        env: Option<EnvironmentId>,
        limit: Option<u32>,
    ) -> Result<Vec<Deployment>, DeployError> {
        self.recover().await;
        let limit = limit.unwrap_or(50).clamp(1, 500);
        Ok(self.inner.store.deps(Some(project), env, limit).await?.into_iter().map(|d| d.deployment).collect())
    }

    /// The deployment currently live in `env` (latest `Succeeded`).
    pub async fn current(&self, env: EnvironmentId) -> Result<Option<Deployment>, DeployError> {
        Ok(self
            .inner
            .store
            .deps(None, Some(env), 500)
            .await?
            .into_iter()
            .find(|d| d.deployment.status == DeploymentStatus::Succeeded)
            .map(|d| d.deployment))
    }

    /// Version currently live in `env`.
    pub async fn current_version(&self, env: EnvironmentId) -> Result<Option<String>, DeployError> {
        Ok(self.current(env).await?.and_then(|d| d.version))
    }

    /// The successful deployment live before the current one in `env`
    /// (a different version; what a rollback without `toVersion` restores).
    pub async fn previous(&self, env: EnvironmentId) -> Result<Option<Deployment>, DeployError> {
        let ok: Vec<Deployment> = self
            .inner
            .store
            .deps(None, Some(env), 500)
            .await?
            .into_iter()
            .filter(|d| d.deployment.status == DeploymentStatus::Succeeded)
            .map(|d| d.deployment)
            .collect();
        let Some(cur) = ok.first() else { return Ok(None) };
        Ok(ok.iter().skip(1).find(|d| d.version != cur.version).cloned())
    }

    /// Version live before the current one in `env`.
    pub async fn previous_version(&self, env: EnvironmentId) -> Result<Option<String>, DeployError> {
        Ok(self.previous(env).await?.and_then(|d| d.version))
    }

    /// Most recent deployments of every project (newest first) with a display
    /// name `<environment> <version>` — the deployment-log feed of the monitor.
    pub async fn recent(&self, limit: u32) -> Result<Vec<DeploymentRef>, DeployError> {
        self.recover().await;
        let rows = self.inner.store.deps(None, None, limit.clamp(1, 500)).await?;
        let mut names: HashMap<EnvironmentId, String> = HashMap::new();
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let d = r.deployment;
            if let std::collections::hash_map::Entry::Vacant(slot) = names.entry(d.environment_id) {
                let n = self
                    .inner
                    .store
                    .env(d.environment_id)
                    .await?
                    .map_or_else(|| d.environment_id.to_string(), |e| e.environment.name);
                slot.insert(n);
            }
            let env_name = names.get(&d.environment_id).cloned().unwrap_or_default();
            out.push(DeploymentRef {
                deployment_id: d.id,
                project_id: d.project_id,
                environment_id: d.environment_id,
                name: format!("{env_name} {}", d.version.as_deref().unwrap_or("")).trim().to_owned(),
                status: d.status,
            });
        }
        Ok(out)
    }

    /// Redacted log lines from `offset`.
    pub async fn logs(&self, id: DeploymentId, offset: u64) -> Result<DeployLogsResult, DeployError> {
        let s = self.inner.store.dep(id).await?.ok_or_else(|| DeployError::NotFound(format!("deployment {id}")))?;
        let (lines, next_offset, complete) =
            self.inner.logs.read(id, s.deployment.log_object_id.as_deref(), offset).await?;
        let complete = complete && is_final(s.deployment.status);
        Ok(DeployLogsResult { lines, next_offset, complete })
    }

    /// The static-site adapter (local serving state).
    #[must_use]
    pub fn static_site(&self) -> &crate::adapters::StaticSiteAdapter {
        &self.inner.builtin.static_site
    }

    /// The local-process adapter (process output tail).
    #[must_use]
    pub fn local_process(&self) -> &crate::adapters::LocalProcessAdapter {
        &self.inner.builtin.local_process
    }
}

/// Cloud targets deferred to a later phase (RR-G14): `unimplementedFeature`
/// instead of a generic "no adapter" error.
fn deferred_target(target: &DeploymentTarget) -> Option<&'static str> {
    match target {
        DeploymentTarget::Aws => Some("deploy adapter Aws (deferred, RR-G14)"),
        DeploymentTarget::Gcp => Some("deploy adapter Gcp (deferred, RR-G14)"),
        DeploymentTarget::Azure => Some("deploy adapter Azure (deferred, RR-G14)"),
        DeploymentTarget::Cloudflare => Some("deploy adapter Cloudflare (deferred, RR-G14)"),
        _ => None,
    }
}

#[cfg(test)]
mod deferred_tests {
    use super::*;

    #[test]
    fn deferred_cloud_targets_are_unimplemented_features() {
        for t in [DeploymentTarget::Aws, DeploymentTarget::Gcp, DeploymentTarget::Azure, DeploymentTarget::Cloudflare] {
            assert!(deferred_target(&t).is_some(), "{t:?}");
        }
        assert!(deferred_target(&DeploymentTarget::Ssh).is_none());
        let e: orkestr_protocol::RpcError =
            DeployError::Unimplemented(deferred_target(&DeploymentTarget::Aws).unwrap()).into();
        assert!(e.data.as_ref().and_then(|d| d.get("unimplementedFeature")).is_some(), "{e:?}");
    }
}

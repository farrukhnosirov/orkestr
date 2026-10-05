//! The provider registry (spec §9): persisted configs, hot-swappable
//! adapters, runtime status (health, auth, limits, circuit breaker), model
//! catalog cache, background health polling, routing and fallback execution.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use orkestr_domain::{
    AuthState, ManagerConfig, Model, ModelRef, Provider, ProviderHealth, ProviderId, ProviderKind, ProviderStatus,
    RoutingConfig, Timestamp,
};
use orkestr_storage::{Db, Migration, StorageError};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::adapters::cli::resolve_binary;
use crate::catalog::{default_models, kind_info, OLLAMA_DEFAULT_URL, PHASE1_KINDS};
use crate::credentials::{CredentialError, CredentialResolver};
use crate::factory::{
    build_adapter, effective_base_url, headers_from_settings, is_local, validate_base_url, AdapterDeps,
};
use crate::fallback::{
    chat_with_fallback, AdapterSource, AttemptOutcome, FallbackOptions, HandoffHook, ProviderEvent, ProviderEventSink,
};
use crate::limits::{CircuitBreaker, LatencyStats, LimitWindows};
use crate::pricing::estimate_cost;
use crate::router::{self, Candidate, RouteRequest};
use crate::store::{ProviderStore, KEY_BOOTSTRAPPED, KEY_MANAGER, KEY_ROUTING, MIGRATIONS};
use crate::types::{
    AuthStatus, ChatEvent, ChatMessage, ChatRequest, ChatStream, HealthReport, ProviderDescriptor, ProviderError, Usage,
};

/// Migrations of this crate (composed by the core).
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}

/// Registry errors.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Unknown provider.
    #[error("not found: {0}")]
    NotFound(String),
    /// Invalid input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Conflict (duplicate name).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Provider failure.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Storage failure.
    #[error(transparent)]
    Storage(StorageError),
    /// Secret vault failure.
    #[error("credential: {0}")]
    Credential(#[from] CredentialError),
}

impl From<StorageError> for RegistryError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::Conflict(m) => RegistryError::Conflict(m),
            StorageError::NotFound(m) => RegistryError::NotFound(m),
            StorageError::Invalid(m) => RegistryError::Invalid(m),
            other => RegistryError::Storage(other),
        }
    }
}

/// Result alias.
pub type RegistryResult<T> = Result<T, RegistryError>;

/// Registry tuning.
#[derive(Debug, Clone)]
pub struct RegistryOptions {
    /// Background health poll interval (HTTP providers; CLIs are probed on demand).
    pub health_interval: Duration,
    /// Model catalog TTL.
    pub model_ttl: Duration,
    /// Timeout for health / auth / model probes.
    pub probe_timeout: Duration,
    /// Auto-detect Ollama and CLIs on first start.
    pub detect_on_first_start: bool,
    /// Ollama URL probed on first start.
    pub ollama_detect_url: String,
    /// Fallback executor options.
    pub fallback: FallbackOptions,
}

impl Default for RegistryOptions {
    fn default() -> Self {
        Self {
            health_interval: Duration::from_secs(60),
            model_ttl: Duration::from_secs(600),
            probe_timeout: Duration::from_secs(15),
            detect_on_first_start: true,
            ollama_detect_url: OLLAMA_DEFAULT_URL.to_owned(),
            fallback: FallbackOptions::default(),
        }
    }
}

/// Input of [`ProviderRegistry::create`].
#[derive(Debug, Clone, Default)]
pub struct NewProvider {
    /// Kind.
    pub kind: Option<ProviderKind>,
    /// Display name (default: kind name, made unique).
    pub name: Option<String>,
    /// Base URL.
    pub base_url: Option<String>,
    /// Non-secret custom headers.
    pub headers: Option<BTreeMap<String, String>>,
    /// Default model.
    pub default_model: Option<String>,
    /// Enabled (default true).
    pub enabled: Option<bool>,
    /// API key (stored in the vault; never persisted here).
    pub api_key: Option<String>,
    /// Pinned CLI binary path.
    pub binary_path: Option<String>,
}

/// Partial update. Empty strings clear `base_url`, `default_model`, `binary_path`.
#[derive(Debug, Clone, Default)]
pub struct ProviderPatch {
    /// Name.
    pub name: Option<String>,
    /// Base URL.
    pub base_url: Option<String>,
    /// Headers (replace).
    pub headers: Option<BTreeMap<String, String>>,
    /// Default model.
    pub default_model: Option<String>,
    /// Enabled.
    pub enabled: Option<bool>,
    /// CLI binary path.
    pub binary_path: Option<String>,
}

/// A provider with its live state.
#[derive(Debug, Clone)]
pub struct ProviderView {
    /// Config.
    pub provider: Provider,
    /// Live status.
    pub status: ProviderStatus,
    /// An API key is stored.
    pub has_credential: bool,
    /// Adapter descriptor.
    pub descriptor: ProviderDescriptor,
}

/// Model list result.
#[derive(Debug, Clone)]
pub struct ModelsOutcome {
    /// Models.
    pub models: Vec<Model>,
    /// Served from cache.
    pub cached: bool,
    /// When fetched.
    pub fetched_at: Timestamp,
}

/// Connection test result.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    /// Success.
    pub ok: bool,
    /// Model used.
    pub model: String,
    /// Time to first event.
    pub latency_ms: u64,
    /// Start of the reply.
    pub sample: Option<String>,
    /// Failure.
    pub error: Option<ProviderError>,
    /// Usage.
    pub usage: Option<Usage>,
    /// Estimated cost (USD).
    pub cost_usd: Option<f64>,
}

#[derive(Clone)]
struct Entry {
    config: Provider,
    adapter: Arc<dyn ProviderAdapter>,
}

#[derive(Default)]
struct Runtime {
    status: ProviderStatus,
    circuit: CircuitBreaker,
    windows: LimitWindows,
    latency: LatencyStats,
}

struct CachedModels {
    models: Vec<Model>,
    fetched_at: Timestamp,
    at: Instant,
}

fn to_ts(d: Duration) -> Timestamp {
    chrono::Utc::now() + chrono::Duration::from_std(d).unwrap_or_else(|_| chrono::Duration::seconds(0))
}

/// The registry.
pub struct ProviderRegistry {
    store: ProviderStore,
    deps: AdapterDeps,
    sink: Arc<dyn ProviderEventSink>,
    options: RegistryOptions,
    entries: RwLock<HashMap<ProviderId, Entry>>,
    runtime: Mutex<HashMap<ProviderId, Runtime>>,
    models: Mutex<HashMap<ProviderId, CachedModels>>,
    writes: tokio::sync::Mutex<()>,
    /// Historical benchmark statistics for auto-routing (wired by the core).
    stats: RwLock<Option<Arc<dyn crate::history::ModelStatsSource>>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry").field("providers", &self.entries.read().len()).finish_non_exhaustive()
    }
}

/// Config validation failures are client errors, not provider failures.
fn invalid(e: ProviderError) -> RegistryError {
    match e {
        ProviderError::BadRequest(m) => RegistryError::Invalid(m),
        other => RegistryError::Provider(other),
    }
}

fn validate_name(name: &str) -> RegistryResult<String> {
    let t = name.trim();
    if t.is_empty() || t.chars().count() > 80 {
        return Err(RegistryError::Invalid("provider name must be 1..=80 characters".into()));
    }
    Ok(t.to_owned())
}

fn validate_api_key(key: &str) -> RegistryResult<String> {
    let k = key.trim();
    if k.is_empty() || k.len() > 4096 || k.contains(char::is_whitespace) || k.chars().any(char::is_control) {
        return Err(RegistryError::Invalid("API key must be a single non-empty token".into()));
    }
    Ok(k.to_owned())
}

fn validate_model_id(m: &str) -> RegistryResult<String> {
    let t = m.trim();
    if t.is_empty() || t.len() > 200 || t.chars().any(char::is_control) {
        return Err(RegistryError::Invalid("model id must be 1..=200 printable characters".into()));
    }
    Ok(t.to_owned())
}

fn validate_binary_path(p: &str) -> RegistryResult<String> {
    let path = std::path::Path::new(p.trim());
    if !path.is_absolute() || !path.is_file() {
        return Err(RegistryError::Invalid("CLI path must be an absolute path to an existing file".into()));
    }
    Ok(path.to_string_lossy().into_owned())
}

impl ProviderRegistry {
    /// Creates a registry over a migrated database.
    #[must_use]
    pub fn new(db: Db, credentials: Arc<dyn CredentialResolver>, sink: Arc<dyn ProviderEventSink>) -> Self {
        Self::with_options(db, AdapterDeps::new(credentials), sink, RegistryOptions::default())
    }

    /// Full constructor.
    #[must_use]
    pub fn with_options(db: Db, deps: AdapterDeps, sink: Arc<dyn ProviderEventSink>, options: RegistryOptions) -> Self {
        Self {
            store: ProviderStore::new(db),
            deps,
            sink,
            options,
            entries: RwLock::new(HashMap::new()),
            runtime: Mutex::new(HashMap::new()),
            models: Mutex::new(HashMap::new()),
            writes: tokio::sync::Mutex::new(()),
            stats: RwLock::new(None),
        }
    }

    /// Loads persisted providers and performs first-start registration
    /// (Mock, plus detected CLIs and a reachable local Ollama).
    pub async fn init(&self) -> RegistryResult<()> {
        for p in self.store.list().await? {
            match build_adapter(&p, &self.deps) {
                Ok(adapter) => {
                    self.entries.write().insert(p.id, Entry { config: p, adapter });
                }
                Err(e) => tracing::warn!(provider = %p.id, error = %e, "skipping provider with invalid config"),
            }
        }
        for p in self.store_entries() {
            if let Some((models, fetched_at)) = self.store.cached_models(p.id).await? {
                self.models.lock().insert(
                    p.id,
                    CachedModels {
                        models,
                        fetched_at,
                        at: Instant::now().checked_sub(self.options.model_ttl).unwrap_or_else(Instant::now),
                    },
                );
            }
        }
        if self.store.setting::<bool>(KEY_BOOTSTRAPPED).await?.unwrap_or(false) {
            return Ok(());
        }
        self.first_start().await?;
        self.store.set_setting(KEY_BOOTSTRAPPED, &true).await?;
        Ok(())
    }

    fn store_entries(&self) -> Vec<Provider> {
        self.entries.read().values().map(|e| e.config.clone()).collect()
    }

    async fn first_start(&self) -> RegistryResult<()> {
        let kinds: Vec<ProviderKind> = self.store_entries().iter().map(|p| p.kind).collect();
        if !kinds.contains(&ProviderKind::Mock) {
            self.create(NewProvider { kind: Some(ProviderKind::Mock), ..NewProvider::default() }).await?;
        }
        if !self.options.detect_on_first_start {
            return Ok(());
        }
        for (kind, bin) in [(ProviderKind::ClaudeCode, "claude"), (ProviderKind::Codex, "codex")] {
            if !kinds.contains(&kind) && resolve_binary(bin, None).is_some() {
                tracing::info!(binary = bin, "detected official CLI; registering provider");
                self.create(NewProvider { kind: Some(kind), ..NewProvider::default() }).await?;
            }
        }
        if !kinds.contains(&ProviderKind::Ollama) {
            let url = format!("{}/api/version", self.options.ollama_detect_url.trim_end_matches('/'));
            let reachable = self
                .deps
                .client
                .get(url)
                .timeout(Duration::from_millis(1500))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if reachable {
                tracing::info!("detected local Ollama; registering provider");
                let base = (self.options.ollama_detect_url != OLLAMA_DEFAULT_URL)
                    .then(|| self.options.ollama_detect_url.clone());
                self.create(NewProvider { kind: Some(ProviderKind::Ollama), base_url: base, ..NewProvider::default() })
                    .await?;
            }
        }
        Ok(())
    }

    /// Starts background health polling (immediate pass, then every interval).
    pub fn spawn_health_loop(self: &Arc<Self>, shutdown: CancellationToken) -> tokio::task::JoinHandle<()> {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let ids: Vec<(ProviderId, ProviderKind, bool)> =
                    me.entries.read().values().map(|e| (e.config.id, e.config.kind, e.config.enabled)).collect();
                for (id, kind, enabled) in ids {
                    // CLIs: version probe only on first pass / on demand (spec §9).
                    let cli = matches!(kind, ProviderKind::ClaudeCode | ProviderKind::Codex);
                    if enabled && (first || !cli) {
                        tokio::select! {
                            _ = me.health_check(id) => {}
                            () = shutdown.cancelled() => return,
                        }
                    }
                }
                first = false;
                tokio::select! {
                    () = tokio::time::sleep(me.options.health_interval) => {}
                    () = shutdown.cancelled() => return,
                }
            }
        })
    }

    fn entry(&self, id: ProviderId) -> RegistryResult<Entry> {
        self.entries.read().get(&id).cloned().ok_or_else(|| RegistryError::NotFound(format!("provider {id} not found")))
    }

    /// The adapter of a provider (enabled or not).
    #[must_use]
    pub fn adapter(&self, id: ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.entries.read().get(&id).map(|e| Arc::clone(&e.adapter))
    }

    fn status(&self, id: ProviderId) -> ProviderStatus {
        let now = Instant::now();
        let mut rt = self.runtime.lock();
        let r = rt.entry(id).or_default();
        let mut s = r.status.clone();
        s.circuit_open = r.circuit.is_open(now);
        if r.windows.rate_limited_until.is_none_or_past(now) {
            s.rate_limited_until = None;
        }
        if !r.windows.quota_exhausted(now) {
            s.quota_exhausted_until = None;
        }
        s.avg_latency_ms = r.latency.average_ms().map(|v| v.round() as u64);
        s
    }

    fn view(&self, e: &Entry) -> ProviderView {
        ProviderView {
            provider: e.config.clone(),
            status: self.status(e.config.id),
            has_credential: e.config.secret_ref_id.is_some(),
            descriptor: e.adapter.descriptor(),
        }
    }

    /// All providers with status, in creation order.
    #[must_use]
    pub fn list(&self) -> Vec<ProviderView> {
        let mut entries: Vec<Entry> = self.entries.read().values().cloned().collect();
        entries.sort_by(|a, b| a.config.created_at.cmp(&b.config.created_at).then(a.config.id.cmp(&b.config.id)));
        entries.iter().map(|e| self.view(e)).collect()
    }

    /// One provider.
    pub fn get(&self, id: ProviderId) -> RegistryResult<ProviderView> {
        Ok(self.view(&self.entry(id)?))
    }

    /// Updates the status and emits `StatusChanged` when health/auth changed
    /// (or `force`).
    fn update_status(&self, id: ProviderId, force: bool, f: impl FnOnce(&mut ProviderStatus)) {
        let event = {
            let mut rt = self.runtime.lock();
            let s = &mut rt.entry(id).or_default().status;
            let before = (s.health, s.auth_state);
            f(s);
            (force || before != (s.health, s.auth_state)).then(|| ProviderEvent::StatusChanged {
                provider_id: id,
                health: s.health,
                auth_state: s.auth_state,
                detail: s.health_detail.clone().or_else(|| s.auth_detail.clone()),
            })
        };
        if let Some(ev) = event {
            self.sink.emit(ev);
        }
    }

    fn apply_auth(s: &mut ProviderStatus, auth: &AuthStatus) {
        s.auth_state = auth.wire();
        s.auth_detail = Some(auth.detail());
    }

    async fn insert_entry(&self, p: Provider) -> RegistryResult<ProviderView> {
        let adapter = build_adapter(&p, &self.deps).map_err(invalid)?;
        let auth = tokio::time::timeout(Duration::from_secs(3), adapter.auth_state()).await.ok();
        let entry = Entry { config: p.clone(), adapter };
        self.entries.write().insert(p.id, entry.clone());
        self.update_status(p.id, true, |s| {
            if let Some(a) = &auth {
                Self::apply_auth(s, a);
            }
        });
        Ok(self.view(&entry))
    }

    fn unique_name(&self, base: &str) -> String {
        let taken: Vec<String> = self.entries.read().values().map(|e| e.config.name.to_lowercase()).collect();
        if !taken.contains(&base.to_lowercase()) {
            return base.to_owned();
        }
        (2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| !taken.contains(&n.to_lowercase()))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Creates a provider. The API key (if any) goes to the secret vault.
    pub async fn create(&self, input: NewProvider) -> RegistryResult<ProviderView> {
        let _w = self.writes.lock().await;
        let kind = input.kind.ok_or_else(|| RegistryError::Invalid("kind is required".into()))?;
        if !PHASE1_KINDS.contains(&kind) {
            return Err(RegistryError::Invalid(format!("provider kind {kind:?} is not supported yet")));
        }
        let info = kind_info(kind);
        let name = match input.name {
            Some(n) => validate_name(&n)?,
            None => self.unique_name(info.display_name),
        };
        let mut settings = json!({});
        if let Some(h) = input.headers.filter(|h| !h.is_empty()) {
            if !info.supports_custom_headers {
                return Err(RegistryError::Invalid(format!("{} does not support custom headers", info.display_name)));
            }
            settings["headers"] = json!(h);
            headers_from_settings(&settings).map_err(invalid)?;
        }
        if let Some(b) = input.binary_path.filter(|b| !b.trim().is_empty()) {
            if info.binary.is_none() {
                return Err(RegistryError::Invalid("binaryPath only applies to CLI providers".into()));
            }
            settings["binaryPath"] = json!(validate_binary_path(&b)?);
        }
        let base_url = match input.base_url.filter(|u| !u.trim().is_empty()) {
            Some(u) if info.supports_base_url => Some(validate_base_url(&u).map_err(invalid)?),
            Some(_) => {
                return Err(RegistryError::Invalid(format!("{} has no configurable base URL", info.display_name)))
            }
            None if matches!(kind, ProviderKind::OpenAiCompatible) => {
                return Err(RegistryError::Invalid("OpenAI-compatible providers need a base URL".into()))
            }
            None => None,
        };
        let default_model = match input.default_model.filter(|m| !m.trim().is_empty()) {
            Some(m) => Some(validate_model_id(&m)?),
            None => info.default_model.map(str::to_owned),
        };
        let api_key = input.api_key.filter(|k| !k.trim().is_empty()).map(|k| validate_api_key(&k)).transpose()?;
        if api_key.is_some() && !(info.requires_api_key || info.api_key_optional) {
            return Err(RegistryError::Invalid(format!("{} does not use an API key", info.display_name)));
        }
        let now = chrono::Utc::now();
        let mut p = Provider {
            id: ProviderId::new(),
            kind,
            name,
            enabled: input.enabled.unwrap_or(true),
            base_url,
            auth_method: info.auth_method,
            credential_ref_id: None,
            secret_ref_id: None,
            is_local: false,
            default_model,
            settings,
            created_at: now,
            updated_at: now,
        };
        p.is_local = is_local(&p);
        build_adapter(&p, &self.deps).map_err(invalid)?; // validate before touching the vault
        if let Some(k) = api_key {
            p.secret_ref_id = Some(self.deps.credentials.store(p.id, &format!("{} API key", p.name), &k).await?);
        }
        if let Err(e) = self.store.insert(&p).await {
            if let Some(s) = p.secret_ref_id {
                let _ = self.deps.credentials.delete(s).await;
            }
            return Err(e.into());
        }
        tracing::info!(provider = %p.id, kind = ?p.kind, name = %p.name, "provider created");
        self.insert_entry(p).await
    }

    /// Updates a provider and hot-swaps its adapter (in-flight streams finish
    /// on the old instance).
    pub async fn update(&self, id: ProviderId, patch: ProviderPatch) -> RegistryResult<ProviderView> {
        let _w = self.writes.lock().await;
        let mut p = self.entry(id)?.config;
        let info = kind_info(p.kind);
        let mut endpoint_changed = false;
        if let Some(n) = patch.name {
            p.name = validate_name(&n)?;
        }
        if let Some(u) = patch.base_url {
            if !info.supports_base_url {
                return Err(RegistryError::Invalid(format!("{} has no configurable base URL", info.display_name)));
            }
            p.base_url = if u.trim().is_empty() { None } else { Some(validate_base_url(&u).map_err(invalid)?) };
            endpoint_changed = true;
        }
        if let Some(h) = patch.headers {
            if !info.supports_custom_headers && !h.is_empty() {
                return Err(RegistryError::Invalid(format!("{} does not support custom headers", info.display_name)));
            }
            if let Some(obj) = p.settings.as_object_mut() {
                obj.insert("headers".into(), json!(h));
            }
            headers_from_settings(&p.settings).map_err(invalid)?;
            endpoint_changed = true;
        }
        if let Some(b) = patch.binary_path {
            if info.binary.is_none() {
                return Err(RegistryError::Invalid("binaryPath only applies to CLI providers".into()));
            }
            if let Some(obj) = p.settings.as_object_mut() {
                if b.trim().is_empty() {
                    obj.remove("binaryPath");
                } else {
                    obj.insert("binaryPath".into(), json!(validate_binary_path(&b)?));
                }
            }
        }
        if let Some(m) = patch.default_model {
            p.default_model = if m.trim().is_empty() { None } else { Some(validate_model_id(&m)?) };
        }
        if let Some(e) = patch.enabled {
            p.enabled = e;
        }
        if p.kind == ProviderKind::OpenAiCompatible && effective_base_url(&p).is_none() {
            return Err(RegistryError::Invalid("OpenAI-compatible providers need a base URL".into()));
        }
        p.is_local = is_local(&p);
        p.updated_at = chrono::Utc::now();
        build_adapter(&p, &self.deps).map_err(invalid)?;
        self.store.update(&p).await?;
        if endpoint_changed {
            self.models.lock().remove(&id);
        }
        self.insert_entry(p).await
    }

    /// Deletes a provider, its stored secret and references in Manager /
    /// routing configs.
    pub async fn delete(&self, id: ProviderId) -> RegistryResult<()> {
        let _w = self.writes.lock().await;
        let p = self.entry(id)?.config;
        if !self.store.delete(id).await? {
            return Err(RegistryError::NotFound(format!("provider {id} not found")));
        }
        self.entries.write().remove(&id);
        self.runtime.lock().remove(&id);
        self.models.lock().remove(&id);
        if let Some(s) = p.secret_ref_id {
            if let Err(e) = self.deps.credentials.delete(s).await {
                tracing::warn!(provider = %id, error = %e, "failed to delete provider secret");
            }
        }
        let mut mgr = self.manager_config().await?;
        let before = mgr.clone();
        if mgr.model.as_ref().is_some_and(|m| m.provider_id == id) {
            mgr.model = None;
        }
        mgr.fallback_chain.retain(|m| m.provider_id != id);
        if mgr != before {
            self.store.set_setting(KEY_MANAGER, &mgr).await?;
        }
        let mut routing = self.routing_config().await?;
        let before = routing.clone();
        routing.rules.retain(|r| r.model.provider_id != id);
        for r in &mut routing.rules {
            r.fallback.retain(|m| m.provider_id != id);
        }
        routing.default_fallback.retain(|m| m.provider_id != id);
        if routing != before {
            self.store.set_setting(KEY_ROUTING, &routing).await?;
        }
        tracing::info!(provider = %id, "provider deleted");
        Ok(())
    }

    /// Stores / replaces the API key of a provider.
    pub async fn set_credential(&self, id: ProviderId, api_key: &str) -> RegistryResult<ProviderView> {
        let _w = self.writes.lock().await;
        let mut p = self.entry(id)?.config;
        let info = kind_info(p.kind);
        if !(info.requires_api_key || info.api_key_optional) {
            return Err(RegistryError::Invalid(format!("{} does not use an API key", info.display_name)));
        }
        let key = validate_api_key(api_key)?;
        let new_ref = self.deps.credentials.store(id, &format!("{} API key", p.name), &key).await?;
        let old = p.secret_ref_id.replace(new_ref);
        p.updated_at = chrono::Utc::now();
        if let Err(e) = self.store.update(&p).await {
            let _ = self.deps.credentials.delete(new_ref).await;
            return Err(e.into());
        }
        if let Some(o) = old {
            if let Err(e) = self.deps.credentials.delete(o).await {
                tracing::warn!(provider = %id, error = %e, "failed to delete replaced secret");
            }
        }
        self.models.lock().remove(&id);
        tracing::info!(provider = %id, "provider credential updated");
        self.insert_entry(p).await
    }

    /// Removes the API key of a provider.
    pub async fn clear_credential(&self, id: ProviderId) -> RegistryResult<ProviderView> {
        let _w = self.writes.lock().await;
        let mut p = self.entry(id)?.config;
        let Some(old) = p.secret_ref_id.take() else { return Ok(self.view(&self.entry(id)?)) };
        p.updated_at = chrono::Utc::now();
        self.store.update(&p).await?;
        if let Err(e) = self.deps.credentials.delete(old).await {
            tracing::warn!(provider = %id, error = %e, "failed to delete secret");
        }
        self.insert_entry(p).await
    }

    /// Active auth + health probe (never spends tokens).
    pub async fn health_check(&self, id: ProviderId) -> RegistryResult<ProviderStatus> {
        let e = self.entry(id)?;
        let timeout = self.options.probe_timeout;
        let auth = tokio::time::timeout(timeout, e.adapter.auth_state()).await.ok();
        let report = match auth {
            Some(AuthStatus::NotConfigured) => HealthReport {
                health: ProviderHealth::Unknown,
                latency_ms: None,
                detail: Some("not configured".into()),
                version: None,
            },
            Some(AuthStatus::NotInstalled { ref binary }) => HealthReport::down(format!("`{binary}` is not installed")),
            _ => tokio::time::timeout(timeout, e.adapter.health())
                .await
                .unwrap_or_else(|_| HealthReport::down(format!("health probe timed out after {}s", timeout.as_secs()))),
        };
        let needs_key = kind_info(e.config.kind).requires_api_key;
        let rate_limited = {
            let rt = self.runtime.lock();
            rt.get(&id).is_some_and(|r| r.windows.blocked_for(Instant::now()).is_some())
        };
        self.update_status(id, false, |s| {
            if let Some(a) = &auth {
                Self::apply_auth(s, a);
            }
            let auth_failed = report.detail.as_deref().is_some_and(|d| d.starts_with("authentication failed"));
            if auth_failed {
                s.auth_state = AuthState::Invalid;
                s.auth_detail = report.detail.clone();
            } else if needs_key && report.health == ProviderHealth::Healthy && s.auth_state == AuthState::Unverified {
                // A successful authenticated probe validates the key.
                s.auth_state = AuthState::Authenticated;
            }
            s.health = if rate_limited && report.health == ProviderHealth::Healthy {
                ProviderHealth::RateLimited
            } else {
                report.health
            };
            s.health_detail = report.detail.clone();
            s.latency_ms = report.latency_ms;
            if report.version.is_some() {
                s.version = report.version.clone();
            }
            s.checked_at = Some(chrono::Utc::now());
        });
        Ok(self.status(id))
    }

    /// Model catalog (cached for the TTL unless `refresh`).
    pub async fn models(&self, id: ProviderId, refresh: bool) -> RegistryResult<ModelsOutcome> {
        let e = self.entry(id)?;
        if !refresh {
            if let Some(c) = self.models.lock().get(&id).filter(|c| c.at.elapsed() < self.options.model_ttl) {
                return Ok(ModelsOutcome { models: c.models.clone(), cached: true, fetched_at: c.fetched_at });
            }
        }
        let fetched = tokio::time::timeout(self.options.probe_timeout, e.adapter.list_models())
            .await
            .unwrap_or_else(|_| Err(ProviderError::Network("model list timed out".into())));
        match fetched {
            Ok(models) => {
                let now = chrono::Utc::now();
                self.store.cache_models(id, &models, now).await?;
                self.models
                    .lock()
                    .insert(id, CachedModels { models: models.clone(), fetched_at: now, at: Instant::now() });
                Ok(ModelsOutcome { models, cached: false, fetched_at: now })
            }
            Err(err) => {
                // Serve a stale cache rather than nothing; surface the error otherwise.
                if let Some(c) = self.models.lock().get(&id) {
                    tracing::warn!(provider = %id, error = %err, "model refresh failed; serving stale cache");
                    return Ok(ModelsOutcome { models: c.models.clone(), cached: true, fetched_at: c.fetched_at });
                }
                Err(err.into())
            }
        }
    }

    fn known_models(&self, p: &Provider) -> Vec<Model> {
        self.models
            .lock()
            .get(&p.id)
            .map(|c| c.models.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| default_models(p.kind, p.id, p.is_local))
    }

    /// Runs a tiny real request ("Reply with OK") to verify the connection.
    /// Spends a few tokens on paid providers — only on explicit user action.
    pub async fn test(&self, id: ProviderId, model: Option<String>) -> RegistryResult<TestOutcome> {
        let e = self.entry(id)?;
        let model = match model.filter(|m| !m.trim().is_empty()) {
            Some(m) => validate_model_id(&m)?,
            None => match e.config.default_model.clone() {
                Some(m) => m,
                None => self
                    .models(id, false)
                    .await
                    .ok()
                    .and_then(|m| m.models.first().map(|m| m.id.clone()))
                    .ok_or_else(|| RegistryError::Invalid("no model available — pick a model first".into()))?,
            },
        };
        let mut req = ChatRequest::new(
            model.clone(),
            vec![ChatMessage::user("Connection test from Orkestr. Reply with exactly: OK")],
        );
        req.max_output_tokens = Some(16);
        let start = Instant::now();
        let mut first: Option<u64> = None;
        let mut text = String::new();
        let mut usage = None;
        let mut error = None;
        let run = async {
            match e.adapter.chat(req, CancellationToken::new()).await {
                Err(err) => error = Some(err),
                Ok(mut s) => {
                    while let Some(ev) = s.next().await {
                        first.get_or_insert_with(|| start.elapsed().as_millis() as u64);
                        match ev {
                            ChatEvent::TextDelta(t) if text.len() < 400 => text.push_str(&t),
                            ChatEvent::Usage(u) => usage = Some(u),
                            ChatEvent::Error(err) => {
                                error = Some(err);
                                break;
                            }
                            ChatEvent::Done { .. } => break,
                            _ => {}
                        }
                    }
                }
            }
        };
        if tokio::time::timeout(Duration::from_secs(120), run).await.is_err() {
            error = Some(ProviderError::Network("connection test timed out after 120s".into()));
        }
        let latency_ms = first.unwrap_or_else(|| start.elapsed().as_millis() as u64);
        let model_ref = ModelRef { provider_id: id, model: model.clone() };
        let outcome = match &error {
            None => AttemptOutcome::Success { latency_ms },
            Some(err) => AttemptOutcome::Failure(err.clone()),
        };
        self.record(&model_ref, &outcome);
        if error.is_none() {
            self.update_status(id, false, |s| {
                if s.auth_state != AuthState::Authenticated {
                    s.auth_state = AuthState::Authenticated;
                }
            });
        }
        let sample = (!text.trim().is_empty()).then(|| text.chars().take(200).collect::<String>());
        Ok(TestOutcome {
            ok: error.is_none(),
            cost_usd: usage.as_ref().and_then(|u| estimate_cost(&model, u)),
            model,
            latency_ms,
            sample,
            error,
            usage,
        })
    }

    /// Records an attempt outcome: circuit breaker, limit windows, latency,
    /// status transitions and events.
    pub fn record(&self, model: &ModelRef, outcome: &AttemptOutcome) {
        let id = model.provider_id;
        let now = Instant::now();
        let mut rate_event = None;
        let update: Box<dyn FnOnce(&mut ProviderStatus) + Send> = {
            let mut rt = self.runtime.lock();
            let r = rt.entry(id).or_default();
            match outcome {
                AttemptOutcome::Success { latency_ms } => {
                    r.circuit.record_success();
                    r.windows.clear();
                    r.latency.record(*latency_ms);
                    Box::new(|s: &mut ProviderStatus| {
                        s.health = ProviderHealth::Healthy;
                        s.health_detail = None;
                        s.rate_limited_until = None;
                        s.quota_exhausted_until = None;
                        if s.auth_state == AuthState::Invalid {
                            s.auth_state = AuthState::Authenticated;
                        }
                    })
                }
                AttemptOutcome::Failure(err) => {
                    let opened = err.trips_circuit() && r.circuit.record_failure(now);
                    let circuit_open = r.circuit.is_open(now);
                    let msg = err.to_string();
                    match err {
                        ProviderError::RateLimited { retry_after_secs } => {
                            let wait = retry_after_secs.map_or(LimitWindows::DEFAULT_RATE_WAIT, Duration::from_secs);
                            r.windows.rate_limited_until = Some(now + wait);
                            rate_event = Some((model.model.clone(), *retry_after_secs));
                            Box::new(move |s: &mut ProviderStatus| {
                                s.health = ProviderHealth::RateLimited;
                                s.health_detail = Some(msg);
                                s.rate_limited_until = Some(to_ts(wait));
                            })
                        }
                        ProviderError::QuotaExceeded(_) => {
                            r.windows.quota_until = Some(now + LimitWindows::DEFAULT_QUOTA_WAIT);
                            Box::new(move |s: &mut ProviderStatus| {
                                s.health = ProviderHealth::Degraded;
                                s.health_detail = Some(msg);
                                s.quota_exhausted_until = Some(to_ts(LimitWindows::DEFAULT_QUOTA_WAIT));
                            })
                        }
                        ProviderError::Auth(_) => Box::new(move |s: &mut ProviderStatus| {
                            s.auth_state = AuthState::Invalid;
                            s.auth_detail = Some(msg);
                        }),
                        ProviderError::Network(_) | ProviderError::Server(_) => {
                            Box::new(move |s: &mut ProviderStatus| {
                                s.health = if circuit_open { ProviderHealth::Down } else { ProviderHealth::Degraded };
                                s.health_detail = Some(if opened {
                                    format!("circuit open after repeated failures: {msg}")
                                } else {
                                    msg
                                });
                            })
                        }
                        _ => Box::new(|_s: &mut ProviderStatus| {}),
                    }
                }
            }
        };
        if let Some((m, retry)) = rate_event {
            self.sink.emit(ProviderEvent::RateLimited { provider_id: id, model: Some(m), retry_after_secs: retry });
        }
        self.update_status(id, false, update);
    }

    /// Routing candidates (every known model of every provider, plus `extra`).
    #[must_use]
    pub fn candidates(&self, extra: &[ModelRef]) -> Vec<Candidate> {
        let now = Instant::now();
        let entries: Vec<Entry> = self.entries.read().values().cloned().collect();
        let mut out = Vec::new();
        for e in &entries {
            let (status, circuit_open, blocked, quota, latency) = {
                let mut rt = self.runtime.lock();
                let r = rt.entry(e.config.id).or_default();
                (
                    r.status.clone(),
                    r.circuit.is_open(now),
                    r.windows.blocked_for(now).map(|d| d.as_secs().max(1)),
                    r.windows.quota_exhausted(now),
                    r.latency.average_ms(),
                )
            };
            let usable = !matches!(status.auth_state, AuthState::Unconfigured | AuthState::Invalid)
                && !(kind_info(e.config.kind).requires_api_key && e.config.secret_ref_id.is_none());
            let desc = e.adapter.descriptor();
            let mut models = self.known_models(&e.config);
            for x in extra.iter().filter(|x| x.provider_id == e.config.id) {
                if !models.iter().any(|m| m.id == x.model) {
                    models.push(Model {
                        provider_id: e.config.id,
                        id: x.model.clone(),
                        display_name: x.model.clone(),
                        context_window: desc.max_context_tokens,
                        max_output_tokens: None,
                        capabilities: desc.capabilities,
                        pricing: crate::pricing::pricing_for(&x.model),
                        is_local: desc.is_local,
                    });
                }
            }
            for m in models {
                // Scripted mock behaviours (failures, echo, slow) are only routed
                // when explicitly referenced; automatic routing uses `mock-smart`.
                let explicit = extra.iter().any(|x| x.provider_id == e.config.id && x.model == m.id);
                if e.config.kind == ProviderKind::Mock && m.id != "mock-smart" && !explicit {
                    continue;
                }
                out.push(Candidate {
                    model: ModelRef { provider_id: e.config.id, model: m.id },
                    kind: e.config.kind,
                    is_local: e.config.is_local,
                    enabled: e.config.enabled,
                    usable,
                    health: status.health,
                    blocked_for_secs: blocked,
                    quota_exhausted: quota,
                    circuit_open,
                    capabilities: m.capabilities,
                    context_window: m.context_window,
                    pricing: m.pricing,
                    latency_ms: latency,
                    history: None,
                });
            }
        }
        out
    }

    /// Wires historical benchmark statistics into auto-routing (§22).
    pub fn set_stats_source(&self, source: Arc<dyn crate::history::ModelStatsSource>) {
        *self.stats.write() = Some(source);
    }

    /// Context window (tokens) of `model` from the cached model list or the
    /// provider descriptor (model-aware context budgeting, §20).
    #[must_use]
    pub fn model_context_window(&self, model: &ModelRef) -> Option<u32> {
        self.candidates(std::slice::from_ref(model))
            .into_iter()
            .find(|c| &c.model == model)
            .map(|c| c.context_window)
            .filter(|w| *w > 0)
    }

    /// Candidates with historical statistics for `purpose` attached.
    #[must_use]
    pub fn candidates_with_history(&self, extra: &[ModelRef], purpose: Option<&str>) -> Vec<Candidate> {
        let mut out = self.candidates(extra);
        if let Some(src) = self.stats.read().clone() {
            for c in &mut out {
                c.history = src.history(&c.model, purpose);
            }
        }
        out
    }

    /// Routes a request with the stored routing configuration.
    pub async fn route(&self, req: &RouteRequest) -> RegistryResult<Vec<ModelRef>> {
        let cfg = self.routing_config().await?;
        let mut extra = router::manual_chain(req, &cfg);
        extra.extend(req.exclude.iter().cloned());
        Ok(router::route(req, &self.candidates_with_history(&extra, req.purpose.as_deref()), &cfg))
    }

    /// Streams a chat over a fallback chain (see [`chat_with_fallback`]).
    pub fn chat(
        self: &Arc<Self>,
        chain: Vec<ModelRef>,
        request: ChatRequest,
        hook: Option<Arc<dyn HandoffHook>>,
        cancel: CancellationToken,
    ) -> ChatStream {
        let source: Arc<dyn AdapterSource> = Arc::clone(self) as Arc<dyn AdapterSource>;
        chat_with_fallback(source, chain, request, hook, self.options.fallback, cancel)
    }

    /// AI Manager settings (§5).
    pub async fn manager_config(&self) -> RegistryResult<ManagerConfig> {
        Ok(self.store.setting(KEY_MANAGER).await?.unwrap_or_default())
    }

    fn check_ref(&self, m: &ModelRef, what: &str) -> RegistryResult<()> {
        validate_model_id(&m.model)?;
        if self.entries.read().contains_key(&m.provider_id) {
            Ok(())
        } else {
            Err(RegistryError::Invalid(format!("{what} references unknown provider {}", m.provider_id)))
        }
    }

    /// Validates and stores the Manager settings.
    pub async fn set_manager_config(&self, cfg: ManagerConfig) -> RegistryResult<ManagerConfig> {
        if let Some(m) = &cfg.model {
            self.check_ref(m, "manager model")?;
        }
        for m in &cfg.fallback_chain {
            self.check_ref(m, "fallback chain")?;
        }
        if cfg.fallback_chain.len() > 16 {
            return Err(RegistryError::Invalid("fallback chain is limited to 16 models".into()));
        }
        if !(1..=64).contains(&cfg.max_parallel_agents) {
            return Err(RegistryError::Invalid("maxParallelAgents must be 1..=64".into()));
        }
        if cfg.budget_usd.is_some_and(|b| !b.is_finite() || b < 0.0) {
            return Err(RegistryError::Invalid("budgetUsd must be a non-negative number".into()));
        }
        if cfg.time_limit_mins.is_some_and(|t| t == 0 || t > 24 * 60) {
            return Err(RegistryError::Invalid("timeLimitMins must be 1..=1440".into()));
        }
        if cfg.system_instructions.chars().count() > 20_000 {
            return Err(RegistryError::Invalid("system instructions are limited to 20000 characters".into()));
        }
        self.store.set_setting(KEY_MANAGER, &cfg).await?;
        Ok(cfg)
    }

    /// Routing settings (§7).
    pub async fn routing_config(&self) -> RegistryResult<RoutingConfig> {
        Ok(self.store.setting(KEY_ROUTING).await?.unwrap_or_default())
    }

    /// Validates and stores the routing settings.
    pub async fn set_routing_config(&self, cfg: RoutingConfig) -> RegistryResult<RoutingConfig> {
        let mut purposes: Vec<String> = Vec::new();
        for r in &cfg.rules {
            let p = r.purpose.trim();
            if p.is_empty() || p.chars().count() > 40 {
                return Err(RegistryError::Invalid("routing purpose must be 1..=40 characters".into()));
            }
            if purposes.iter().any(|x| x.eq_ignore_ascii_case(p)) {
                return Err(RegistryError::Invalid(format!("duplicate routing rule for `{p}`")));
            }
            purposes.push(p.to_owned());
            self.check_ref(&r.model, "routing rule")?;
            for m in &r.fallback {
                self.check_ref(m, "routing fallback")?;
            }
        }
        for m in &cfg.default_fallback {
            self.check_ref(m, "default fallback")?;
        }
        if cfg.max_rate_limit_wait_secs > 3600 {
            return Err(RegistryError::Invalid("maxRateLimitWaitSecs must be ≤ 3600".into()));
        }
        let mut cfg = cfg;
        for r in &mut cfg.rules {
            r.purpose = r.purpose.trim().to_owned();
        }
        self.store.set_setting(KEY_ROUTING, &cfg).await?;
        Ok(cfg)
    }
}

trait PastExt {
    fn is_none_or_past(&self, now: Instant) -> bool;
}

impl PastExt for Option<Instant> {
    fn is_none_or_past(&self, now: Instant) -> bool {
        self.is_none_or(|t| t <= now)
    }
}

impl AdapterSource for ProviderRegistry {
    fn adapter(&self, provider: &ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.entries.read().get(provider).filter(|e| e.config.enabled).map(|e| Arc::clone(&e.adapter))
    }

    fn observe(&self, model: &ModelRef, outcome: &AttemptOutcome) {
        // Rate-limit events are emitted by the executor itself.
        if let AttemptOutcome::Failure(ProviderError::RateLimited { retry_after_secs }) = outcome {
            let wait = retry_after_secs.map_or(LimitWindows::DEFAULT_RATE_WAIT, Duration::from_secs);
            {
                let mut rt = self.runtime.lock();
                rt.entry(model.provider_id).or_default().windows.rate_limited_until = Some(Instant::now() + wait);
            }
            let msg = ProviderError::RateLimited { retry_after_secs: *retry_after_secs }.to_string();
            self.update_status(model.provider_id, false, |s| {
                s.health = ProviderHealth::RateLimited;
                s.health_detail = Some(msg);
                s.rate_limited_until = Some(to_ts(wait));
            });
            return;
        }
        self.record(model, outcome);
    }

    fn emit(&self, event: ProviderEvent) {
        self.sink.emit(event);
    }
}

/// Helper used by tests and the core: a settings JSON value with headers.
#[must_use]
pub fn headers_json(headers: &BTreeMap<String, String>) -> Value {
    json!({ "headers": headers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::mock::MockOptions;
    use crate::credentials::MemoryCredentials;
    use orkestr_domain::RoutingRule;

    #[derive(Default)]
    struct Sink(Mutex<Vec<ProviderEvent>>);
    impl ProviderEventSink for Sink {
        fn emit(&self, e: ProviderEvent) {
            self.0.lock().push(e);
        }
    }

    async fn registry() -> (Arc<ProviderRegistry>, Arc<MemoryCredentials>, Arc<Sink>, Db) {
        let db = Db::open_in_memory().unwrap();
        db.migrate(migrations()).await.unwrap();
        let creds = Arc::new(MemoryCredentials::new());
        let sink = Arc::new(Sink::default());
        let mut deps = AdapterDeps::new(creds.clone());
        deps.mock = MockOptions { chunk_delay: Duration::ZERO };
        let opts = RegistryOptions { detect_on_first_start: false, ..RegistryOptions::default() };
        let reg = Arc::new(ProviderRegistry::with_options(db.clone(), deps, sink.clone(), opts));
        reg.init().await.unwrap();
        (reg, creds, sink, db)
    }

    #[tokio::test]
    async fn first_start_registers_mock_once() {
        let (reg, creds, _sink, db) = registry().await;
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].provider.kind, ProviderKind::Mock);
        assert_eq!(list[0].provider.name, "Mock (offline demo)");
        assert!(list[0].provider.is_local);
        // A second registry over the same DB loads but does not re-bootstrap.
        let reg2 = ProviderRegistry::with_options(
            db,
            AdapterDeps::new(creds),
            Arc::new(Sink::default()),
            RegistryOptions { detect_on_first_start: false, ..RegistryOptions::default() },
        );
        reg2.init().await.unwrap();
        assert_eq!(reg2.list().len(), 1);
    }

    #[tokio::test]
    async fn crud_credentials_and_hot_swap() {
        let (reg, creds, sink, _db) = registry().await;
        let v = reg
            .create(NewProvider {
                kind: Some(ProviderKind::OpenAi),
                api_key: Some("sk-test-123".into()),
                ..NewProvider::default()
            })
            .await
            .unwrap();
        assert!(v.has_credential);
        assert_eq!(creds.len(), 1);
        assert_eq!(v.provider.default_model.as_deref(), Some("gpt-4.1-mini"));
        assert_eq!(v.status.auth_state, AuthState::Unverified);
        // Secrets never appear in the persisted config.
        assert!(!serde_json::to_string(&v.provider).unwrap().contains("sk-test-123"));
        let id = v.provider.id;
        let old_adapter = reg.adapter(id).unwrap();
        let v = reg.set_credential(id, "sk-new-456").await.unwrap();
        assert_eq!(creds.len(), 1, "old secret deleted");
        assert!(!Arc::ptr_eq(&old_adapter, &reg.adapter(id).unwrap()), "adapter hot-swapped");
        let v2 = reg
            .update(
                id,
                ProviderPatch { name: Some("OpenAI (work)".into()), enabled: Some(false), ..ProviderPatch::default() },
            )
            .await
            .unwrap();
        assert_eq!(v2.provider.name, "OpenAI (work)");
        assert!(!v2.provider.enabled);
        assert!(v.has_credential);
        let v3 = reg.clear_credential(id).await.unwrap();
        assert!(!v3.has_credential);
        assert!(creds.is_empty());
        assert!(matches!(
            reg.create(NewProvider {
                kind: Some(ProviderKind::OpenAi),
                name: Some("openai (WORK)".into()),
                ..NewProvider::default()
            })
            .await,
            Err(RegistryError::Conflict(_))
        ));
        assert!(matches!(
            reg.create(NewProvider { kind: Some(ProviderKind::OpenAiCompatible), ..NewProvider::default() }).await,
            Err(RegistryError::Invalid(_))
        ));
        assert!(matches!(
            reg.create(NewProvider {
                kind: Some(ProviderKind::Mock),
                api_key: Some("x".into()),
                ..NewProvider::default()
            })
            .await,
            Err(RegistryError::Invalid(_))
        ));
        reg.delete(id).await.unwrap();
        assert!(matches!(reg.get(id), Err(RegistryError::NotFound(_))));
        assert!(sink
            .0
            .lock()
            .iter()
            .any(|e| matches!(e, ProviderEvent::StatusChanged { provider_id, .. } if *provider_id == id)));
    }

    #[tokio::test]
    async fn mock_test_health_models_and_routing() {
        let (reg, _creds, _sink, _db) = registry().await;
        let mock = reg.list()[0].provider.id;
        let st = reg.health_check(mock).await.unwrap();
        assert_eq!(st.health, ProviderHealth::Healthy);
        assert_eq!(st.auth_state, AuthState::Authenticated);
        let m = reg.models(mock, false).await.unwrap();
        assert!(!m.cached);
        assert!(m.models.iter().any(|m| m.id == "mock-smart"));
        assert!(reg.models(mock, false).await.unwrap().cached);
        let t = reg.test(mock, None).await.unwrap();
        assert!(t.ok, "{t:?}");
        assert_eq!(t.model, "mock-smart");
        assert!(t.sample.unwrap().contains("Mock (offline demo)"));

        // Unconfigured OpenAI provider is not routable; mock is.
        reg.create(NewProvider { kind: Some(ProviderKind::OpenAi), ..NewProvider::default() }).await.unwrap();
        let chain =
            reg.route(&RouteRequest { purpose: Some("Backend".into()), ..RouteRequest::default() }).await.unwrap();
        assert!(!chain.is_empty());
        assert!(chain.iter().all(|m| m.provider_id == mock));
        // Routing rules referencing unknown providers are rejected.
        let bogus = ModelRef { provider_id: ProviderId::new(), model: "x".into() };
        let bad = RoutingConfig {
            rules: vec![RoutingRule { purpose: "Backend".into(), model: bogus, fallback: vec![] }],
            ..RoutingConfig::default()
        };
        assert!(matches!(reg.set_routing_config(bad).await, Err(RegistryError::Invalid(_))));
        let good = RoutingConfig {
            rules: vec![RoutingRule {
                purpose: "Backend".into(),
                model: ModelRef { provider_id: mock, model: "mock-echo".into() },
                fallback: vec![],
            }],
            ..RoutingConfig::default()
        };
        reg.set_routing_config(good).await.unwrap();
        let chain =
            reg.route(&RouteRequest { purpose: Some("backend".into()), ..RouteRequest::default() }).await.unwrap();
        assert_eq!(chain, vec![ModelRef { provider_id: mock, model: "mock-echo".into() }]);
    }

    #[tokio::test]
    async fn rate_limits_feed_status_and_router() {
        let (reg, _creds, sink, _db) = registry().await;
        let mock = reg.list()[0].provider.id;
        let chain = vec![
            ModelRef { provider_id: mock, model: "mock-rate-limit".into() },
            ModelRef { provider_id: mock, model: "mock-echo".into() },
        ];
        let mut req = ChatRequest::new("x", vec![ChatMessage::user("hey")]);
        req.max_output_tokens = Some(10);
        let opts = FallbackOptions { max_rate_limit_wait: Duration::ZERO, same_model_retries: 0 };
        let src: Arc<dyn AdapterSource> = reg.clone();
        let out: Vec<ChatEvent> =
            chat_with_fallback(src, chain, req, None, opts, CancellationToken::new()).collect().await;
        assert!(matches!(out.last(), Some(ChatEvent::Done { .. })));
        let events = sink.0.lock().clone();
        assert!(events.iter().any(|e| matches!(e, ProviderEvent::RateLimited { .. })));
        assert!(events.iter().any(|e| matches!(e, ProviderEvent::Fallback { reason, .. } if reason == "rate_limited")));
        // Success afterwards cleared the window.
        assert_eq!(reg.get(mock).unwrap().status.health, ProviderHealth::Healthy);
        // Quota failures block the provider in the router.
        reg.record(
            &ModelRef { provider_id: mock, model: "mock-smart".into() },
            &AttemptOutcome::Failure(ProviderError::QuotaExceeded("x".into())),
        );
        assert!(reg.get(mock).unwrap().status.quota_exhausted_until.is_some());
        let routed = reg.route(&RouteRequest::default()).await.unwrap();
        assert!(routed.is_empty());
    }

    #[tokio::test]
    async fn circuit_opens_after_repeated_server_errors() {
        let (reg, _creds, _sink, _db) = registry().await;
        let mock = reg.list()[0].provider.id;
        let m = ModelRef { provider_id: mock, model: "mock-server-error".into() };
        for _ in 0..5 {
            reg.record(&m, &AttemptOutcome::Failure(ProviderError::Server("boom".into())));
        }
        let st = reg.get(mock).unwrap().status;
        assert!(st.circuit_open);
        assert_eq!(st.health, ProviderHealth::Down);
        assert!(reg.candidates(&[]).iter().all(|c| c.circuit_open));
    }

    #[tokio::test]
    async fn delete_prunes_manager_and_routing_refs() {
        let (reg, _creds, _sink, _db) = registry().await;
        let v = reg.create(NewProvider { kind: Some(ProviderKind::Ollama), ..NewProvider::default() }).await.unwrap();
        let id = v.provider.id;
        assert!(v.provider.is_local);
        let r = ModelRef { provider_id: id, model: "llama3.2".into() };
        reg.set_manager_config(ManagerConfig {
            model: Some(r.clone()),
            fallback_chain: vec![r.clone()],
            ..ManagerConfig::default()
        })
        .await
        .unwrap();
        reg.set_routing_config(RoutingConfig { default_fallback: vec![r], ..RoutingConfig::default() }).await.unwrap();
        reg.delete(id).await.unwrap();
        let mgr = reg.manager_config().await.unwrap();
        assert!(mgr.model.is_none() && mgr.fallback_chain.is_empty());
        assert!(reg.routing_config().await.unwrap().default_fallback.is_empty());
        assert!(matches!(
            reg.set_manager_config(ManagerConfig { max_parallel_agents: 0, ..ManagerConfig::default() }).await,
            Err(RegistryError::Invalid(_))
        ));
    }
}

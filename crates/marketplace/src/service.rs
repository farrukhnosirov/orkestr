//! [`Marketplace`]: facade used by `core/src/rpc/marketplace.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use orkestr_domain::{now, Actor, AuditOutcome, PluginId, SecretScope};
use orkestr_events::EventBus;
use orkestr_plugins::PluginService;
use orkestr_protocol::methods::marketplace::{
    AddSourceParams, EntryDetailsResult, InstallEntryParams, InstallEntryResult, InstalledPackage, KeyTrust,
    MarketplaceSearchParams, MarketplaceUpdatedPayload, PackageKind, PublishPackageParams, PublisherKey, RegistryEntry,
    RegistrySource, RegistrySourceKind,
};
use orkestr_protocol::methods::plugins::{InstallPluginParams, InstallPluginResult};
use orkestr_protocol::{EventKind, NewEvent};
use orkestr_security::{AccessContext, NewAudit, Security, SecurityError};
use orkestr_storage::Db;

use crate::index::{self, IndexDocument, IndexEntry, IndexKey};
use crate::publish::{self, PackageMeta, PublishRequest};
use crate::signing::{self, Ed25519Verifier, KeyPair, SignatureVerifier};
use crate::sources::{
    self, DirectoryTransport, HttpTransport, OfficialTransport, RegistryTransport, OFFICIAL_SOURCE_ID,
};
use crate::store::{SourceRow, Store};
use crate::MarketplaceError;

/// Vault secret holding the local publisher key seed.
pub const PUBLISHER_KEY_SECRET: &str = "MARKETPLACE_PUBLISHER_KEY";

/// Services the marketplace needs.
#[derive(Clone)]
pub struct MarketplaceDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Vault (publisher key), audit.
    pub security: Arc<Security>,
    /// Plugin host (installs).
    pub plugins: Arc<PluginService>,
    /// `<data_dir>/marketplace` (cache).
    pub data_dir: PathBuf,
}

/// Imports agent templates (core: `templates.import` / `templates.delete`).
#[async_trait]
pub trait TemplateSink: Send + Sync {
    /// Imports a template envelope; returns the new template id.
    async fn import(&self, json: &str, accept_permissions: bool) -> Result<String, MarketplaceError>;
    /// Deletes an imported template.
    async fn remove(&self, template_id: &str) -> Result<(), MarketplaceError>;
}

/// Installs plugin / widget packages (core: `plugins.install` / `plugins.uninstall`).
#[async_trait]
pub trait PluginSink: Send + Sync {
    /// Installs a verified package.
    async fn install(&self, bytes: &[u8], source: String) -> Result<InstallPluginResult, MarketplaceError>;
    /// Uninstalls a plugin.
    async fn uninstall(&self, plugin_id: &str) -> Result<(), MarketplaceError>;
}

fn plugin_err(e: orkestr_plugins::PluginError) -> MarketplaceError {
    use orkestr_plugins::PluginError as P;
    match e {
        P::NotFound(m) => MarketplaceError::NotFound(m),
        P::Invalid(m) | P::Package(m) => MarketplaceError::Invalid(m),
        P::Denied(m) => MarketplaceError::Denied(m),
        P::Conflict(m) => MarketplaceError::Conflict(m),
        P::Signature(m) => MarketplaceError::Signature(m),
        P::Unimplemented(f) => MarketplaceError::Unimplemented(f),
        other => MarketplaceError::Internal(other.to_string()),
    }
}

#[async_trait]
impl PluginSink for PluginService {
    async fn install(&self, bytes: &[u8], source: String) -> Result<InstallPluginResult, MarketplaceError> {
        PluginService::install(
            self,
            InstallPluginParams { path: None, package_base64: Some(B64.encode(bytes)), source: Some(source) },
        )
        .await
        .map_err(plugin_err)
    }

    async fn uninstall(&self, plugin_id: &str) -> Result<(), MarketplaceError> {
        let id = PluginId::parse(plugin_id).map_err(|e| MarketplaceError::Invalid(e.to_string()))?;
        PluginService::uninstall(self, id).await.map_err(plugin_err)
    }
}

/// Install targets for one call.
#[derive(Clone, Copy)]
pub struct Installers<'a> {
    /// Agent templates.
    pub templates: &'a dyn TemplateSink,
    /// Plugins and widget packages.
    pub plugins: &'a dyn PluginSink,
}

impl std::fmt::Debug for Installers<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Installers").finish_non_exhaustive()
    }
}

/// Marketplace service.
pub struct Marketplace {
    deps: MarketplaceDeps,
    store: Store,
    verifier: Arc<dyn SignatureVerifier>,
    official_key: String,
    official: Arc<dyn RegistryTransport>,
    lock: tokio::sync::Mutex<()>,
    init: tokio::sync::OnceCell<()>,
}

impl std::fmt::Debug for Marketplace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Marketplace").field("data_dir", &self.deps.data_dir).finish_non_exhaustive()
    }
}

type R<T> = Result<T, MarketplaceError>;

fn denied(m: impl Into<String>) -> MarketplaceError {
    MarketplaceError::Denied(m.into())
}

fn sec_err(e: SecurityError) -> MarketplaceError {
    match e {
        SecurityError::VaultLocked => denied("the vault is locked"),
        other => MarketplaceError::Internal(other.to_string()),
    }
}

/// Vault secret name holding the publish token of a source.
#[must_use]
pub fn source_token_secret(source_id: &str) -> String {
    let id: String =
        source_id.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    format!("MARKETPLACE_SOURCE_TOKEN_{id}")
}

impl Marketplace {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: MarketplaceDeps) -> Self {
        Self::with_official(deps, sources::OFFICIAL_PUBLIC_KEY, Arc::new(OfficialTransport))
    }

    /// Creates the service with another official catalogue (tests).
    #[must_use]
    pub fn with_official(deps: MarketplaceDeps, official_key: &str, official: Arc<dyn RegistryTransport>) -> Self {
        let store = Store::new(deps.db.clone());
        Self {
            deps,
            store,
            verifier: Arc::new(Ed25519Verifier),
            official_key: official_key.to_owned(),
            official,
            lock: tokio::sync::Mutex::new(()),
            init: tokio::sync::OnceCell::new(),
        }
    }

    /// Replaces the signature verifier (unification with `orkestr_plugins::signing`).
    pub fn set_verifier(&mut self, verifier: Arc<dyn SignatureVerifier>) {
        self.verifier = verifier;
    }

    async fn ensure_init(&self) -> R<()> {
        self.init
            .get_or_try_init(|| async {
                let rows = self.store.sources().await?;
                if !rows.iter().any(|r| r.source.id == OFFICIAL_SOURCE_ID) {
                    self.store
                        .put_source(SourceRow {
                            source: RegistrySource {
                                id: OFFICIAL_SOURCE_ID.into(),
                                name: "Orkestr official".into(),
                                kind: RegistrySourceKind::Official,
                                location: "bundled".into(),
                                enabled: true,
                                entries: 0,
                                last_refresh: None,
                                last_error: None,
                            },
                            pinned_key: Some(self.official_key.clone()),
                            index_json: None,
                            index_sig: None,
                        })
                        .await?;
                    self.refresh_locked(Some(OFFICIAL_SOURCE_ID)).await?;
                }
                Ok::<(), MarketplaceError>(())
            })
            .await
            .map(|_| ())
    }

    async fn emit(&self, source_id: Option<&str>, change: &str) {
        let payload = MarketplaceUpdatedPayload { source_id: source_id.map(str::to_owned), change: change.to_owned() };
        if let Err(e) = self.deps.events.publish(NewEvent::new(EventKind::MarketplaceUpdated(payload))).await {
            tracing::warn!(error = %e, "MarketplaceUpdated not published");
        }
    }

    async fn audit(&self, action: &str, resource: &str, details: serde_json::Value) {
        self.deps
            .security
            .audit
            .record(NewAudit::new(self.deps.security.user(), action, resource, AuditOutcome::Executed).details(details))
            .await;
    }

    fn transport(&self, s: &RegistrySource) -> R<Arc<dyn RegistryTransport>> {
        Ok(match s.kind {
            RegistrySourceKind::Official => Arc::clone(&self.official),
            RegistrySourceKind::Directory => Arc::new(DirectoryTransport::new(&s.location)),
            RegistrySourceKind::Http => Arc::new(HttpTransport::new(&s.location)?),
        })
    }

    async fn source_row(&self, id: &str) -> R<SourceRow> {
        self.store
            .sources()
            .await?
            .into_iter()
            .find(|r| r.source.id == id)
            .ok_or_else(|| MarketplaceError::NotFound(format!("source {id}")))
    }

    /// Cached verified index of a source (re-verified on read).
    fn cached_index(&self, row: &SourceRow) -> Option<IndexDocument> {
        let (json, sig) = (row.index_json.as_ref()?, row.index_sig.as_ref()?);
        index::verify_index(json, sig, self.verifier.as_ref(), row.pinned_key.as_deref()).ok().map(|v| v.doc)
    }

    // ---- sources -------------------------------------------------------

    /// Configured sources.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn sources(&self) -> R<Vec<RegistrySource>> {
        self.ensure_init().await?;
        Ok(self.store.sources().await?.into_iter().map(|r| r.source).collect())
    }

    /// Adds a directory / HTTP source and refreshes it (refresh errors are
    /// reported on the source, not as a failure).
    ///
    /// # Errors
    /// Invalid location, duplicate, vault failure.
    pub async fn add_source(&self, p: AddSourceParams) -> R<RegistrySource> {
        self.ensure_init().await?;
        let location = match p.kind {
            RegistrySourceKind::Official => {
                return Err(MarketplaceError::Invalid("the official source is built in".into()))
            }
            RegistrySourceKind::Directory => {
                let path = PathBuf::from(p.location.trim());
                if !path.is_absolute() {
                    return Err(MarketplaceError::Invalid("a directory source needs an absolute path".into()));
                }
                path.to_string_lossy().into_owned()
            }
            RegistrySourceKind::Http => sources::normalize_http_base(&p.location)?,
        };
        let rows = self.store.sources().await?;
        if rows.iter().any(|r| r.source.location == location) {
            return Err(MarketplaceError::Conflict(format!("source {location} already exists")));
        }
        let prefix = if p.kind == RegistrySourceKind::Http { "http" } else { "dir" };
        let id = format!("{prefix}-{}", &signing::sha256_hex(location.as_bytes())[..8]);
        let name = p.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| location.clone());
        if let Some(token) = p.token.filter(|t| !t.trim().is_empty()) {
            self.deps
                .security
                .secrets
                .set(
                    &source_token_secret(&id),
                    SecretScope::Global,
                    zeroize::Zeroizing::new(token),
                    self.deps.security.user(),
                )
                .await
                .map_err(sec_err)?;
        }
        let source = RegistrySource {
            id: id.clone(),
            name,
            kind: p.kind,
            location,
            enabled: true,
            entries: 0,
            last_refresh: None,
            last_error: None,
        };
        self.store.put_source(SourceRow { source, pinned_key: None, index_json: None, index_sig: None }).await?;
        self.audit("marketplace.addSource", &id, serde_json::json!({})).await;
        {
            let _g = self.lock.lock().await;
            self.refresh_locked(Some(&id)).await?;
        }
        self.emit(Some(&id), "sourceAdded").await;
        Ok(self.source_row(&id).await?.source)
    }

    /// Removes a source (the official one cannot be removed).
    ///
    /// # Errors
    /// Unknown / official source.
    pub async fn remove_source(&self, id: &str) -> R<()> {
        self.ensure_init().await?;
        if id == OFFICIAL_SOURCE_ID {
            return Err(MarketplaceError::Invalid("the official source cannot be removed".into()));
        }
        if !self.store.delete_source(id).await? {
            return Err(MarketplaceError::NotFound(format!("source {id}")));
        }
        self.emit(Some(id), "sourceRemoved").await;
        Ok(())
    }

    /// Fetches and verifies indexes (one source or all enabled ones).
    ///
    /// # Errors
    /// Unknown source / storage failure (verification failures are recorded
    /// as `lastError`; the previous verified index stays in use).
    pub async fn refresh(&self, source_id: Option<&str>) -> R<Vec<RegistrySource>> {
        self.ensure_init().await?;
        {
            let _g = self.lock.lock().await;
            self.refresh_locked(source_id).await?;
        }
        self.emit(source_id, "refreshed").await;
        self.sources().await
    }

    async fn refresh_locked(&self, source_id: Option<&str>) -> R<()> {
        let rows = self.store.sources().await?;
        if let Some(id) = source_id {
            if !rows.iter().any(|r| r.source.id == id) {
                return Err(MarketplaceError::NotFound(format!("source {id}")));
            }
        }
        for mut row in rows {
            if source_id.is_some_and(|id| id != row.source.id) || (source_id.is_none() && !row.source.enabled) {
                continue;
            }
            match self.fetch_index(&row).await {
                Ok((json, sig, signer_key, doc)) => {
                    row.pinned_key.get_or_insert(signer_key);
                    row.index_json = Some(json);
                    row.index_sig = Some(sig);
                    row.source.entries = u32::try_from(doc.entries.len()).unwrap_or(u32::MAX);
                    row.source.last_refresh = Some(now());
                    row.source.last_error = None;
                    self.ingest(&row.source, &doc).await?;
                }
                Err(e) => {
                    tracing::warn!(source = %row.source.id, error = %e, "registry refresh failed");
                    row.source.last_error = Some(e.to_string());
                }
            }
            self.store.put_source(row).await?;
        }
        Ok(())
    }

    async fn fetch_index(&self, row: &SourceRow) -> R<(Vec<u8>, Vec<u8>, String, IndexDocument)> {
        let t = self.transport(&row.source)?;
        let json = t.fetch("index.json").await?;
        let sig = t.fetch("index.sig").await?;
        let v = index::verify_index(&json, &sig, self.verifier.as_ref(), row.pinned_key.as_deref())?;
        Ok((json, sig, v.signer.public_key, v.doc))
    }

    /// Records publisher keys, key revocations and revoked installs of an index.
    async fn ingest(&self, source: &RegistrySource, doc: &IndexDocument) -> R<()> {
        for k in &doc.keys {
            let revoked = doc.revoked_keys.contains(&k.key_id);
            match self.store.key(&k.key_id).await? {
                Some(mut known) => {
                    if revoked && known.trust != KeyTrust::Revoked {
                        known.trust = KeyTrust::Revoked;
                        self.store.put_key(&known).await?;
                    }
                }
                None => {
                    let official =
                        source.kind == RegistrySourceKind::Official && k.public_key.trim() == self.official_key.trim();
                    let trust = if revoked {
                        KeyTrust::Revoked
                    } else if official {
                        KeyTrust::Official
                    } else {
                        KeyTrust::Untrusted
                    };
                    self.store.put_key(&public_key_view(k, trust)?).await?;
                }
            }
        }
        for mut inst in self.store.installed().await? {
            if inst.source_id != source.id || inst.revoked {
                continue;
            }
            let hit = doc.revoked_keys.contains(&inst.key_id)
                || doc.entries.iter().any(|e| e.id == inst.id && e.version == inst.version && e.revoked);
            if hit {
                inst.revoked = true;
                self.store.put_installed(&inst).await?;
            }
        }
        Ok(())
    }

    /// Verified indexes of every enabled source (lazy first refresh).
    async fn indexes(&self, source_id: Option<&str>) -> R<Vec<(RegistrySource, IndexDocument)>> {
        self.ensure_init().await?;
        let mut out = Vec::new();
        for row in self.store.sources().await? {
            if !row.source.enabled || source_id.is_some_and(|id| id != row.source.id) {
                continue;
            }
            if let Some(doc) = self.cached_index(&row) {
                out.push((row.source, doc));
            }
        }
        Ok(out)
    }

    // ---- search --------------------------------------------------------

    fn to_wire(
        e: &IndexEntry,
        doc: &IndexDocument,
        source_id: &str,
        installed: Option<&InstalledPackage>,
    ) -> RegistryEntry {
        RegistryEntry {
            id: e.id.clone(),
            kind: e.kind,
            name: e.name.clone(),
            version: e.version.clone(),
            publisher: e.publisher.clone(),
            description: e.description.clone(),
            key_id: e.key_id.clone(),
            sha256: e.sha256.clone(),
            url: e.url.clone(),
            revoked: doc.is_revoked(e),
            tags: e.tags.clone(),
            source_id: source_id.to_owned(),
            installed_version: installed.map(|i| i.version.clone()),
        }
    }

    /// Latest (non-revoked unless asked) version per package id.
    fn latest<'a>(doc: &'a IndexDocument, id: &str, include_revoked: bool) -> Option<&'a IndexEntry> {
        doc.entries
            .iter()
            .filter(|e| e.id == id && (include_revoked || !doc.is_revoked(e)))
            .max_by(|a, b| publish::version_cmp(&a.version, &b.version))
    }

    /// Searches the verified indexes.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn search(&self, p: MarketplaceSearchParams) -> R<Vec<RegistryEntry>> {
        let installed: BTreeMap<String, InstalledPackage> =
            self.store.installed().await?.into_iter().map(|i| (i.id.clone(), i)).collect();
        let q = p.query.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
        let tags: Vec<String> = p.tags.iter().map(|t| t.to_lowercase()).collect();
        let limit = p.limit.unwrap_or(100).clamp(1, 1000) as usize;
        let mut out = Vec::new();
        for (source, doc) in self.indexes(p.source_id.as_deref()).await? {
            let mut ids: Vec<&str> = doc.entries.iter().map(|e| e.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let Some(e) = Self::latest(&doc, id, p.include_revoked) else { continue };
                if p.kind.is_some_and(|k| k != e.kind) {
                    continue;
                }
                if !tags.iter().all(|t| e.tags.iter().any(|et| et.to_lowercase() == *t)) {
                    continue;
                }
                if let Some(q) = &q {
                    let hay = format!("{} {} {} {} {}", e.id, e.name, e.description, e.publisher, e.tags.join(" "))
                        .to_lowercase();
                    if !q.split_whitespace().all(|w| hay.contains(w)) {
                        continue;
                    }
                }
                out.push(Self::to_wire(e, &doc, &source.id, installed.get(&e.id)));
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.source_id.cmp(&b.source_id)));
        out.truncate(limit);
        Ok(out)
    }

    async fn find(
        &self,
        id: &str,
        version: Option<&str>,
        source_id: Option<&str>,
    ) -> R<(RegistrySource, IndexDocument, IndexEntry)> {
        for (source, doc) in self.indexes(source_id).await? {
            let entry = match version {
                Some(v) => doc.entries.iter().find(|e| e.id == id && e.version == v).cloned(),
                None => Self::latest(&doc, id, false).or_else(|| Self::latest(&doc, id, true)).cloned(),
            };
            if let Some(entry) = entry {
                return Ok((source, doc, entry));
            }
        }
        Err(MarketplaceError::NotFound(format!("package {id}{}", version.map(|v| format!("@{v}")).unwrap_or_default())))
    }

    async fn key_view(&self, doc: &IndexDocument, key_id: &str) -> R<PublisherKey> {
        if let Some(k) = self.store.key(key_id).await? {
            return Ok(k);
        }
        let k = doc.key(key_id).ok_or_else(|| MarketplaceError::Signature(format!("unknown key {key_id}")))?;
        public_key_view(k, KeyTrust::Untrusted)
    }

    /// Package details.
    ///
    /// # Errors
    /// Unknown package.
    pub async fn get(&self, id: &str, source_id: Option<&str>) -> R<EntryDetailsResult> {
        let (source, doc, e) = self.find(id, None, source_id).await?;
        let installed = self.store.installed().await?.into_iter().find(|i| i.id == e.id);
        let mut versions: Vec<String> =
            doc.entries.iter().filter(|x| x.id == e.id).map(|x| x.version.clone()).collect();
        versions.sort_by(|a, b| publish::version_cmp(b, a));
        let mut key = self.key_view(&doc, &e.key_id).await?;
        if doc.revoked_keys.contains(&e.key_id) {
            key.trust = KeyTrust::Revoked;
        }
        Ok(EntryDetailsResult {
            entry: Self::to_wire(&e, &doc, &source.id, installed.as_ref()),
            versions,
            permissions: e.permissions.clone(),
            key,
        })
    }

    // ---- install -------------------------------------------------------

    /// Installs (or updates) a package after verifying index signature,
    /// revocation, publisher key trust, key pinning, sha256 and the publisher
    /// signature.
    ///
    /// # Errors
    /// [`MarketplaceError::Denied`] for revoked / untrusted / key change,
    /// [`MarketplaceError::Signature`] for digest or signature mismatches.
    pub async fn install(&self, p: InstallEntryParams, to: Installers<'_>) -> R<InstallEntryResult> {
        let (source, doc, e) = self.find(&p.id, p.version.as_deref(), p.source_id.as_deref()).await?;
        let _g = self.lock.lock().await;
        if doc.is_revoked(&e) {
            return Err(denied(format!("{}@{} is revoked by registry {}", e.id, e.version, source.id)));
        }
        let key = self.key_view(&doc, &e.key_id).await?;
        match key.trust {
            KeyTrust::Revoked => return Err(denied(format!("publisher key {} is revoked", key.fingerprint))),
            KeyTrust::Untrusted => {
                return Err(denied(format!(
                    "publisher key {} ({}) is not trusted; confirm its fingerprint with marketplace.trustKey",
                    key.fingerprint, key.publisher
                )))
            }
            KeyTrust::Trusted | KeyTrust::Official => {}
        }
        if let Some(pinned) = self.store.pin(&e.id).await? {
            if pinned != e.key_id {
                return Err(denied(format!(
                    "{} is pinned to publisher key {pinned}; this version is signed by {} (key change refused)",
                    e.id, e.key_id
                )));
            }
        }
        let previous = self.store.installed().await?.into_iter().find(|i| i.id == e.id);
        if let Some(prev) = &previous {
            if prev.version == e.version && prev.sha256 == e.sha256 && !prev.revoked {
                return Err(MarketplaceError::Conflict(format!("{}@{} is already installed", e.id, e.version)));
            }
        }
        let bytes = self.transport(&source)?.fetch(&e.url).await?;
        index::verify_package(&doc, &e, &bytes, self.verifier.as_ref())?;

        let mut result = InstallEntryResult {
            kind: e.kind,
            version: e.version.clone(),
            plugin: None,
            pending_grants: Vec::new(),
            template_ids: Vec::new(),
        };
        let mut record = InstalledPackage {
            id: e.id.clone(),
            kind: e.kind,
            name: e.name.clone(),
            version: e.version.clone(),
            source_id: source.id.clone(),
            key_id: e.key_id.clone(),
            sha256: e.sha256.clone(),
            template_ids: Vec::new(),
            plugin_id: None,
            revoked: false,
            update_available: None,
            installed_at: now(),
        };
        match e.kind {
            PackageKind::AgentTemplate => {
                let json = std::str::from_utf8(&bytes)
                    .map_err(|_| MarketplaceError::Invalid("agent template is not UTF-8".into()))?;
                let accept = publish::template_permissions_only_deny(&bytes);
                let tid = to.templates.import(json, accept).await?;
                if let Some(prev) = &previous {
                    for old in &prev.template_ids {
                        if let Err(err) = to.templates.remove(old).await {
                            tracing::warn!(template = %old, error = %err, "old template version not removed");
                        }
                    }
                }
                result.template_ids = vec![tid.clone()];
                record.template_ids = vec![tid];
            }
            PackageKind::Plugin | PackageKind::Widget => {
                if let Some(old) = previous.as_ref().and_then(|p| p.plugin_id.as_deref()) {
                    to.plugins.uninstall(old).await?;
                }
                let r = to.plugins.install(&bytes, format!("marketplace:{}/{}", source.id, e.id)).await?;
                record.plugin_id = Some(r.plugin.plugin.id.to_string());
                result.pending_grants = r.pending_grants;
                result.plugin = Some(r.plugin);
            }
        }
        self.store.set_pin(&e.id, &e.key_id).await?;
        self.store.put_installed(&record).await?;
        drop(_g);
        self.audit(
            "marketplace.install",
            &e.id,
            serde_json::json!({ "version": e.version, "source": source.id, "keyId": e.key_id, "sha256": e.sha256 }),
        )
        .await;
        self.emit(Some(&source.id), "installed").await;
        Ok(result)
    }

    /// Installed packages with update availability (same key only) and
    /// revocation state.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn installed(&self) -> R<Vec<InstalledPackage>> {
        let indexes = self.indexes(None).await?;
        let mut out = self.store.installed().await?;
        for inst in &mut out {
            if let Some((_, doc)) = indexes.iter().find(|(s, _)| s.id == inst.source_id) {
                if doc.revoked_keys.contains(&inst.key_id)
                    || doc.entries.iter().any(|e| e.id == inst.id && e.version == inst.version && e.revoked)
                {
                    inst.revoked = true;
                }
                inst.update_available = Self::latest(doc, &inst.id, false)
                    .filter(|l| publish::version_cmp(&l.version, &inst.version).is_gt() && l.key_id == inst.key_id)
                    .map(|l| l.version.clone());
            }
        }
        Ok(out)
    }

    /// Uninstalls a package installed from a registry (the key pin stays).
    ///
    /// # Errors
    /// Not installed / sink failure.
    pub async fn uninstall(&self, id: &str, to: Installers<'_>) -> R<()> {
        let _g = self.lock.lock().await;
        let inst = self
            .store
            .installed()
            .await?
            .into_iter()
            .find(|i| i.id == id)
            .ok_or_else(|| MarketplaceError::NotFound(format!("installed package {id}")))?;
        for t in &inst.template_ids {
            match to.templates.remove(t).await {
                Ok(()) | Err(MarketplaceError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(pid) = &inst.plugin_id {
            match to.plugins.uninstall(pid).await {
                Ok(()) | Err(MarketplaceError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.store.delete_installed(id).await?;
        drop(_g);
        self.audit("marketplace.uninstall", id, serde_json::json!({ "version": inst.version })).await;
        self.emit(Some(&inst.source_id), "uninstalled").await;
        Ok(())
    }

    // ---- keys ----------------------------------------------------------

    /// Known publisher keys and the local publisher key.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn keys(&self) -> R<(Vec<PublisherKey>, Option<PublisherKey>)> {
        self.ensure_init().await?;
        let local_id = self.store.meta("publisher_key_id").await?;
        let keys = self.store.keys().await?;
        let local = local_id.and_then(|id| keys.iter().find(|k| k.key_id == id).cloned());
        Ok((keys, local))
    }

    /// Trusts (pins) a key after the user confirmed its fingerprint.
    ///
    /// # Errors
    /// Unknown key, fingerprint mismatch, revoked key.
    pub async fn trust_key(&self, key_id: &str, fingerprint: &str) -> R<PublisherKey> {
        let mut k = self.store.key(key_id).await?.ok_or_else(|| MarketplaceError::NotFound(format!("key {key_id}")))?;
        let norm = |s: &str| s.chars().filter(char::is_ascii_hexdigit).collect::<String>().to_lowercase();
        if norm(fingerprint) != norm(&k.fingerprint) {
            return Err(denied("fingerprint does not match the key"));
        }
        match k.trust {
            KeyTrust::Revoked => return Err(denied("the key is revoked")),
            KeyTrust::Official | KeyTrust::Trusted => return Ok(k),
            KeyTrust::Untrusted => {}
        }
        k.trust = KeyTrust::Trusted;
        self.store.put_key(&k).await?;
        self.audit(
            "marketplace.trustKey",
            key_id,
            serde_json::json!({ "fingerprint": k.fingerprint, "publisher": k.publisher }),
        )
        .await;
        self.emit(None, "keyTrusted").await;
        Ok(k)
    }

    /// Removes trust from a key (installs and updates signed by it are refused).
    ///
    /// # Errors
    /// Unknown key, official key.
    pub async fn untrust_key(&self, key_id: &str) -> R<PublisherKey> {
        let mut k = self.store.key(key_id).await?.ok_or_else(|| MarketplaceError::NotFound(format!("key {key_id}")))?;
        match k.trust {
            KeyTrust::Official => return Err(MarketplaceError::Invalid("the official key cannot be untrusted".into())),
            KeyTrust::Trusted => {
                k.trust = KeyTrust::Untrusted;
                self.store.put_key(&k).await?;
                self.audit("marketplace.untrustKey", key_id, serde_json::json!({})).await;
                self.emit(None, "keyUntrusted").await;
            }
            KeyTrust::Untrusted | KeyTrust::Revoked => {}
        }
        Ok(k)
    }

    // ---- publish -------------------------------------------------------

    /// The vault-held publisher key, created on first use.
    async fn publisher_key(&self, publisher: Option<&str>) -> R<(KeyPair, String)> {
        let sec = &self.deps.security;
        let ctx = AccessContext { project_id: None };
        let actor: Actor = sec.user();
        let key = match sec.secrets.resolve_by_name(PUBLISHER_KEY_SECRET, actor, ctx).await {
            Ok(seed) => KeyPair::from_seed(&seed)?,
            Err(SecurityError::SecretNotFound(_)) => {
                let k = KeyPair::generate();
                sec.secrets
                    .set(PUBLISHER_KEY_SECRET, SecretScope::Global, zeroize::Zeroizing::new(k.seed_b64()), sec.user())
                    .await
                    .map_err(sec_err)?;
                self.audit("marketplace.publisherKeyCreated", &k.key_id(), serde_json::json!({})).await;
                k
            }
            Err(e) => return Err(sec_err(e)),
        };
        let stored = self.store.meta("publisher_name").await?;
        let name = publisher
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .or(stored.clone())
            .unwrap_or_else(|| "local".to_owned());
        if stored.as_deref() != Some(&name) {
            self.store.set_meta("publisher_name", &name).await?;
        }
        self.store.set_meta("publisher_key_id", &key.key_id()).await?;
        let mut view = public_key_view(
            &IndexKey { key_id: key.key_id(), publisher: name.clone(), public_key: key.public_key() },
            KeyTrust::Trusted,
        )?;
        if let Some(existing) = self.store.key(&view.key_id).await? {
            view.first_seen = existing.first_seen;
        }
        self.store.put_key(&view).await?;
        Ok((key, name))
    }

    /// Signs a local package with the publisher key and adds it to a
    /// directory source or uploads it to a gateway registry.
    ///
    /// # Errors
    /// Invalid package, unknown / read-only source, conflicts, transport failures.
    pub async fn publish(&self, p: PublishPackageParams) -> R<RegistryEntry> {
        self.ensure_init().await?;
        let row = self.source_row(&p.source_id).await?;
        let bytes =
            tokio::fs::read(&p.path).await.map_err(|e| MarketplaceError::Invalid(format!("{}: {e}", p.path)))?;
        if bytes.len() > sources::MAX_PACKAGE_BYTES {
            return Err(MarketplaceError::Invalid("package is too large".into()));
        }
        let (key, publisher) = self.publisher_key(p.publisher.as_deref()).await?;
        let meta = publish::describe(p.kind, &bytes, &publisher)?;
        let sha256 = signing::sha256_hex(&bytes);
        let signature = key.sign(&signing::package_message(&sha256)?);
        let _g = self.lock.lock().await;
        let entry = match row.source.kind {
            RegistrySourceKind::Official => {
                return Err(MarketplaceError::Invalid("the official source is read-only".into()))
            }
            RegistrySourceKind::Directory => {
                publish_to_directory(Path::new(&row.source.location), &key, meta, &bytes, &sha256, &signature).await?
            }
            RegistrySourceKind::Http => self.publish_to_http(&row.source, &key, meta, &bytes, signature).await?,
        };
        self.refresh_locked(Some(&row.source.id)).await?;
        drop(_g);
        self.audit(
            "marketplace.publish",
            &entry.id,
            serde_json::json!({ "version": entry.version, "source": row.source.id, "keyId": entry.key_id }),
        )
        .await;
        self.emit(Some(&row.source.id), "published").await;
        let doc = IndexDocument::empty("");
        Ok(Self::to_wire(&entry, &doc, &row.source.id, None))
    }

    async fn publish_to_http(
        &self,
        source: &RegistrySource,
        key: &KeyPair,
        meta: PackageMeta,
        bytes: &[u8],
        signature: String,
    ) -> R<IndexEntry> {
        let t = HttpTransport::new(&source.location)?;
        let token = self
            .deps
            .security
            .secrets
            .resolve_by_name(
                &source_token_secret(&source.id),
                self.deps.security.user(),
                AccessContext { project_id: None },
            )
            .await
            .map_err(|e| match e {
                SecurityError::SecretNotFound(_) => denied(format!(
                    "no publish token for {}: add the source again with a gateway session or core token",
                    source.location
                )),
                other => sec_err(other),
            })?;
        let body =
            PublishRequest { entry: meta, public_key: key.public_key(), signature, package_base64: B64.encode(bytes) };
        let url = format!("{}/publish", t.base());
        let resp = t
            .client()
            .post(&url)
            .bearer_auth(token.as_str())
            .json(&body)
            .send()
            .await
            .map_err(|e| MarketplaceError::Source(format!("{url}: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or(text);
            return Err(match status.as_u16() {
                401 | 403 => denied(format!("registry refused the publish: {msg}")),
                409 => MarketplaceError::Conflict(msg),
                400 | 422 => MarketplaceError::Invalid(msg),
                _ => MarketplaceError::Source(format!("{url}: HTTP {status}: {msg}")),
            });
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            entry: IndexEntry,
        }
        let r: Resp = serde_json::from_str(&text).map_err(|e| MarketplaceError::Source(format!("{url}: {e}")))?;
        Ok(r.entry)
    }
}

/// Wire view of an index key.
fn public_key_view(k: &IndexKey, trust: KeyTrust) -> R<PublisherKey> {
    Ok(PublisherKey {
        key_id: k.key_id.clone(),
        publisher: k.publisher.clone(),
        public_key: k.public_key.clone(),
        fingerprint: signing::fingerprint(&k.public_key)?,
        trust,
        first_seen: now(),
    })
}

/// Adds a signed package to a directory registry signed by `key` (created
/// when empty). Package files are named by digest.
///
/// # Errors
/// Directory signed by another key, duplicate version, I/O.
pub async fn publish_to_directory(
    root: &Path,
    key: &KeyPair,
    meta: PackageMeta,
    bytes: &[u8],
    sha256: &str,
    signature: &str,
) -> R<IndexEntry> {
    tokio::fs::create_dir_all(root.join("packages")).await?;
    let index_path = root.join("index.json");
    let mut doc = if tokio::fs::try_exists(&index_path).await? {
        let json = tokio::fs::read(&index_path).await?;
        let sig = tokio::fs::read(root.join("index.sig")).await?;
        index::verify_index(&json, &sig, &Ed25519Verifier, Some(&key.public_key()))
            .map_err(|e| {
                MarketplaceError::Conflict(format!("directory registry is not signed by this publisher key: {e}"))
            })?
            .doc
    } else {
        IndexDocument::empty(root.file_name().and_then(|n| n.to_str()).unwrap_or("registry"))
    };
    if doc.entries.iter().any(|e| e.id == meta.id && e.version == meta.version) {
        return Err(MarketplaceError::Conflict(format!("{}@{} is already published", meta.id, meta.version)));
    }
    if doc.key(&key.key_id()).is_none() {
        doc.keys.push(IndexKey {
            key_id: key.key_id(),
            publisher: meta.publisher.clone(),
            public_key: key.public_key(),
        });
    }
    let ext = if meta.kind == PackageKind::AgentTemplate { "json" } else { "orkp" };
    let url = format!("packages/{sha256}.{ext}");
    tokio::fs::write(root.join(&url), bytes).await?;
    let entry = IndexEntry {
        id: meta.id,
        kind: meta.kind,
        name: meta.name,
        version: meta.version,
        publisher: meta.publisher,
        description: meta.description,
        tags: meta.tags,
        permissions: meta.permissions,
        key_id: key.key_id(),
        sha256: sha256.to_owned(),
        signature: signature.to_owned(),
        url,
        revoked: false,
    };
    doc.entries.push(entry.clone());
    write_index(root, &doc, key).await?;
    Ok(entry)
}

/// Signs and atomically writes `index.json` + `index.sig`.
///
/// # Errors
/// I/O.
pub async fn write_index(root: &Path, doc: &IndexDocument, key: &KeyPair) -> R<()> {
    let (json, sig) = index::sign_index(doc, key)?;
    for (name, data) in [("index.json", json), ("index.sig", sig)] {
        let tmp = root.join(format!("{name}.tmp"));
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, root.join(name)).await?;
    }
    Ok(())
}

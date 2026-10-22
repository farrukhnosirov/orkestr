//! Marketplace end-to-end over real registries: official catalogue,
//! directory source, gateway-hosted HTTP registry. Templates and plugins go
//! to recording sinks.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_marketplace::index::{IndexDocument, IndexEntry, IndexKey};
use orkestr_marketplace::publish::{describe, PackageMeta};
use orkestr_marketplace::service::{publish_to_directory, write_index};
use orkestr_marketplace::signing::{package_message, sha256_hex, KeyPair};
use orkestr_marketplace::{Installers, Marketplace, MarketplaceDeps, MarketplaceError, PluginSink, TemplateSink};
use orkestr_protocol::methods::marketplace::{
    AddSourceParams, InstallEntryParams, KeyTrust, MarketplaceSearchParams, PackageKind, PublishPackageParams,
    RegistrySourceKind,
};
use orkestr_protocol::methods::plugins::InstallPluginResult;
use orkestr_storage::Db;
use parking_lot::Mutex;

#[derive(Default)]
struct Templates {
    imported: Mutex<Vec<(String, String)>>,
    removed: Mutex<Vec<String>>,
}

#[async_trait]
impl TemplateSink for Templates {
    async fn import(&self, json: &str, _accept: bool) -> Result<String, MarketplaceError> {
        let mut g = self.imported.lock();
        let id = format!("tpl-{}", g.len() + 1);
        g.push((id.clone(), json.to_owned()));
        Ok(id)
    }
    async fn remove(&self, id: &str) -> Result<(), MarketplaceError> {
        self.removed.lock().push(id.to_owned());
        Ok(())
    }
}

struct NoPlugins;

#[async_trait]
impl PluginSink for NoPlugins {
    async fn install(&self, _b: &[u8], _s: String) -> Result<InstallPluginResult, MarketplaceError> {
        Err(MarketplaceError::Unimplemented("plugins.install"))
    }
    async fn uninstall(&self, _id: &str) -> Result<(), MarketplaceError> {
        Ok(())
    }
}

struct H {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    mp: Marketplace,
    templates: Templates,
}

impl H {
    fn to(&self) -> Installers<'_> {
        Installers { templates: &self.templates, plugins: &NoPlugins }
    }

    async fn install(&self, id: &str, version: Option<&str>) -> Result<(), MarketplaceError> {
        self.mp
            .install(
                InstallEntryParams { id: id.into(), version: version.map(str::to_owned), source_id: None },
                self.to(),
            )
            .await
            .map(|_| ())
    }

    async fn trust_all(&self) {
        let (keys, _) = self.mp.keys().await.unwrap();
        for k in keys.into_iter().filter(|k| k.trust == KeyTrust::Untrusted) {
            self.mp.trust_key(&k.key_id, &k.fingerprint).await.unwrap();
        }
    }
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("o.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_marketplace::marketplace_migrations());
    db.migrate(migs).await.unwrap();
    let events = orkestr_events::EventBus::in_memory(1024);
    let security = Arc::new(
        orkestr_security::Security::open(orkestr_security::SecurityConfig {
            db: db.clone(),
            events: events.clone(),
            data_dir: dir.path().join("data"),
            credentials: Some(Arc::new(orkestr_platform::MemoryCredentialStore::default())),
            secret_backend: orkestr_security::SecretBackendChoice::Memory,
            guard: orkestr_security::GuardConfig::default(),
        })
        .await
        .unwrap(),
    );
    let plugins = Arc::new(orkestr_plugins::PluginService::new(orkestr_plugins::PluginDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: dir.path().join("plugins"),
    }));
    let mp =
        Marketplace::new(MarketplaceDeps { db, events, security, plugins, data_dir: dir.path().join("marketplace") });
    let root = dir.path().to_path_buf();
    H { _dir: dir, root, mp, templates: Templates::default() }
}

fn template(name: &str, version: &str) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "format": "orkestr.agent-template",
        "schemaVersion": 1,
        "template": { "name": name, "role": "Reviewer", "description": format!("{name} v{version}"), "version": version, "permissions": [] }
    }))
    .unwrap()
}

/// Signs `bytes` as `publisher` and adds it to a registry signed by `registry`.
fn entry(publisher: &KeyPair, publisher_name: &str, bytes: &[u8], meta: PackageMeta) -> IndexEntry {
    let sha256 = sha256_hex(bytes);
    IndexEntry {
        id: meta.id,
        kind: meta.kind,
        name: meta.name,
        version: meta.version,
        publisher: publisher_name.into(),
        description: meta.description,
        tags: meta.tags,
        permissions: meta.permissions,
        key_id: publisher.key_id(),
        signature: publisher.sign(&package_message(&sha256).unwrap()),
        sha256,
        url: String::new(),
        revoked: false,
    }
}

async fn write_registry(root: &Path, registry: &KeyPair, items: &[(&KeyPair, &str, Vec<u8>)], revoked: &[&str]) {
    std::fs::create_dir_all(root.join("packages")).unwrap();
    let mut doc = IndexDocument::empty("test");
    for (k, publisher, bytes) in items {
        if doc.key(&k.key_id()).is_none() {
            doc.keys.push(IndexKey { key_id: k.key_id(), publisher: (*publisher).into(), public_key: k.public_key() });
        }
        let meta = describe(PackageKind::AgentTemplate, bytes, "acme").unwrap();
        let mut e = entry(k, publisher, bytes, meta);
        e.url = format!("packages/{}.json", e.sha256);
        e.revoked = revoked.contains(&e.version.as_str());
        std::fs::write(root.join(&e.url), bytes).unwrap();
        doc.entries.push(e);
    }
    write_index(root, &doc, registry).await.unwrap();
}

async fn add_dir(h: &H, dir: &Path) -> String {
    h.mp.add_source(AddSourceParams {
        kind: RegistrySourceKind::Directory,
        location: dir.to_string_lossy().into_owned(),
        name: Some("dir".into()),
        token: None,
    })
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn official_catalogue_installs_templates() {
    let h = harness().await;
    let found =
        h.mp.search(MarketplaceSearchParams { kind: Some(PackageKind::AgentTemplate), ..Default::default() })
            .await
            .unwrap();
    assert!(found.len() >= 2, "{found:?}");
    assert!(found.iter().all(|e| e.source_id == "official"));
    let hits =
        h.mp.search(MarketplaceSearchParams {
            query: Some("docs".into()),
            tags: vec!["researcher".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    // Official key: no trust prompt.
    h.install("orkestr.strict-code-reviewer", None).await.unwrap();
    assert_eq!(h.templates.imported.lock().len(), 1);
    let installed = h.mp.installed().await.unwrap();
    assert_eq!(installed[0].template_ids, vec!["tpl-1".to_owned()]);
    assert!(matches!(h.install("orkestr.strict-code-reviewer", None).await, Err(MarketplaceError::Conflict(_))));
    let details = h.mp.get("orkestr.docs-writer", None).await.unwrap();
    assert_eq!(details.key.trust, KeyTrust::Official);
    h.mp.uninstall("orkestr.strict-code-reviewer", h.to()).await.unwrap();
    assert_eq!(*h.templates.removed.lock(), vec!["tpl-1".to_owned()]);
}

#[tokio::test]
async fn tampered_index_is_rejected_and_cache_kept() {
    let h = harness().await;
    let reg = KeyPair::generate();
    let dir = h.root.join("reg");
    write_registry(&dir, &reg, &[(&reg, "acme", template("Linter", "1.0.0"))], &[]).await;
    let sid = add_dir(&h, &dir).await;
    assert_eq!(
        h.mp.search(MarketplaceSearchParams { source_id: Some(sid.clone()), ..Default::default() })
            .await
            .unwrap()
            .len(),
        1
    );

    let text = std::fs::read_to_string(dir.join("index.json")).unwrap().replace("Linter v1.0.0", "Totally safe");
    std::fs::write(dir.join("index.json"), text).unwrap();
    let sources = h.mp.refresh(Some(&sid)).await.unwrap();
    let s = sources.iter().find(|s| s.id == sid).unwrap();
    assert!(s.last_error.as_deref().unwrap().contains("signature"), "{s:?}");
    let found =
        h.mp.search(MarketplaceSearchParams { source_id: Some(sid.clone()), ..Default::default() }).await.unwrap();
    assert_eq!(found[0].description, "Linter v1.0.0", "the tampered index must not be used");

    // A different registry key is refused too (pinned on first use).
    write_registry(&dir, &KeyPair::generate(), &[(&reg, "acme", template("Linter", "1.0.0"))], &[]).await;
    let sources = h.mp.refresh(Some(&sid)).await.unwrap();
    assert!(sources
        .iter()
        .find(|s| s.id == sid)
        .unwrap()
        .last_error
        .as_deref()
        .unwrap()
        .contains("registry key changed"));
}

#[tokio::test]
async fn untrusted_key_needs_trust_and_revocation_is_honoured() {
    let h = harness().await;
    let reg = KeyPair::generate();
    let dir = h.root.join("reg");
    write_registry(&dir, &reg, &[(&reg, "acme", template("Linter", "1.0.0"))], &[]).await;
    add_dir(&h, &dir).await;
    let err = h.install("acme.linter", None).await.unwrap_err();
    assert!(matches!(&err, MarketplaceError::Denied(m) if m.contains("not trusted")), "{err}");
    let (keys, _) = h.mp.keys().await.unwrap();
    let k = keys.iter().find(|k| k.key_id == reg.key_id()).unwrap();
    assert_eq!(k.trust, KeyTrust::Untrusted);
    assert!(h.mp.trust_key(&k.key_id, "0000").await.is_err(), "fingerprint must match");
    h.mp.trust_key(&k.key_id, &k.fingerprint).await.unwrap();
    h.install("acme.linter", None).await.unwrap();

    // The registry revokes 1.0.0: the install is marked, reinstalling is refused.
    write_registry(&dir, &reg, &[(&reg, "acme", template("Linter", "1.0.0"))], &["1.0.0"]).await;
    h.mp.refresh(None).await.unwrap();
    assert!(h.mp.installed().await.unwrap()[0].revoked);
    h.mp.uninstall("acme.linter", h.to()).await.unwrap();
    let err = h.install("acme.linter", Some("1.0.0")).await.unwrap_err();
    assert!(matches!(&err, MarketplaceError::Denied(m) if m.contains("revoked")), "{err}");
}

#[tokio::test]
async fn key_change_on_update_is_refused() {
    let h = harness().await;
    let reg = KeyPair::generate();
    let (a, b) = (KeyPair::generate(), KeyPair::generate());
    let dir = h.root.join("reg");
    write_registry(&dir, &reg, &[(&a, "acme", template("Linter", "1.0.0"))], &[]).await;
    add_dir(&h, &dir).await;
    h.trust_all().await;
    h.install("acme.linter", None).await.unwrap();
    // 1.1.0 by the same key is an update; 2.0.0 by another (even trusted) key is not.
    write_registry(
        &dir,
        &reg,
        &[
            (&a, "acme", template("Linter", "1.0.0")),
            (&a, "acme", template("Linter", "1.1.0")),
            (&b, "mallory", template("Linter", "2.0.0")),
        ],
        &[],
    )
    .await;
    h.mp.refresh(None).await.unwrap();
    h.trust_all().await;
    let err = h.install("acme.linter", Some("2.0.0")).await.unwrap_err();
    assert!(matches!(&err, MarketplaceError::Denied(m) if m.contains("key change")), "{err}");
    assert_eq!(h.mp.installed().await.unwrap()[0].update_available, None, "latest is by another key");
    h.install("acme.linter", Some("1.1.0")).await.unwrap();
    let inst = h.mp.installed().await.unwrap();
    assert_eq!(inst[0].version, "1.1.0");
    assert_eq!(*h.templates.removed.lock(), vec!["tpl-1".to_owned()], "old template replaced");
}

#[tokio::test]
async fn tampered_package_is_rejected() {
    let h = harness().await;
    let reg = KeyPair::generate();
    let dir = h.root.join("reg");
    write_registry(&dir, &reg, &[(&reg, "acme", template("Linter", "1.0.0"))], &[]).await;
    add_dir(&h, &dir).await;
    h.trust_all().await;
    let pkg = std::fs::read_dir(dir.join("packages")).unwrap().next().unwrap().unwrap().path();
    std::fs::write(&pkg, template("Linter", "6.6.6")).unwrap();
    let err = h.install("acme.linter", None).await.unwrap_err();
    assert!(matches!(&err, MarketplaceError::Signature(m) if m.contains("sha256")), "{err}");
}

#[tokio::test]
async fn directory_source_publish_install_round_trip() {
    let h = harness().await;
    let dir = h.root.join("my-registry");
    let sid = add_dir(&h, &dir).await; // empty yet: refresh error recorded, not fatal
    let file = h.root.join("t.agent.json");
    std::fs::write(&file, template("Release Checker", "0.3.0")).unwrap();
    let entry =
        h.mp.publish(PublishPackageParams {
            path: file.to_string_lossy().into_owned(),
            kind: PackageKind::AgentTemplate,
            source_id: sid.clone(),
            publisher: Some("Team Blue".into()),
        })
        .await
        .unwrap();
    assert_eq!(entry.id, "team-blue.release-checker");
    let (_, local) = h.mp.keys().await.unwrap();
    let local = local.expect("publisher key created on first publish");
    assert_eq!(local.trust, KeyTrust::Trusted);
    assert_eq!(local.key_id, entry.key_id);
    // Republishing the same version conflicts.
    let again =
        h.mp.publish(PublishPackageParams {
            path: file.to_string_lossy().into_owned(),
            kind: PackageKind::AgentTemplate,
            source_id: sid.clone(),
            publisher: None,
        })
        .await;
    assert!(matches!(again, Err(MarketplaceError::Conflict(_))));
    h.install("team-blue.release-checker", None).await.unwrap();
    assert!(h.templates.imported.lock()[0].1.contains("Release Checker"));
    // A directory signed by someone else cannot be published into.
    let foreign = h.root.join("foreign");
    write_registry(&foreign, &KeyPair::generate(), &[], &[]).await;
    let meta = describe(PackageKind::AgentTemplate, &template("X", "1.0.0"), "p").unwrap();
    let r = publish_to_directory(&foreign, &KeyPair::generate(), meta, b"x", &sha256_hex(b"x"), "sig").await;
    assert!(matches!(r, Err(MarketplaceError::Conflict(_))));
}

#[tokio::test]
async fn gateway_registry_publish_refresh_install_round_trip() {
    let h = harness().await;
    let gw_dir = h.root.join("gateway");
    let cfg = orkestr_gateway::GatewayConfig::from_args(
        ["--data-dir", gw_dir.to_str().unwrap(), "--bind", "127.0.0.1:0"].map(str::to_owned),
    )
    .unwrap();
    let gw = orkestr_gateway::spawn(cfg, orkestr_gateway::relay::Relay::default()).await.unwrap();
    let account = gw.state.store.create_account("publisher").unwrap();
    let token = gw.state.store.create_session(&account);
    let base = format!("http://{}/registry", gw.addr);

    // Without a token the gateway refuses publishes.
    let anon =
        reqwest::Client::new().post(format!("{base}/publish")).json(&serde_json::json!({})).send().await.unwrap();
    assert!(anon.status().is_client_error());

    let source =
        h.mp.add_source(AddSourceParams {
            kind: RegistrySourceKind::Http,
            location: base.clone(),
            name: Some("team".into()),
            token: Some(token.clone()),
        })
        .await
        .unwrap();
    assert!(source.last_error.is_none(), "{source:?}");
    let file = h.root.join("t.agent.json");
    std::fs::write(&file, template("Gateway Helper", "1.0.0")).unwrap();
    let entry =
        h.mp.publish(PublishPackageParams {
            path: file.to_string_lossy().into_owned(),
            kind: PackageKind::AgentTemplate,
            source_id: source.id.clone(),
            publisher: Some("acme".into()),
        })
        .await
        .unwrap();
    assert_eq!(entry.url, format!("packages/{}", entry.sha256));
    let found =
        h.mp.search(MarketplaceSearchParams { query: Some("gateway helper".into()), ..Default::default() })
            .await
            .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].source_id, source.id);
    h.install("acme.gateway-helper", None).await.unwrap();
    assert_eq!(h.templates.imported.lock().len(), 1);

    // Revocation on the gateway reaches the client on refresh.
    let resp = reqwest::Client::new()
        .post(format!("{base}/revoke"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "id": "acme.gateway-helper" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    h.mp.refresh(Some(&source.id)).await.unwrap();
    assert!(h.mp.installed().await.unwrap()[0].revoked);

    // Tampering with the gateway's files is detected by clients.
    let idx = gw_dir.join("registry").join("index.json");
    let text = std::fs::read_to_string(&idx).unwrap().replace("\"revoked\": true", "\"revoked\": false");
    std::fs::write(&idx, text).unwrap();
    let sources = h.mp.refresh(Some(&source.id)).await.unwrap();
    assert!(sources.iter().find(|s| s.id == source.id).unwrap().last_error.is_some());
    gw.task.abort();
}

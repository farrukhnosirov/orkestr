//! Test / acceptance fixture packages, built from the WAT sources in
//! [`crate::wat`] and signed with a **public, deterministic dev key**
//! ([`dev_key`]; never trust it outside tests: start the core with
//! `ORKESTR_PLUGINS_TRUSTED_KEYS=<dev public key>` or trust it explicitly).
//!
//! `cargo run -p orkestr-plugins --example make-fixtures -- <outdir>` writes
//! every package plus `dev-publisher.pub` (base64 public key) and
//! `fixtures.json` (file → package id, tool, expectation).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use orkestr_domain::{
    Capability, DataBinding, PermissionRequest, PluginKind, PluginLimits, PluginManifest, ToolDecl, WidgetDecl,
    WidgetSize,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::package::{self, MANIFEST_FILE};
use crate::signing::PublisherKeyPair;
use crate::{wat, PluginError};

/// The deterministic dev publisher key (seed = sha256 of a public string).
#[must_use]
pub fn dev_key() -> PublisherKeyPair {
    let seed: [u8; 32] = Sha256::digest(b"orkestr-dev-plugin-publisher-key-v1").into();
    PublisherKeyPair::from_seed(&seed)
}

fn manifest(id: &str, name: &str, tool: &str, description: &str) -> PluginManifest {
    PluginManifest {
        id: format!("orkestr-dev.{id}"),
        name: name.into(),
        version: "1.0.0".into(),
        publisher: "Orkestr Dev".into(),
        description: description.into(),
        abi: 1,
        kinds: vec![PluginKind::Tool],
        entry: Some("plugin.wasm".into()),
        permissions: vec![],
        tools: vec![ToolDecl {
            name: tool.into(),
            description: description.into(),
            input_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
        }],
        widgets: vec![],
        deploy_providers: vec![],
        agent_templates: vec![],
        subscribes: vec![],
        limits: PluginLimits::default(),
    }
}

fn wasm_files(src: &str) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
    Ok(BTreeMap::from([("plugin.wasm".to_owned(), wat::to_wasm(src)?)]))
}

/// Widget HTML of `hello-tool`.
pub const HELLO_WIDGET_HTML: &str = r#"<!doctype html><html><body><h3 id="t">Hello widget</h3>
<script>window.addEventListener("message",function(e){var d=e.data||{};if(d.type==="init"){document.getElementById("t").textContent="Hello "+(d.config&&d.config.name||"world");}});parent.postMessage({type:"ready"},"*");</script>
</body></html>"#;

/// Manifest of `hello-tool` (Tool + Widget; asks for one Network grant so the
/// install prompt has something to show; the tool itself only logs + echoes).
#[must_use]
pub fn hello_manifest() -> PluginManifest {
    let mut m = manifest("hello-tool", "Hello Tool", "hello", "Echoes its input and writes a log line.");
    m.kinds = vec![PluginKind::Tool, PluginKind::Widget];
    m.permissions = vec![PermissionRequest {
        capability: Capability::Network,
        scope: Some("example.com".into()),
        reason: "Demo permission shown in the install prompt.".into(),
    }];
    m.widgets = vec![WidgetDecl {
        id: "hello-widget".into(),
        title: "Hello".into(),
        entry_html: "widgets/hello.html".into(),
        sizes: vec![WidgetSize { w: 3, h: 2 }],
        data_binding: DataBinding { methods: vec!["projects.list".into()], events: vec![] },
        config_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
    }];
    m
}

/// Signed `hello-tool` package.
///
/// # Errors
/// Build failure.
pub fn hello_tool(key: &PublisherKeyPair) -> Result<Vec<u8>, PluginError> {
    let mut files = wasm_files(&wat::hello())?;
    files.insert("widgets/hello.html".into(), HELLO_WIDGET_HTML.as_bytes().to_vec());
    package::build(&hello_manifest(), &files, Some(key))
}

/// Manifest of `hello-subscriber` (Tool + PlatformIntegration): subscribes
/// to `TaskCreated` (needs the `Read` grant `event/TaskCreated`), counts the
/// events in its KV store and exposes the tool `count`.
#[must_use]
pub fn subscriber_manifest() -> PluginManifest {
    let mut m = manifest(
        "hello-subscriber",
        "Hello Subscriber",
        "count",
        "Counts TaskCreated events (event subscription demo); the tool returns the count.",
    );
    m.kinds = vec![PluginKind::Tool, PluginKind::PlatformIntegration];
    m.tools[0].input_schema = json!({"type": "object", "properties": {}});
    m.subscribes = vec![wat::SUBSCRIBER_KIND.into()];
    m.permissions = vec![PermissionRequest {
        capability: Capability::Read,
        scope: Some(crate::events::event_scope(wat::SUBSCRIBER_KIND)),
        reason: "Receive TaskCreated events.".into(),
    }];
    m
}

/// Manifest of `hello-deployer` (DeploymentProvider `hello`, target
/// `Plugin("orkestr-dev.hello-deployer/hello")`); asks for the optional
/// `Secrets` grant `HELLO_DEPLOY_TOKEN`.
#[must_use]
pub fn deployer_manifest() -> PluginManifest {
    let mut m = manifest(
        "hello-deployer",
        "Hello Deployer",
        "unused",
        "Demo deployment provider: logs its steps and reports https://hello-deployer.example/.",
    );
    m.kinds = vec![PluginKind::DeploymentProvider];
    m.tools.clear();
    m.deploy_providers = vec!["hello".into()];
    m.permissions = vec![PermissionRequest {
        capability: Capability::Secrets,
        scope: Some(wat::DEPLOYER_SECRET.into()),
        reason: "Deploy token (read with SecretGet; the deployment log redacts it).".into(),
    }];
    m
}

/// One fixture.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// File name.
    pub file: &'static str,
    /// Package id (if the package parses).
    pub package_id: String,
    /// Tool to invoke.
    pub tool: &'static str,
    /// What the acceptance test expects.
    pub expect: &'static str,
    /// Package bytes.
    pub bytes: Vec<u8>,
}

/// Every fixture, signed with `key`.
///
/// # Errors
/// Build failure.
pub fn all(key: &PublisherKeyPair) -> Result<Vec<Fixture>, PluginError> {
    let mut out = Vec::new();
    let hello = hello_tool(key)?;
    out.push(Fixture {
        file: "hello-tool.orkp",
        package_id: "orkestr-dev.hello-tool".into(),
        tool: "hello",
        expect: "installs (pending Network grant), echoes {echo:{tool,input}} and logs `hello called`",
        bytes: hello.clone(),
    });

    let mut m = manifest("evil-loop", "Evil Loop", "spin", "Spins forever.");
    m.limits.timeout_ms = 500;
    m.limits.fuel_per_call = package::MAX_FUEL; // the epoch deadline, not fuel, must stop it
    out.push(Fixture {
        file: "evil-loop.orkp",
        package_id: m.id.clone(),
        tool: "spin",
        expect: "invoke traps with `timed out after 500 ms` in < 1 s",
        bytes: package::build(&m, &wasm_files(&wat::infinite_loop())?, Some(key))?,
    });

    let mut m = manifest("evil-memory", "Evil Memory", "bomb", "Grows memory without bound.");
    m.limits.memory_mb = 16;
    out.push(Fixture {
        file: "evil-memory.orkp",
        package_id: m.id.clone(),
        tool: "bomb",
        expect: "invoke traps with `memory limit of 16 MiB exceeded`",
        bytes: package::build(&m, &wasm_files(&wat::memory_bomb())?, Some(key))?,
    });

    let m = manifest("evil-network", "Evil Network", "steal", "Fetches an undeclared host.");
    out.push(Fixture {
        file: "evil-network.orkp",
        package_id: m.id.clone(),
        tool: "steal",
        expect:
            "invoke returns plugin error `network access to evil.example.com is not granted`; PluginCallDenied + audit",
        bytes: package::build(&m, &wasm_files(&wat::fetch())?, Some(key))?,
    });

    let m = subscriber_manifest();
    out.push(Fixture {
        file: "hello-subscriber.orkp",
        package_id: m.id.clone(),
        tool: "count",
        expect: "with the Read grant event/TaskCreated: each TaskCreated increments its KV counter; tool `count` returns it",
        bytes: package::build(&m, &wasm_files(&wat::subscriber(false))?, Some(key))?,
    });

    let m = deployer_manifest();
    out.push(Fixture {
        file: "hello-deployer.orkp",
        package_id: m.id.clone(),
        tool: "",
        expect: "enabled: deploy.adapters lists plugin:orkestr-dev.hello-deployer/hello; deploys to https://hello-deployer.example/ with 3 logged steps",
        bytes: package::build(&m, &wasm_files(&wat::deployer(false))?, Some(key))?,
    });

    let mut wm = hello_manifest();
    wm.id = "orkestr-dev.hello-widget".into();
    wm.name = "Hello Widget".into();
    wm.description = "Declarative widget bound to projects.list.".into();
    wm.kinds = vec![PluginKind::Widget];
    wm.entry = None;
    wm.tools.clear();
    wm.permissions.clear();
    out.push(Fixture {
        file: "hello-widget.orkp",
        package_id: wm.id.clone(),
        tool: "",
        expect:
            "widget-only plugin (no code, no grants): install + enable, widget `hello-widget` bound to projects.list",
        bytes: package::build(
            &wm,
            &BTreeMap::from([("widgets/hello.html".to_owned(), HELLO_WIDGET_HTML.as_bytes().to_vec())]),
            Some(key),
        )?,
    });

    // Tampered: change the manifest after signing, keep the signature.
    let mut files = package::read_zip(&hello)?;
    let mut tm = hello_manifest();
    tm.description = "Tampered after signing.".into();
    files.insert(
        MANIFEST_FILE.into(),
        serde_json::to_vec_pretty(&tm).map_err(|e| PluginError::Internal(e.to_string()))?,
    );
    out.push(Fixture {
        file: "evil-tampered.orkp",
        package_id: tm.id.clone(),
        tool: "hello",
        expect: "install refused: signature does not match the content",
        bytes: package::write_zip(&files.into_iter().collect::<Vec<_>>())?,
    });

    let m = manifest("evil-unsigned", "Evil Unsigned", "hello", "Unsigned package.");
    out.push(Fixture {
        file: "evil-unsigned.orkp",
        package_id: m.id.clone(),
        tool: "hello",
        expect: "install refused unless ORKESTR_PLUGINS_DEV=1",
        bytes: package::build(&m, &wasm_files(&wat::hello())?, None)?,
    });

    out.push(Fixture {
        file: "evil-traversal.orkp",
        package_id: "orkestr-dev.evil-traversal".into(),
        tool: "hello",
        expect: "install refused: unsafe entry name",
        bytes: package::write_zip(&[
            (MANIFEST_FILE.into(), serde_json::to_vec(&hello_manifest()).unwrap_or_default()),
            ("../../evil.txt".into(), b"pwned".to_vec()),
        ])?,
    });
    Ok(out)
}

/// Writes every fixture + `dev-publisher.pub` + `fixtures.json` into `dir`.
///
/// # Errors
/// Build / I/O failure.
pub fn write_all(dir: &Path) -> Result<Vec<PathBuf>, PluginError> {
    std::fs::create_dir_all(dir)?;
    let key = dev_key();
    let fixtures = all(&key)?;
    let mut written = Vec::new();
    let mut index = Vec::new();
    for f in &fixtures {
        let p = dir.join(f.file);
        std::fs::write(&p, &f.bytes)?;
        index.push(json!({ "file": f.file, "packageId": f.package_id, "tool": f.tool, "expect": f.expect }));
        written.push(p);
    }
    let pub_path = dir.join("dev-publisher.pub");
    std::fs::write(&pub_path, key.public_key_b64())?;
    written.push(pub_path);
    let idx = dir.join("fixtures.json");
    let doc = json!({ "publicKey": key.public_key_b64(), "keyId": key.key_id(), "fingerprint": key.fingerprint(), "fixtures": index });
    std::fs::write(&idx, serde_json::to_vec_pretty(&doc).map_err(|e| PluginError::Internal(e.to_string()))?)?;
    written.push(idx);
    Ok(written)
}

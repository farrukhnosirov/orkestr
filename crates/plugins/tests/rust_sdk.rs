//! Runs a plugin built with the Rust guest SDK (`examples/plugins/hello-tool`)
//! through the real host. Skipped unless `ORKESTR_PLUGIN_SDK_WASM` points at
//! the built `hello_tool.wasm` (CI has no wasm toolchain; WAT fixtures cover
//! the ABI there).

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use orkestr_plugins::{fixtures, package, PluginDeps, PluginService};
use orkestr_protocol::methods::plugins::{InstallPluginParams, InvokePluginParams};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn rust_sdk_plugin_runs_in_the_sandbox() {
    let Ok(path) = std::env::var("ORKESTR_PLUGIN_SDK_WASM") else {
        eprintln!("skipped: ORKESTR_PLUGIN_SDK_WASM not set");
        return;
    };
    let wasm = std::fs::read(path).unwrap();
    let manifest_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/hello-tool/orkestr-plugin.json");
    let manifest = serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let key = fixtures::dev_key();
    let bytes = package::build(&manifest, &BTreeMap::from([("plugin.wasm".to_owned(), wasm)]), Some(&key)).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let db = orkestr_storage::Db::open(dir.path().join("o.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_plugins::plugin_migrations());
    db.migrate(migs).await.unwrap();
    let events = orkestr_events::EventBus::in_memory(256);
    let sec = Arc::new(
        orkestr_security::Security::open(orkestr_security::SecurityConfig {
            db: db.clone(),
            events: events.clone(),
            data_dir: dir.path().join("data"),
            credentials: None,
            secret_backend: orkestr_security::SecretBackendChoice::Memory,
            guard: orkestr_security::GuardConfig::default(),
        })
        .await
        .unwrap(),
    );
    let svc = PluginService::new(PluginDeps { db, events, security: sec, data_dir: dir.path().join("plugins") });
    svc.trust_store().trust(&key.public_key_b64(), "dev", "test").await.unwrap();
    let r = svc
        .install(InstallPluginParams {
            package_base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = r.plugin.plugin.id;
    svc.set_enabled(id, true, None).await.unwrap();
    for _ in 0..2 {
        let out = svc
            .invoke(InvokePluginParams {
                plugin_id: id,
                tool: "hello".into(),
                input: json!({"name": "Ada"}),
                project_id: None,
            })
            .await
            .unwrap();
        assert_eq!(out.output["greeting"], "Hello, Ada!");
    }
    let host = svc.host_for(None);
    use orkestr_plugins::host::HostApi;
    let calls = host.call(&id, orkestr_plugins::HostOp::KvGet, json!({"key": "calls"})).await.unwrap();
    assert_eq!(calls, json!(2));
    assert!(svc.logs(id, 10).await.unwrap().iter().any(|l| l.message == "hello called"));
}

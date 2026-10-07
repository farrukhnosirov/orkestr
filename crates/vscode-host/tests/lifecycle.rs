//! Lifecycle of the supervised server against a fake REH package: a tiny
//! Node.js HTTP server that behaves like `out/server-main.js` (announces its
//! port, checks the connection token from `--connection-token-file`, refuses
//! insecure arguments, can be crashed on demand). Skipped when `node` is not
//! on PATH.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orkestr_platform::{NativeProcessControl, PlatformInfo};
use orkestr_protocol::methods::vscode::{VsCodeConfigPatch, VsCodeState};
use orkestr_vscode_host::lifecycle::Backoff;
use orkestr_vscode_host::target::select_target;
use orkestr_vscode_host::{HostOptions, VsCodeHost};

const FAKE_SERVER: &str = r#"
const http = require('http'); const fs = require('fs');
const a = process.argv.slice(2);
const get = (f) => { const i = a.indexOf(f); return i < 0 ? null : a[i + 1]; };
if (a.includes('--without-connection-token') || get('--host') !== '127.0.0.1' || !get('--connection-token-file')) {
  console.error('insecure arguments'); process.exit(3);
}
const tok = fs.readFileSync(get('--connection-token-file'), 'utf8').trim();
const s = http.createServer((q, r) => {
  const u = new URL(q.url, 'http://x');
  if (u.pathname === '/version') { r.end('fake-commit'); return; }
  if (u.searchParams.get('tkn') !== tok) { r.statusCode = 403; r.end('forbidden'); return; }
  if (u.pathname === '/crash') { r.end('bye'); setTimeout(() => process.exit(7), 50); return; }
  r.setHeader('content-type', 'text/html'); r.end('<!DOCTYPE html><title>fake workbench</title>');
});
s.listen(Number(get('--port')) || 0, '127.0.0.1', () => {
  console.log('Extension host agent listening on ' + s.address().port);
  console.log('Web UI available at http://localhost:' + s.address().port + '?tkn=' + tok);
});
"#;

fn fake_install(root: &Path) -> Option<()> {
    let info = PlatformInfo::detect();
    let target = select_target(info.os, info.arch).ok()?;
    let node = orkestr_platform::process::which("node")?;
    let dir = root.join("1.0.0");
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out").join("server-main.js"), FAKE_SERVER).unwrap();
    std::fs::write(dir.join("product.json"), r#"{"extensionsGallery":{"serviceUrl":"https://open-vsx.org/vscode/gallery"}}"#)
        .unwrap();
    std::fs::copy(node, dir.join(target.node_executable())).unwrap();
    let manifest = serde_json::json!({
        "version": "1.0.0", "assetName": target.asset_name("1.0.0"), "sha256": "00", "sizeBytes": 1,
        "installedAt": "2026-10-07T00:00:00Z", "dir": dir.to_string_lossy(),
    });
    std::fs::write(root.join("installed.json"), manifest.to_string()).unwrap();
    Some(())
}

fn host(root: PathBuf) -> VsCodeHost {
    let info = PlatformInfo::detect();
    let mut opts = HostOptions::new(root, info.os, info.arch, Arc::new(NativeProcessControl));
    opts.backoff = Backoff::new(Duration::from_millis(100), Duration::from_millis(400), 3, Duration::from_secs(60));
    opts.health_interval = Duration::from_millis(500);
    opts.ready_timeout = Duration::from_secs(30);
    VsCodeHost::new(opts).unwrap()
}

async fn wait_for(h: &VsCodeHost, state: VsCodeState) {
    for _ in 0..200 {
        if h.status().state == state {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("state {state:?} not reached: {:?}\nlogs: {:#?}", h.status(), h.logs());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_crash_restart_stop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vscode-server");
    std::fs::create_dir_all(&root).unwrap();
    if fake_install(&root).is_none() {
        eprintln!("skipped: node not found or unsupported host");
        return;
    }
    let project = dir.path().join("my project");
    std::fs::create_dir_all(&project).unwrap();
    let h = host(root.clone());
    assert_eq!(h.status().state, VsCodeState::Stopped, "a valid manifest counts as installed");

    // Start: URL carries the token and the folder; the token gates access.
    let started = h.start(Some(&project)).await.unwrap();
    assert!(started.url.starts_with("http://127.0.0.1:"), "{}", started.url);
    assert!(started.url.contains("?tkn="));
    assert!(started.url.contains("&folder="));
    assert!(started.url.contains("my%20project"));
    let status = h.status();
    assert_eq!(status.state, VsCodeState::Running);
    let port = status.running.as_ref().unwrap().port;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let ok = http.get(&started.url).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    assert!(ok.text().await.unwrap().contains("<!DOCTYPE html>"));
    let denied = http.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
    assert_eq!(denied.status(), 403, "requests without the token are refused");
    let token = started.url.split("tkn=").nth(1).unwrap().split('&').next().unwrap().to_owned();
    assert!(!format!("{:?}", h.status()).contains(&token), "status never carries the token");
    assert!(h.logs().iter().all(|l| !l.contains(&token)), "logs are redacted: {:#?}", h.logs());
    assert!(h.logs().iter().any(|l| l.contains("Web UI available")));
    assert!(root.join("connection-token").exists());

    // A second start reuses the instance (another folder, same server).
    let again = h.start(None).await.unwrap();
    assert_eq!(again.origin, started.origin);

    // Crash → restart with back-off on the remembered port.
    let _ = http.get(format!("http://127.0.0.1:{port}/crash?tkn={token}")).send().await;
    wait_for(&h, VsCodeState::Restarting).await;
    wait_for(&h, VsCodeState::Running).await;
    let running = h.status().running.unwrap();
    assert_eq!(running.restarts, 1);
    assert_eq!(running.port, port, "the port (and so the UI origin) is stable across restarts");

    // Stop kills the process.
    let stopped = h.stop().await.unwrap();
    assert_eq!(stopped.state, VsCodeState::Stopped);
    assert!(stopped.running.is_none());
    assert!(!root.join("connection-token").exists(), "token file removed on stop");
    assert!(http.get(format!("http://127.0.0.1:{port}/version")).send().await.is_err(), "server is gone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_budget_ends_in_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vscode-server");
    std::fs::create_dir_all(&root).unwrap();
    if fake_install(&root).is_none() {
        eprintln!("skipped: node not found or unsupported host");
        return;
    }
    let h = host(root);
    let started = h.start(None).await.unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let token = started.url.split("tkn=").nth(1).unwrap().to_owned();
    // 3 restarts allowed; the 4th crash gives up.
    for _ in 0..4 {
        wait_for(&h, VsCodeState::Running).await;
        let port = h.status().running.unwrap().port;
        let current = std::fs::read_to_string(h.layout().token_file()).unwrap();
        let _ = http.get(format!("http://127.0.0.1:{port}/crash?tkn={}", current.trim())).send().await;
        for _ in 0..100 {
            if h.status().state != VsCodeState::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    wait_for(&h, VsCodeState::Error).await;
    assert!(h.status().error.unwrap().contains("exited"));
    assert!(!token.is_empty());
}

#[tokio::test]
async fn install_requires_consent_and_config_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    let h = host(dir.path().join("vscode-server"));
    assert_eq!(h.status().state, VsCodeState::NotInstalled);
    let e = h.install(false, None).await.unwrap_err();
    assert!(e.to_string().contains("consent"), "{e}");
    let e = h.start(None).await.unwrap_err();
    assert!(e.to_string().contains("not installed"), "{e}");
    assert!(h.set_config(VsCodeConfigPatch { idle_stop_secs: Some(5), ..Default::default() }).await.is_err());
    let s = h
        .set_config(VsCodeConfigPatch { auto_stop: Some(false), idle_stop_secs: Some(600), ..Default::default() })
        .await
        .unwrap();
    assert!(!s.config.auto_stop);
    // Persisted.
    let h2 = host(dir.path().join("vscode-server"));
    assert_eq!(h2.status().config.idle_stop_secs, 600);
}

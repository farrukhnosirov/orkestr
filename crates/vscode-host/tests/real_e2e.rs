//! End-to-end against the real network (ignored by default):
//!
//! ```text
//! cargo test -p orkestr-vscode-host --test real_e2e -- --ignored --nocapture
//! ```
//!
//! 1. resolves the latest VSCodium release on GitHub, downloads the REH-web
//!    package for this host, verifies its published sha256 and unpacks it;
//! 2. starts the server on a temp project and loads the workbench HTML with
//!    the token (and gets 403 without it);
//! 3. installs an extension from Open VSX through the server CLI into the
//!    server's extensions dir and checks the running server lists it;
//! 4. stops the server.
//!
//! `ORKESTR_E2E_VSCODE_ROOT` reuses an install directory between runs.

use std::sync::Arc;
use std::time::Duration;

use orkestr_platform::{NativeProcessControl, PlatformInfo};
use orkestr_protocol::methods::vscode::VsCodeState;
use orkestr_vscode_host::target::select_target;
use orkestr_vscode_host::{HostOptions, VsCodeHost};

const EXTENSION: &str = "mechatroner.rainbow-csv";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "downloads ~130 MB from GitHub and Open VSX"]
async fn real_download_start_and_open_vsx_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::env::var_os("ORKESTR_E2E_VSCODE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| tmp.path().join("vscode-server"));
    let info = PlatformInfo::detect();
    let host = VsCodeHost::new(HostOptions::new(&root, info.os, info.arch, Arc::new(NativeProcessControl))).unwrap();

    // 1. Release + verified install.
    let release = host.check_update().await.unwrap();
    println!("latest: {} {} ({} bytes)", release.version, release.asset_name, release.size_bytes);
    assert_eq!(release.license, "MIT");
    assert_eq!(release.extension_registry, "https://open-vsx.org");
    if host.status().installed.as_ref().map(|i| i.version.clone()) != Some(release.version.clone()) {
        let started = std::time::Instant::now();
        host.install(true, Some(release.version.clone())).await.unwrap();
        loop {
            let s = host.status();
            match s.state {
                VsCodeState::Installing => {
                    if let Some(p) = &s.progress {
                        print!("\r{:?} {}/{:?}      ", p.phase, p.done, p.total);
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                VsCodeState::Stopped => break,
                other => panic!("install ended in {other:?}: {:?}\n{:#?}", s.error, host.logs()),
            }
        }
        println!("\ninstalled in {:?}", started.elapsed());
    }
    let installation = host.status().installed.unwrap();
    println!("sha256 verified: {}", installation.sha256);
    assert_eq!(installation.sha256.len(), 64);

    // 2. Start on a project and load the UI.
    let project = tmp.path().join("demo project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("data.csv"), "a,b\n1,2\n").unwrap();
    let started = host.start(Some(&project)).await.unwrap();
    println!("UI: {}", started.origin);
    let http = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let first = http.get(&started.url).send().await.unwrap();
    assert_eq!(first.status(), 302, "the token is exchanged for a cookie");
    let cookie = first.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();
    let location = first.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.contains("folder="), "{location}");
    let page = http.get(format!("{}{location}", started.origin)).header("Cookie", &cookie).send().await.unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(html.contains("<!DOCTYPE html>") && html.contains("vscode-workbench-web-configuration"), "workbench HTML");
    let denied = http.get(format!("{}/", started.origin)).send().await.unwrap();
    assert_eq!(denied.status(), 403, "no token, no UI");
    assert!(host
        .logs()
        .iter()
        .all(|l| !l.contains(started.url.split("tkn=").nth(1).unwrap().split('&').next().unwrap())));

    // 3. Install an Open VSX extension through the server CLI.
    let dir = std::path::PathBuf::from(&installation.dir);
    let target = select_target(info.os, info.arch).unwrap();
    let layout = host.layout();
    let out = tokio::process::Command::new(dir.join(target.node_executable()))
        .arg(dir.join("out").join("server-main.js"))
        .arg("--extensions-dir")
        .arg(layout.extensions_dir())
        .arg("--user-data-dir")
        .arg(layout.user_data_dir())
        .arg("--server-data-dir")
        .arg(layout.server_data_dir())
        .args(["--accept-server-license-terms", "--install-extension", EXTENSION, "--force"])
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    println!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "extension install failed");
    assert!(stdout.contains("successfully installed") || stdout.contains("already installed"), "{stdout}");
    let listed = tokio::process::Command::new(dir.join(target.node_executable()))
        .arg(dir.join("out").join("server-main.js"))
        .arg("--extensions-dir")
        .arg(layout.extensions_dir())
        .args(["--accept-server-license-terms", "--list-extensions", "--show-versions"])
        .output()
        .await
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout).to_string();
    println!("installed extensions:\n{listed}");
    assert!(listed.to_lowercase().contains(EXTENSION), "{listed}");
    let manifest = std::fs::read_to_string(layout.extensions_dir().join("extensions.json")).unwrap();
    assert!(manifest.contains("open-vsx") || manifest.contains(EXTENSION), "{manifest}");

    // 4. Stop.
    let port = host.status().running.unwrap().port;
    let stopped = host.stop().await.unwrap();
    assert_eq!(stopped.state, VsCodeState::Stopped);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(http.get(format!("http://127.0.0.1:{port}/version")).send().await.is_err(), "server is gone");
}

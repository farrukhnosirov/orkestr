//! Live check of the Orkestr bridge (ignored by default): installs the bridge
//! extension into a real VSCodium REH server, opens the workbench in a
//! headless browser (the extension host only starts with a window), waits for
//! the bridge to connect and runs harmless commands through it.
//!
//! ```text
//! ORKESTR_E2E_VSCODE_ROOT=<an install made by real_e2e> \
//! ORKESTR_E2E_BROWSER="C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe" \
//! cargo test -p orkestr-vscode-host --test bridge_live -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use orkestr_platform::{NativeProcessControl, PlatformInfo};
use orkestr_vscode_host::bridge::BRIDGE_EXTENSION_ID;
use orkestr_vscode_host::{HostOptions, VsCodeHost};
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs an installed VSCodium server and a browser"]
async fn bridge_runs_commands_in_a_real_vscode() {
    let (Some(root), Some(browser)) =
        (std::env::var_os("ORKESTR_E2E_VSCODE_ROOT"), std::env::var_os("ORKESTR_E2E_BROWSER"))
    else {
        eprintln!("set ORKESTR_E2E_VSCODE_ROOT and ORKESTR_E2E_BROWSER");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let info = PlatformInfo::detect();
    let mut opts = HostOptions::new(&root, info.os, info.arch, Arc::new(NativeProcessControl));
    // The temp project is not a trusted folder: in Restricted Mode VS Code
    // disables extensions without untrusted-workspace support (rainbow-csv).
    opts.extra_server_args = vec!["--disable-workspace-trust".into()];
    let host = VsCodeHost::new(opts).unwrap();
    host.spawn_background();
    let project = tmp.path().join("bridge project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("data.csv"), "name,age\nalice,30\nbob,4\n").unwrap();

    let started = host.start(Some(&project)).await.unwrap();
    assert!(host.bridge_installed(), "the bridge was installed before the server started:\n{:#?}", host.logs());
    println!("UI: {}", started.origin);

    // A headless window loads the workbench → the extension host starts → the bridge connects.
    let profile = tmp.path().join("browser-profile");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let browser = Kill(
        std::process::Command::new(browser)
            .args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(&started.url)
            .spawn()
            .unwrap(),
    );

    let connected = host.bridge().wait_connected(Duration::from_secs(120)).await;
    let result = async {
        assert!(connected, "the bridge did not connect:\n{:#?}", host.logs());
        let hello = host.bridge().hello().unwrap();
        println!("bridge hello: {hello:?}");
        assert!(host.status().bridge_connected);

        let ids = host.bridge_call("listCommands", Value::Null, Duration::ZERO, Duration::from_secs(30)).await.unwrap();
        let ids: Vec<String> = serde_json::from_value(ids).unwrap();
        println!("{} runtime commands", ids.len());
        println!("rainbow at runtime: {:?}", ids.iter().filter(|i| i.contains("rainbow")).take(5).collect::<Vec<_>>());
        assert!(ids.iter().any(|i| i == "workbench.action.showCommands"));

        let exts =
            host.bridge_call("listExtensions", Value::Null, Duration::ZERO, Duration::from_secs(30)).await.unwrap();
        let user_exts: Vec<&Value> = exts
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| !e["id"].as_str().unwrap_or("").starts_with("vscode."))
            .collect();
        println!("extension host: {user_exts:?}");
        // Catalog of the installed extensions (bridge itself hidden).
        let catalog = host.catalog(None, None).await;
        println!("extensions: {:?}", catalog.iter().map(|e| (&e.id, e.commands.len())).collect::<Vec<_>>());
        assert!(catalog.iter().all(|e| e.id != BRIDGE_EXTENSION_ID));

        // Harmless commands round trip.
        let r = host
            .bridge_call(
                "execute",
                json!({ "command": "workbench.action.showCommands", "args": [] }),
                Duration::ZERO,
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        println!("showCommands → {r}");
        let csv = project.join("data.csv");
        let r = host
            .bridge_call("openFile", json!({ "path": csv, "line": 2 }), Duration::ZERO, Duration::from_secs(30))
            .await
            .unwrap();
        println!("openFile → {r}");
        if let Some(rainbow) = catalog.iter().find(|e| e.id == "mechatroner.rainbow-csv") {
            let cmd =
                rainbow.commands.iter().find(|c| c.id.to_lowercase().contains("align")).unwrap_or(&rainbow.commands[0]);
            // rainbow-csv runs in the browser's web-worker extension host,
            // which may come up a little after the workspace host.
            let mut r = Err(orkestr_vscode_host::HostError::Cancelled);
            for _ in 0..15 {
                r = host
                    .bridge_call(
                        "execute",
                        json!({ "command": cmd.id, "args": [] }),
                        Duration::ZERO,
                        Duration::from_secs(30),
                    )
                    .await;
                if r.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            println!("{} ({}) → {r:?}", cmd.id, cmd.title);
            assert!(r.is_ok(), "{r:?}");
        }
        // An unknown command reports the extension host's error.
        let r = host
            .bridge_call(
                "execute",
                json!({ "command": "orkestr.doesNotExist" }),
                Duration::ZERO,
                Duration::from_secs(30),
            )
            .await;
        println!("unknown → {r:?}");
        assert!(r.is_err());
    }
    .await;
    drop(browser);
    host.shutdown().await;
    result
}

//! Update checks against a local mock update server (HTTP on 127.0.0.1) and
//! local files: newer → available, same → none, bad signature → rejected,
//! offline / server errors classified.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use orkestr_updater::manifest::Target;
use orkestr_updater::signature::{generate_keypair, sign};
use orkestr_updater::{Channel, CheckOutcome, Checker, Location, SourceKind, UpdateError, UpdateSource};
use semver::Version;
use tokio_util::sync::CancellationToken;

const INSTALLER: &[u8] = b"MZ\x90\x00 pretend this is Orkestr_0.1.1_x64-setup.exe";
const NAME: &str = "Orkestr_0.1.1_x64-setup.exe";

#[derive(Clone)]
struct Srv {
    base: String,
    good_sig: String,
    bad_sig: String,
}

fn manifest(base: &str, version: &str, sig: &str) -> String {
    serde_json::json!({
        "version": version,
        "notes": "## What's new\n- updater",
        "pub_date": "2026-10-23T12:00:00Z",
        "platforms": { "windows-x86_64": { "url": format!("{base}/{NAME}"), "signature": sig } }
    })
    .to_string()
}

async fn serve(state: Arc<Mutex<Option<Srv>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    async fn json(body: String) -> impl IntoResponse {
        ([(header::CONTENT_TYPE, "application/json")], body)
    }
    let app = axum::Router::new()
        .route(
            "/newer/{target}/{arch}/{current}",
            get(|State(s): State<Arc<Mutex<Option<Srv>>>>| async move {
                let s = s.lock().unwrap().clone().unwrap();
                json(manifest(&s.base, "0.1.1", &s.good_sig)).await
            }),
        )
        .route(
            "/same/{target}/{arch}/{current}",
            get(|State(s): State<Arc<Mutex<Option<Srv>>>>| async move {
                let s = s.lock().unwrap().clone().unwrap();
                json(manifest(&s.base, "0.1.0", &s.good_sig)).await
            }),
        )
        .route(
            "/badsig/{target}/{arch}/{current}",
            get(|State(s): State<Arc<Mutex<Option<Srv>>>>| async move {
                let s = s.lock().unwrap().clone().unwrap();
                json(manifest(&s.base, "0.1.1", &s.bad_sig)).await
            }),
        )
        .route(
            "/beta",
            get(|State(s): State<Arc<Mutex<Option<Srv>>>>| async move {
                let s = s.lock().unwrap().clone().unwrap();
                json(manifest(&s.base, "0.2.0-beta.1", &s.good_sig)).await
            }),
        )
        .route("/none", get(|| async { StatusCode::NO_CONTENT }))
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .route("/html", get(|| async { "<html>captive portal</html>" }))
        .route(&format!("/{NAME}"), get(|| async { INSTALLER }))
        .with_state(state);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn target() -> Target {
    Target { os: "windows".into(), arch: "x86_64".into(), installer: Some("nsis".into()) }
}

struct Fixture {
    base: String,
    pk: String,
    sk: String,
}

async fn fixture() -> Fixture {
    let (sk, pk) = generate_keypair("pw").unwrap();
    let good_sig = sign(INSTALLER, &sk, "pw", NAME, &Version::new(0, 1, 1)).unwrap();
    let bad_sig = sign(b"something else", &sk, "pw", NAME, &Version::new(0, 1, 1)).unwrap();
    let state = Arc::new(Mutex::new(None));
    let base = serve(Arc::clone(&state)).await;
    *state.lock().unwrap() = Some(Srv { base: base.clone(), good_sig, bad_sig });
    Fixture { base, pk, sk }
}

fn checker(pk: &str, channel: Channel) -> Checker {
    Checker::new(pk, target(), Version::new(0, 1, 0), channel).unwrap()
}

fn remote(base: &str, path: &str) -> UpdateSource {
    UpdateSource::Remote { endpoint: format!("{base}{path}") }
}

#[tokio::test]
async fn newer_version_is_available_and_downloads_verified() {
    let f = fixture().await;
    let c = checker(&f.pk, Channel::Stable);
    let out = c.check(&remote(&f.base, "/newer/{{target}}/{{arch}}/{{current_version}}")).await.unwrap();
    let CheckOutcome::Available(info) = out else { panic!("expected an update, got {out:?}") };
    assert_eq!(info.version, "0.1.1");
    assert_eq!(info.current_version, "0.1.0");
    assert_eq!(info.source, SourceKind::Remote);
    assert_eq!(info.file_name, NAME);
    assert_eq!(info.platform_key, "windows-x86_64");
    assert!(info.notes.as_deref().unwrap().contains("updater"));
    // The normalized manifest is what tauri-plugin-updater's `restore_update` parses.
    assert_eq!(info.manifest["version"], "0.1.1");
    assert!(info.manifest["platforms"]["windows-x86_64"]["url"].as_str().unwrap().starts_with("http://127.0.0.1"));

    let dir = tempfile::tempdir().unwrap();
    let mut seen = Vec::new();
    let path = c.fetch(&info, dir.path(), false, &CancellationToken::new(), |d, t| seen.push((d, t))).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), INSTALLER);
    assert_eq!(seen.last(), Some(&(INSTALLER.len() as u64, Some(INSTALLER.len() as u64))));
    assert!(!dir.path().join(format!("{NAME}.part")).exists());
}

#[tokio::test]
async fn same_version_means_up_to_date() {
    let f = fixture().await;
    let out = checker(&f.pk, Channel::Stable)
        .check(&remote(&f.base, "/same/{{target}}/{{arch}}/{{current_version}}"))
        .await
        .unwrap();
    assert_eq!(out, CheckOutcome::UpToDate { latest: Some("0.1.0".into()) });
    let none = checker(&f.pk, Channel::Stable).check(&remote(&f.base, "/none")).await.unwrap();
    assert_eq!(none, CheckOutcome::UpToDate { latest: None });
}

#[tokio::test]
async fn bad_signature_is_rejected_and_the_download_discarded() {
    let f = fixture().await;
    let c = checker(&f.pk, Channel::Stable);
    let CheckOutcome::Available(info) =
        c.check(&remote(&f.base, "/badsig/{{target}}/{{arch}}/{{current_version}}")).await.unwrap()
    else {
        panic!("expected an offer")
    };
    let dir = tempfile::tempdir().unwrap();
    let err = c.fetch(&info, dir.path(), false, &CancellationToken::new(), |_, _| {}).await.unwrap_err();
    assert_eq!(err.code(), "bad_signature", "{err}");
    assert!(!dir.path().join(NAME).exists(), "a tampered download must not be kept");

    // A valid signature by another key is rejected as well.
    let (_, other_pk) = generate_keypair("other").unwrap();
    let c2 = checker(&other_pk, Channel::Stable);
    let CheckOutcome::Available(info) =
        c2.check(&remote(&f.base, "/newer/{{target}}/{{arch}}/{{current_version}}")).await.unwrap()
    else {
        panic!("expected an offer")
    };
    let err = c2.fetch(&info, dir.path(), false, &CancellationToken::new(), |_, _| {}).await.unwrap_err();
    assert_eq!(err.code(), "bad_signature");
}

#[tokio::test]
async fn beta_releases_only_on_the_beta_channel() {
    let f = fixture().await;
    let stable = checker(&f.pk, Channel::Stable).check(&remote(&f.base, "/beta")).await.unwrap();
    assert_eq!(stable, CheckOutcome::UpToDate { latest: Some("0.2.0-beta.1".into()) });
    let beta = checker(&f.pk, Channel::Beta).check(&remote(&f.base, "/beta")).await.unwrap();
    assert!(matches!(beta, CheckOutcome::Available(i) if i.prerelease));
}

#[tokio::test]
async fn errors_are_classified_for_offline_friendly_messages() {
    let f = fixture().await;
    let c = checker(&f.pk, Channel::Stable);
    assert!(matches!(c.check(&remote(&f.base, "/boom")).await, Err(UpdateError::Server(500))));
    assert_eq!(c.check(&remote(&f.base, "/html")).await.unwrap_err().code(), "bad_manifest");
    // Nothing listens on port 9 of loopback.
    let offline =
        c.check(&UpdateSource::Remote { endpoint: "http://127.0.0.1:9/latest.json".into() }).await.unwrap_err();
    assert_eq!(offline.code(), "offline", "{offline}");
    let insecure =
        c.check(&UpdateSource::Remote { endpoint: "http://example.com/latest.json".into() }).await.unwrap_err();
    assert_eq!(insecure.code(), "invalid_endpoint");
}

fn write_release(dir: &Path, sk: &str, version: &str) -> std::path::PathBuf {
    let name = format!("Orkestr_{version}_x64-setup.exe");
    let installer = dir.join(&name);
    std::fs::write(&installer, INSTALLER).unwrap();
    let sig = sign(INSTALLER, sk, "pw", &name, &Version::parse(version).unwrap()).unwrap();
    std::fs::write(dir.join(format!("{name}.sig")), &sig).unwrap();
    let m = serde_json::json!({
        "version": version,
        "platforms": { "windows-x86_64": { "url": name, "signature": sig } }
    });
    let manifest = dir.join("latest.json");
    std::fs::write(&manifest, m.to_string()).unwrap();
    installer
}

#[tokio::test]
async fn local_manifest_and_installer_channels() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let installer = write_release(dir.path(), &f.sk, "0.1.1");
    let c = checker(&f.pk, Channel::Stable);

    // latest.json with a relative URL next to it.
    let CheckOutcome::Available(info) =
        c.check(&UpdateSource::Local { path: dir.path().join("latest.json") }).await.unwrap()
    else {
        panic!("expected an update")
    };
    assert_eq!(info.source, SourceKind::LocalManifest);
    assert_eq!(info.location, Location::File(installer.clone()));
    assert!(info.manifest["platforms"]["windows-x86_64"]["url"].as_str().unwrap().starts_with("file:///"));
    let fetched = c.fetch(&info, dir.path(), false, &CancellationToken::new(), |_, _| {}).await.unwrap();
    assert_eq!(fetched, installer);

    // The release folder itself (a network share, a USB stick): its latest.json is used.
    let out = c.check(&UpdateSource::Local { path: dir.path().to_path_buf() }).await.unwrap();
    assert!(matches!(out, CheckOutcome::Available(i) if i.source == SourceKind::LocalManifest));

    // The installer itself, signed (.sig next to it): verified at check time.
    let CheckOutcome::Available(info) = c.check(&UpdateSource::Local { path: installer.clone() }).await.unwrap() else {
        panic!("expected an update")
    };
    assert_eq!(info.source, SourceKind::LocalInstaller);
    assert!(info.signature.is_some());

    // Tampered installer next to a valid .sig → rejected.
    std::fs::write(&installer, b"MZ evil").unwrap();
    let err = c.check(&UpdateSource::Local { path: installer.clone() }).await.unwrap_err();
    assert_eq!(err.code(), "bad_signature");

    // Unsigned installer: offered, but fetch needs explicit consent.
    std::fs::remove_file(format!("{}.sig", installer.display())).unwrap();
    let CheckOutcome::Available(info) = c.check(&UpdateSource::Local { path: installer.clone() }).await.unwrap() else {
        panic!("expected an update")
    };
    assert!(info.signature.is_none() && info.manifest.is_null());
    let err = c.fetch(&info, dir.path(), false, &CancellationToken::new(), |_, _| {}).await.unwrap_err();
    assert_eq!(err.code(), "unsigned");
    assert!(c.fetch(&info, dir.path(), true, &CancellationToken::new(), |_, _| {}).await.is_ok());

    // The same version locally → up to date.
    let same = tempfile::tempdir().unwrap();
    write_release(same.path(), &f.sk, "0.1.0");
    let out = c.check(&UpdateSource::Local { path: same.path().join("latest.json") }).await.unwrap();
    assert_eq!(out, CheckOutcome::UpToDate { latest: Some("0.1.0".into()) });
}

#[tokio::test]
async fn cancelled_download_leaves_nothing_behind() {
    let f = fixture().await;
    let c = checker(&f.pk, Channel::Stable);
    let CheckOutcome::Available(info) =
        c.check(&remote(&f.base, "/newer/{{target}}/{{arch}}/{{current_version}}")).await.unwrap()
    else {
        panic!()
    };
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = c.fetch(&info, dir.path(), false, &cancel, |_, _| {}).await.unwrap_err();
    assert_eq!(err.code(), "cancelled");
    assert_eq!(std::fs::read_dir(dir.path()).map(|d| d.count()).unwrap_or(0), 0);
}

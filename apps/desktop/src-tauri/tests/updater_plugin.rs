//! Compatibility of Orkestr's release signing (`orkestr-release`) and
//! verified manifests (`orkestr-updater`) with `tauri-plugin-updater`, which
//! performs the install: `restore_update` must accept our manifest (absolute
//! `file://` URL, version recorded in the signature) and `Update::verify`
//! must accept the installer and reject a tampered one or one signed for
//! another version. (Runs with the Common Controls manifest that build.rs
//! embeds into test binaries.)

use orkestr_updater::{CheckOutcome, Checker, Target, UpdateSource};
use semver::Version;

#[test]
fn the_updater_plugin_accepts_orkestr_signed_releases() {
    use orkestr_updater::signature::{generate_keypair, sign};
    use orkestr_updater::Channel;
    use tauri_plugin_updater::UpdaterExt as _;

    let (sk, pk) = generate_keypair("pw").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let name = "Orkestr_9.9.9_x64-setup.exe";
    let bytes = b"MZ\x90\x00 a pretend NSIS installer".to_vec();
    std::fs::write(dir.path().join(name), &bytes).unwrap();
    let sig = sign(&bytes, &sk, "pw", name, &Version::new(9, 9, 9)).unwrap();
    let manifest = serde_json::json!({
        "version": "9.9.9",
        "notes": "notes",
        "pub_date": "2026-10-23T12:00:00Z",
        "platforms": { "windows-x86_64": { "url": name, "signature": sig } }
    });
    std::fs::write(dir.path().join("latest.json"), manifest.to_string()).unwrap();

    let checker =
        Checker::new(pk.clone(), Target::current(Some("nsis")), Version::new(0, 1, 0), Channel::Stable).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let CheckOutcome::Available(info) =
        rt.block_on(checker.check(&UpdateSource::Local { path: dir.path().join("latest.json") })).unwrap()
    else {
        panic!("expected an update")
    };

    let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
    ctx.config_mut().plugins.0.insert("updater".into(), serde_json::json!({ "pubkey": pk, "endpoints": [] }));
    let app = tauri::test::mock_builder().plugin(tauri_plugin_updater::Builder::new().build()).build(ctx).unwrap();
    let updater = app
        .updater_builder()
        .endpoints(vec!["https://localhost/orkestr-updates".parse().unwrap()])
        .unwrap()
        .build()
        .unwrap();
    let update = updater.restore_update(info.manifest.clone()).unwrap().expect("9.9.9 is newer");
    assert_eq!(update.version, "9.9.9");
    update.verify(&bytes).expect("the plugin accepts orkestr-release signatures");
    assert!(update.verify(b"MZ tampered").is_err(), "the plugin rejects a tampered installer");

    // A manifest whose signature was made for another version is rejected by the plugin too.
    let old_sig = sign(&bytes, &sk, "pw", name, &Version::new(1, 0, 0)).unwrap();
    let mut forged = info.manifest.clone();
    forged["platforms"]["windows-x86_64"]["signature"] = old_sig.into();
    let update = updater.restore_update(forged).unwrap().unwrap();
    assert!(update.verify(&bytes).is_err(), "version pinning in the signature");
}

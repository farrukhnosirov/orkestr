include!("src/app_commands.rs");

fn main() {
    // Test binaries (the `tauri::test` mock runtime) need the Common Controls v6
    // manifest too; tauri-build embeds it into the app binary only.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
            .join("windows")
            .join("test.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}", manifest.display());
    }
    // Declaring the app commands scopes them by capability: orb, HUD and
    // panel windows may call only the commands their capability grants
    // (capabilities/*.json); undeclared, every window could call all of them.
    let manifest = tauri_build::AppManifest::new().commands(APP_COMMANDS);
    if let Err(error) = tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)) {
        println!("{error:#}");
        std::process::exit(1);
    }
}

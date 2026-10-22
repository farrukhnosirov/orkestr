include!("src/app_commands.rs");

fn main() {
    // Declaring the app commands scopes them by capability: orb, HUD and
    // panel windows may call only the commands their capability grants
    // (capabilities/*.json); undeclared, every window could call all of them.
    let manifest = tauri_build::AppManifest::new().commands(APP_COMMANDS);
    if let Err(error) = tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)) {
        println!("{error:#}");
        std::process::exit(1);
    }
}

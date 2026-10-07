//! Live end-to-end check against the real Open VSX registry (network).
//! Ignored by default: `cargo test -p orkestr-extensions --test openvsx_live -- --ignored --nocapture`.

use std::sync::Arc;

use orkestr_extensions::lsp::recipes;
use orkestr_extensions::{ExtensionService, NoEvents, OpenVsxClient};
use orkestr_protocol::methods::extensions::{ExtensionSort, ThemeRef};
use orkestr_storage::Db;

async fn service(dir: &std::path::Path) -> ExtensionService {
    let db = Db::open(dir.join("t.db")).unwrap();
    db.migrate(orkestr_extensions::extensions_migrations()).await.unwrap();
    ExtensionService::open(db, dir.join("extensions"), OpenVsxClient::new(None).unwrap(), Arc::new(NoEvents))
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs network access to open-vsx.org"]
async fn install_real_extensions_from_open_vsx() {
    let dir = tempfile::tempdir().unwrap();
    let s = service(dir.path()).await;

    let (total, hits) = s.search("dracula", 0, 10, ExtensionSort::Relevance, None).await.unwrap();
    println!("search dracula: {total} hits, first: {:?}", hits.first().map(|h| &h.id));
    assert!(total > 0);
    let details = s.details("dracula-theme", "theme-dracula").await.unwrap();
    println!(
        "details: {} {} verified={} license={:?} installable={} contributions={:?}",
        details.extension.id,
        details.extension.version,
        details.extension.verified,
        details.extension.license,
        details.installable,
        details.contributions.as_ref().map(|c| c.themes.len())
    );
    assert!(details.readme.is_some());

    // Color theme.
    let dracula = s.install("dracula-theme", "theme-dracula", None).await.unwrap();
    println!("installed {} {} sha256={}", dracula.id, dracula.version, dracula.sha256);
    let t = &dracula.contributions.themes[0];
    let theme = s.theme(&ThemeRef { extension_id: dracula.id.clone(), theme_id: t.id.clone() }).await.unwrap();
    println!(
        "theme {}: {} colors, {} token rules, bg={:?}",
        theme.label,
        theme.colors.len(),
        theme.token_colors.len(),
        theme.colors.get("editor.background")
    );
    assert!(theme.token_colors.len() > 20);
    assert!(theme.colors.contains_key("editor.background"));

    // File icon theme.
    let icons = s.install("PKief", "material-icon-theme", None).await.unwrap();
    let it = &icons.contributions.icon_themes[0];
    let resolved = s.icon_theme(&ThemeRef { extension_id: icons.id.clone(), theme_id: it.id.clone() }).await.unwrap();
    let rust_icon = resolved.dark.file_extensions.get("rs").cloned().expect("rs icon");
    let def = &resolved.definitions[&rust_icon];
    let asset = s.read_asset(&icons.id, def.icon_path.as_deref().unwrap()).await.unwrap();
    println!(
        "icon theme {}: {} definitions, .rs → {rust_icon} ({}, {} bytes)",
        resolved.label,
        resolved.definitions.len(),
        asset.mime,
        asset.text.as_ref().map_or(0, String::len)
    );
    assert_eq!(asset.mime, "image/svg+xml");

    // Grammar + language configuration + snippets.
    let go = s.install("golang", "Go", None).await.unwrap();
    println!(
        "golang.go: {} grammars {:?}, {} languages, {} snippet files, unsupported: {:?}",
        go.contributions.grammars.len(),
        go.contributions.grammars.iter().map(|g| &g.scope_name).collect::<Vec<_>>(),
        go.contributions.languages.len(),
        go.contributions.snippets.len(),
        go.contributions.unsupported.iter().map(|u| &u.kind).collect::<Vec<_>>()
    );
    // golang.go ships grammars for go.mod / go.sum (VS Code has Go itself built in).
    let g = &go.contributions.grammars[0];
    let grammar = s.read_asset(&go.id, &g.path).await.unwrap();
    assert!(grammar.text.unwrap().contains(&g.scope_name));
    // A language Monaco lacks: TOML grammar + injections into Markdown.
    let toml = s.install("tamasfe", "even-better-toml", None).await.unwrap();
    let tg = toml.contributions.grammars.iter().find(|g| g.language.as_deref() == Some("toml")).expect("toml grammar");
    let text = s.read_asset(&toml.id, &tg.path).await.unwrap().text.unwrap();
    println!(
        "even-better-toml: grammar {} ({} bytes), injections {:?}",
        tg.scope_name,
        text.len(),
        toml.contributions
            .grammars
            .iter()
            .filter(|g| !g.injection_to.is_empty())
            .map(|g| &g.scope_name)
            .collect::<Vec<_>>()
    );
    assert!(text.contains("source.toml"));
    if let Some(cfg) = go.contributions.languages.iter().find_map(|l| l.configuration.clone()) {
        let c = s.language_config(&go.id, &cfg).await.unwrap();
        println!("go language config keys: {:?}", c.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    }
    for sn in &go.contributions.snippets {
        let list = s.snippets(&go.id, &sn.path).await.unwrap();
        println!("go snippets {}: {}", sn.path, list.len());
    }

    // Platform-specific build with a bundled language server.
    let ra = s.install("rust-lang", "rust-analyzer", None).await.unwrap();
    println!(
        "rust-analyzer {} target={:?} servers={:?}",
        ra.version, ra.target_platform, ra.contributions.language_servers
    );
    let dirs = s.enabled_dirs().await.unwrap();
    let resolved = recipes::resolve(recipes::recipe("rust-analyzer").unwrap(), &dirs, &|_| None);
    println!("rust-analyzer resolved from extension: {:?}", resolved.as_ref().map(recipes::Resolved::display));
    assert!(resolved.is_some(), "the platform build bundles server/rust-analyzer");

    // Uninstall cleans up.
    s.uninstall(&go.id).await.unwrap();
    assert!(s.list().await.unwrap().iter().all(|e| e.id != go.id));
}

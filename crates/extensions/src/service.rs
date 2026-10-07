//! The extension service: install / uninstall / enable, contributions as
//! data, editor preferences. One instance per core.

use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use orkestr_protocol::methods::extensions::{
    EditorPrefs, ExtensionAssetResult, ExtensionInstallPhase, ExtensionInstallProgressPayload, ExtensionSource,
    ExtensionsChangedPayload, InstalledExtension, MarketplaceDetails, MarketplaceExtension, ResolvedIconTheme,
    ResolvedTheme, Snippet, ThemeRef,
};
use orkestr_storage::Db;
use parking_lot::Mutex;
use serde_json::Value;

use crate::archive::{self, Limits};
use crate::error::{ExtError, ExtResult};
use crate::openvsx::{self, OpenVsxClient};
use crate::store::{Row, Store};
use crate::{icon_theme, manifest, paths, theme};

/// Receives progress and change notifications (the core publishes them as events).
pub trait ExtensionEvents: Send + Sync {
    /// Install progress.
    fn progress(&self, p: ExtensionInstallProgressPayload);
    /// Registry / preference change.
    fn changed(&self, p: ExtensionsChangedPayload);
}

/// Discards notifications (tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoEvents;

impl ExtensionEvents for NoEvents {
    fn progress(&self, _p: ExtensionInstallProgressPayload) {}
    fn changed(&self, _p: ExtensionsChangedPayload) {}
}

/// Max size of an asset served to the editor.
const MAX_ASSET: u64 = 16 << 20;
/// Max size of a local `.vsix`.
const MAX_LOCAL_VSIX: u64 = 400 << 20;

/// Extension service.
pub struct ExtensionService {
    root: PathBuf,
    store: Store,
    registry: OpenVsxClient,
    events: Arc<dyn ExtensionEvents>,
    installing: Mutex<HashSet<String>>,
    limits: Limits,
}

impl std::fmt::Debug for ExtensionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionService").field("root", &self.root).finish_non_exhaustive()
    }
}

struct InstallingGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}

impl Drop for InstallingGuard<'_> {
    fn drop(&mut self) {
        self.set.lock().remove(&self.key);
    }
}

fn dir_name(ext: &InstalledExtension) -> String {
    let clean = |s: &str| -> String {
        s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
    };
    format!("{}-{}", clean(&ext.id), clean(&ext.version))
}

fn mime_of(path: &str) -> Option<(&'static str, bool)> {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    Some(match ext {
        "json" | "jsonc" | "code-snippets" => ("application/json", true),
        "tmlanguage" | "plist" | "tmtheme" | "xml" => ("application/xml", true),
        "yaml" | "yml" => ("application/yaml", true),
        "svg" => ("image/svg+xml", true),
        "md" | "txt" => ("text/plain", true),
        "png" => ("image/png", false),
        "jpg" | "jpeg" => ("image/jpeg", false),
        "gif" => ("image/gif", false),
        "webp" => ("image/webp", false),
        "woff" => ("font/woff", false),
        "woff2" => ("font/woff2", false),
        "ttf" => ("font/ttf", false),
        "otf" => ("font/otf", false),
        _ => return None,
    })
}

impl ExtensionService {
    /// Opens the service; `root` is `<data_dir>/extensions`.
    pub async fn open(
        db: Db,
        root: PathBuf,
        registry: OpenVsxClient,
        events: Arc<dyn ExtensionEvents>,
    ) -> ExtResult<Self> {
        std::fs::create_dir_all(&root)?;
        let svc = Self {
            root,
            store: Store::new(db),
            registry,
            events,
            installing: Mutex::new(HashSet::new()),
            limits: Limits::default(),
        };
        svc.reconcile().await?;
        Ok(svc)
    }

    /// Extensions folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The registry client.
    #[must_use]
    pub fn registry(&self) -> &OpenVsxClient {
        &self.registry
    }

    /// The persisted store (trust records live here too).
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Removes stale staging folders and rows whose folder vanished.
    async fn reconcile(&self) -> ExtResult<()> {
        let staging = self.root.join(".staging");
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        for row in self.store.list().await? {
            if !self.root.join(&row.dir).join("package.json").is_file() {
                tracing::warn!(extension = %row.ext.id, "extension folder is missing; removing it from the registry");
                self.store.delete(&row.ext.id).await?;
            }
        }
        Ok(())
    }

    /// Installed extensions, sorted by display name.
    pub async fn list(&self) -> ExtResult<Vec<InstalledExtension>> {
        let mut v: Vec<_> = self.store.list().await?.into_iter().map(|r| r.ext).collect();
        v.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        Ok(v)
    }

    /// Enabled extensions with their folders (for language-server discovery).
    pub async fn enabled_dirs(&self) -> ExtResult<Vec<(String, PathBuf)>> {
        Ok(self
            .store
            .list()
            .await?
            .into_iter()
            .filter(|r| r.ext.enabled)
            .map(|r| (r.ext.id, self.root.join(r.dir)))
            .collect())
    }

    async fn row(&self, id: &str) -> ExtResult<Row> {
        self.store.get(id).await?.ok_or_else(|| ExtError::NotFound(format!("extension `{id}` is not installed")))
    }

    async fn enabled_row(&self, id: &str) -> ExtResult<Row> {
        let row = self.row(id).await?;
        if !row.ext.enabled {
            return Err(ExtError::Conflict(format!("extension `{id}` is disabled")));
        }
        Ok(row)
    }

    fn progress(
        &self,
        key: &str,
        phase: ExtensionInstallPhase,
        received: u64,
        total: Option<u64>,
        error: Option<String>,
    ) {
        self.events.progress(ExtensionInstallProgressPayload { key: key.to_owned(), phase, received, total, error });
    }

    fn begin(&self, key: &str) -> ExtResult<InstallingGuard<'_>> {
        if !self.installing.lock().insert(key.to_owned()) {
            return Err(ExtError::Conflict(format!("`{key}` is already being installed")));
        }
        Ok(InstallingGuard { set: &self.installing, key: key.to_owned() })
    }

    /// Searches Open VSX and marks installed hits.
    pub async fn search(
        &self,
        query: &str,
        offset: u32,
        size: u32,
        sort: orkestr_protocol::methods::extensions::ExtensionSort,
        category: Option<&str>,
    ) -> ExtResult<(u64, Vec<MarketplaceExtension>)> {
        let (total, mut items) = self.registry.search(query, offset, size, sort, category).await?;
        let installed = self.store.list().await?;
        for it in &mut items {
            it.installed_version = installed.iter().find(|r| r.ext.id == it.id).map(|r| r.ext.version.clone());
        }
        Ok((total, items))
    }

    /// Open VSX details incl. README and contribution classification.
    pub async fn details(&self, namespace: &str, name: &str) -> ExtResult<MarketplaceDetails> {
        let meta = self.registry.metadata(namespace, name).await?;
        let mut header = openvsx::summary_of(&meta).ok_or_else(|| ExtError::NotFound(format!("{namespace}.{name}")))?;
        header.installed_version = self.store.get(&header.id).await?.map(|r| r.ext.version);
        let list = |k: &str| -> Vec<String> {
            meta.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let mut versions: Vec<String> = meta
            .get("allVersions")
            .and_then(Value::as_object)
            .map(|o| o.keys().filter(|k| *k != "latest" && *k != "pre-release").cloned().collect())
            .unwrap_or_default();
        versions.truncate(50);
        let targets: Vec<String> =
            meta.get("downloads").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let target = openvsx::current_target();
        let installable = targets.is_empty() || targets.iter().any(|t| t == &target || t == "universal");
        let readme = match meta.pointer("/files/readme").and_then(Value::as_str) {
            Some(u) => self.registry.text_file(u).await.ok(),
            None => None,
        };
        let contributions = match meta.pointer("/files/manifest").and_then(Value::as_str) {
            Some(u) => match self.registry.text_file(u).await {
                Ok(text) => manifest::parse(&text, None).ok().map(|m| {
                    let mut s = manifest::summarize(&m, None);
                    s.language_servers = crate::lsp::recipes::servers_for_extension(&m.id());
                    s
                }),
                Err(_) => None,
            },
            None => None,
        };
        let s = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty());
        Ok(MarketplaceDetails {
            extension: header,
            versions,
            categories: list("categories"),
            tags: list("tags"),
            repository: s("repository"),
            homepage: s("homepage"),
            engine: meta.pointer("/engines/vscode").and_then(Value::as_str).map(str::to_owned),
            target_platforms: targets,
            install_target: target,
            installable,
            contributions,
            readme,
            pre_release: meta.get("preRelease").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// Downloads (SHA-256 verified) and installs an Open VSX extension.
    pub async fn install(&self, namespace: &str, name: &str, version: Option<&str>) -> ExtResult<InstalledExtension> {
        let key = format!("{namespace}.{name}").to_ascii_lowercase();
        let _guard = self.begin(&key)?;
        let result = async {
            let build = self.registry.resolve_build(namespace, name, version, &openvsx::current_target()).await?;
            self.progress(&key, ExtensionInstallPhase::Downloading, 0, None, None);
            let (bytes, sha) = self
                .registry
                .download(&build, |received, total| {
                    self.progress(&key, ExtensionInstallPhase::Downloading, received, total, None);
                })
                .await?;
            self.progress(&key, ExtensionInstallPhase::Verifying, bytes.len() as u64, Some(bytes.len() as u64), None);
            self.progress(&key, ExtensionInstallPhase::Extracting, bytes.len() as u64, Some(bytes.len() as u64), None);
            let ext = self
                .install_bytes(
                    bytes,
                    sha,
                    ExtensionSource::OpenVsx,
                    Some(build.verified),
                    Some(build.target.clone()),
                    Some(&key),
                )
                .await?;
            Ok::<_, ExtError>(ext)
        }
        .await;
        match &result {
            Ok(ext) => {
                self.progress(&key, ExtensionInstallPhase::Done, 0, None, None);
                self.changed("installed", &ext.id);
            }
            Err(e) => self.progress(&key, ExtensionInstallPhase::Failed, 0, None, Some(e.to_string())),
        }
        result
    }

    /// Installs a local `.vsix` file.
    pub async fn install_vsix(&self, path: &Path) -> ExtResult<InstalledExtension> {
        if !path.is_absolute() {
            return Err(ExtError::Invalid("the .vsix path must be absolute".into()));
        }
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("vsix")) {
            return Err(ExtError::Invalid("choose a .vsix file".into()));
        }
        let meta = std::fs::metadata(path).map_err(|_| ExtError::NotFound(format!("{}", path.display())))?;
        if !meta.is_file() {
            return Err(ExtError::Invalid(format!("{} is not a file", path.display())));
        }
        if meta.len() > MAX_LOCAL_VSIX {
            return Err(ExtError::TooLarge(format!("the .vsix is larger than {MAX_LOCAL_VSIX} bytes")));
        }
        let key = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "vsix".into());
        let _guard = self.begin(&key)?;
        self.progress(&key, ExtensionInstallPhase::Extracting, 0, Some(meta.len()), None);
        let p = path.to_path_buf();
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(p))
            .await
            .map_err(|e| ExtError::Invalid(e.to_string()))??;
        let sha = openvsx::sha256_hex(&bytes);
        let result = self.install_bytes(bytes, sha, ExtensionSource::Vsix, None, None, None).await;
        match &result {
            Ok(ext) => {
                self.progress(&key, ExtensionInstallPhase::Done, 0, None, None);
                self.changed("installed", &ext.id);
            }
            Err(e) => self.progress(&key, ExtensionInstallPhase::Failed, 0, None, Some(e.to_string())),
        }
        result
    }

    /// Unpacks verified `.vsix` bytes and registers the extension.
    pub async fn install_bytes(
        &self,
        bytes: Vec<u8>,
        sha256: String,
        source: ExtensionSource,
        verified: Option<bool>,
        target: Option<String>,
        expect_id: Option<&str>,
    ) -> ExtResult<InstalledExtension> {
        let staging_root = self.root.join(".staging");
        std::fs::create_dir_all(&staging_root)?;
        let staging = staging_root.join(uuid_like());
        let limits = self.limits;
        let st = staging.clone();
        tokio::task::spawn_blocking(move || archive::extract(Cursor::new(bytes), &st, limits))
            .await
            .map_err(|e| ExtError::Invalid(e.to_string()))??;
        let cleanup = |e: ExtError| {
            let _ = std::fs::remove_dir_all(&staging);
            e
        };
        let (text, nls) = archive::read_manifest(&staging).map_err(cleanup)?;
        let m = manifest::parse(&text, nls.as_ref()).map_err(cleanup)?;
        if let Some(expected) = expect_id {
            if m.id() != expected.to_ascii_lowercase() {
                return Err(cleanup(ExtError::Unsafe(format!(
                    "the downloaded package is `{}`, not the requested `{expected}`",
                    m.id()
                ))));
            }
        }
        let mut summary = manifest::summarize(&m, nls.as_ref());
        summary.language_servers = crate::lsp::recipes::servers_for_extension(&m.id());
        let previous = self.store.get(&m.id()).await?;
        let ext = InstalledExtension {
            id: m.id(),
            publisher: m.publisher.clone(),
            name: m.name.clone(),
            display_name: m.display_name.clone(),
            version: m.version.clone(),
            description: m.description.clone(),
            license: m.license.clone(),
            source,
            verified,
            enabled: previous.as_ref().is_none_or(|p| p.ext.enabled),
            installed_at: chrono::Utc::now(),
            sha256,
            engine: m.engine.clone(),
            engine_compatible: m.engine_compatible(),
            target_platform: target,
            icon: m.icon.clone(),
            contributions: summary,
        };
        let dir = dir_name(&ext);
        let final_dir = self.root.join(&dir);
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir).map_err(|e| cleanup(e.into()))?;
        }
        std::fs::rename(&staging, &final_dir).map_err(|e| cleanup(e.into()))?;
        self.store.upsert(&ext, &dir).await?;
        if let Some(prev) = previous {
            if prev.dir != dir {
                let _ = std::fs::remove_dir_all(self.root.join(prev.dir));
            }
        }
        tracing::info!(extension = %ext.id, version = %ext.version, "extension installed");
        Ok(ext)
    }

    /// Enables or disables an extension.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> ExtResult<InstalledExtension> {
        if !self.store.set_enabled(id, enabled).await? {
            return Err(ExtError::NotFound(format!("extension `{id}` is not installed")));
        }
        let row = self.row(id).await?;
        self.changed(if enabled { "enabled" } else { "disabled" }, &row.ext.id);
        Ok(row.ext)
    }

    /// Uninstalls an extension (folder + registry row; preferences pointing at it are cleared).
    pub async fn uninstall(&self, id: &str) -> ExtResult<()> {
        let row = self.row(id).await?;
        self.store.delete(&row.ext.id).await?;
        let dir = self.root.join(&row.dir);
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(error = %e, dir = %dir.display(), "could not remove the extension folder");
        }
        let mut prefs = self.store.prefs().await?;
        let before = prefs.clone();
        if prefs.editor_theme.as_ref().is_some_and(|t| t.extension_id == row.ext.id) {
            prefs.editor_theme = None;
        }
        if prefs.icon_theme.as_ref().is_some_and(|t| t.extension_id == row.ext.id) {
            prefs.icon_theme = None;
        }
        if prefs != before {
            self.store.set_prefs(&prefs).await?;
        }
        self.changed("uninstalled", &row.ext.id);
        Ok(())
    }

    fn changed(&self, reason: &str, id: &str) {
        self.events.changed(ExtensionsChangedPayload { reason: reason.into(), extension_id: Some(id.to_owned()) });
    }

    fn dir_of(&self, row: &Row) -> PathBuf {
        self.root.join(&row.dir)
    }

    /// README of an installed extension.
    pub async fn readme(&self, id: &str) -> ExtResult<Option<String>> {
        let row = self.row(id).await?;
        let dir = self.dir_of(&row);
        for name in ["README.md", "readme.md", "Readme.md", "README.MD"] {
            let p = dir.join(name);
            if p.is_file() {
                return Ok(archive::read_text_capped(&p, 2 << 20).ok());
            }
        }
        Ok(None)
    }

    /// Resolved color theme.
    pub async fn theme(&self, r: &ThemeRef) -> ExtResult<ResolvedTheme> {
        let row = self.enabled_row(&r.extension_id).await?;
        let contrib = row
            .ext
            .contributions
            .themes
            .iter()
            .find(|t| t.id == r.theme_id)
            .cloned()
            .ok_or_else(|| ExtError::NotFound(format!("theme `{}` in `{}`", r.theme_id, r.extension_id)))?;
        let dir = self.dir_of(&row);
        let path = contrib.path.clone();
        let body = tokio::task::spawn_blocking(move || theme::resolve(&dir, &path))
            .await
            .map_err(|e| ExtError::Invalid(e.to_string()))??;
        Ok(ResolvedTheme {
            theme: ThemeRef { extension_id: row.ext.id.clone(), theme_id: contrib.id },
            label: contrib.label,
            ui_theme: contrib.ui_theme,
            colors: body.colors,
            token_colors: body.token_colors,
            semantic_highlighting: body.semantic_highlighting,
        })
    }

    /// Resolved file icon theme.
    pub async fn icon_theme(&self, r: &ThemeRef) -> ExtResult<ResolvedIconTheme> {
        let row = self.enabled_row(&r.extension_id).await?;
        let contrib = row
            .ext
            .contributions
            .icon_themes
            .iter()
            .find(|t| t.id == r.theme_id)
            .cloned()
            .ok_or_else(|| ExtError::NotFound(format!("icon theme `{}` in `{}`", r.theme_id, r.extension_id)))?;
        let dir = self.dir_of(&row);
        let path = contrib.path.clone();
        let body = tokio::task::spawn_blocking(move || icon_theme::resolve(&dir, &path))
            .await
            .map_err(|e| ExtError::Invalid(e.to_string()))??;
        Ok(ResolvedIconTheme {
            theme: ThemeRef { extension_id: row.ext.id.clone(), theme_id: contrib.id },
            label: contrib.label,
            definitions: body.definitions,
            dark: body.dark,
            light: body.light,
            fonts: body.fonts,
            hides_explorer_arrows: body.hides_explorer_arrows,
        })
    }

    /// Reads an asset (grammar, icon, font, configuration) of an enabled extension.
    pub async fn read_asset(&self, id: &str, rel: &str) -> ExtResult<ExtensionAssetResult> {
        let row = self.enabled_row(id).await?;
        let rel = paths::normalize(rel)?;
        let (mime, text) =
            mime_of(&rel).ok_or_else(|| ExtError::Unsupported(format!("`{rel}`: this file type is not served")))?;
        let dir = self.dir_of(&row);
        tokio::task::spawn_blocking(move || {
            let p = paths::resolve_existing(&dir, &rel)?;
            let len = std::fs::metadata(&p)?.len();
            if len > MAX_ASSET {
                return Err(ExtError::TooLarge(format!("`{rel}` is larger than {MAX_ASSET} bytes")));
            }
            let bytes = std::fs::read(&p)?;
            if text {
                let s = String::from_utf8(bytes).map_err(|_| ExtError::Invalid(format!("`{rel}` is not UTF-8")))?;
                Ok(ExtensionAssetResult { mime: mime.into(), text: Some(s), base64: None })
            } else {
                Ok(ExtensionAssetResult {
                    mime: mime.into(),
                    text: None,
                    base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                })
            }
        })
        .await
        .map_err(|e| ExtError::Invalid(e.to_string()))?
    }

    async fn jsonc_file(&self, id: &str, rel: &str) -> ExtResult<String> {
        let a = self.read_asset(id, rel).await?;
        a.text.ok_or_else(|| ExtError::Invalid(format!("`{rel}` is not a text file")))
    }

    /// Parsed `language-configuration.json`.
    pub async fn language_config(&self, id: &str, rel: &str) -> ExtResult<Value> {
        let text = self.jsonc_file(id, rel).await?;
        crate::jsonc::parse(&text)
    }

    /// Parsed snippets file.
    pub async fn snippets(&self, id: &str, rel: &str) -> ExtResult<Vec<Snippet>> {
        let text = self.jsonc_file(id, rel).await?;
        crate::snippets::parse(&text)
    }

    /// Editor preferences.
    pub async fn prefs(&self) -> ExtResult<EditorPrefs> {
        self.store.prefs().await
    }

    /// Stores editor preferences (references must point at installed contributions).
    pub async fn set_prefs(&self, prefs: EditorPrefs) -> ExtResult<EditorPrefs> {
        if let Some(t) = &prefs.editor_theme {
            let row = self.row(&t.extension_id).await?;
            if !row.ext.contributions.themes.iter().any(|x| x.id == t.theme_id) {
                return Err(ExtError::NotFound(format!("theme `{}` in `{}`", t.theme_id, t.extension_id)));
            }
        }
        if let Some(t) = &prefs.icon_theme {
            let row = self.row(&t.extension_id).await?;
            if !row.ext.contributions.icon_themes.iter().any(|x| x.id == t.theme_id) {
                return Err(ExtError::NotFound(format!("icon theme `{}` in `{}`", t.theme_id, t.extension_id)));
            }
        }
        self.store.set_prefs(&prefs).await?;
        self.events.changed(ExtensionsChangedPayload { reason: "prefs".into(), extension_id: None });
        Ok(prefs)
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{n}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::make_zip;

    async fn svc() -> (tempfile::TempDir, ExtensionService) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::store::extensions_migrations()).await.unwrap();
        let s = ExtensionService::open(
            db,
            dir.path().join("extensions"),
            OpenVsxClient::new(Some("https://open-vsx.invalid".into())).unwrap(),
            Arc::new(NoEvents),
        )
        .await
        .unwrap();
        (dir, s)
    }

    fn theme_vsix(version: &str) -> Vec<u8> {
        let pkg = format!(
            r#"{{"name":"theme-demo","publisher":"Demo","version":"{version}","displayName":"Demo Theme",
               "license":"MIT","engines":{{"vscode":"^1.60.0"}},
               "contributes":{{"themes":[{{"label":"Demo Dark","uiTheme":"vs-dark","path":"./themes/dark.json"}}],
               "iconThemes":[{{"id":"demo-icons","label":"Demo Icons","path":"./icons/theme.json"}}],
               "snippets":[{{"language":"rust","path":"./snippets/rust.json"}}],
               "commands":[{{"command":"x"}}]}}}}"#
        );
        make_zip(&[
            ("extension/package.json", pkg.as_bytes(), None),
            ("extension/themes/dark.json", br##"{"colors":{"editor.background":"#123456"},"tokenColors":[{"scope":"comment","settings":{"foreground":"#00ff00"}}]}"##, None),
            ("extension/icons/theme.json", br#"{"iconDefinitions":{"f":{"iconPath":"./f.svg"}},"file":"f"}"#, None),
            ("extension/icons/f.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>", None),
            ("extension/snippets/rust.json", br#"{"main":{"prefix":"main","body":["fn main() {}"]}}"#, None),
            ("extension/README.md", b"# Demo", None),
        ])
    }

    #[tokio::test]
    async fn install_query_update_uninstall() {
        let (_d, s) = svc().await;
        let bytes = theme_vsix("1.0.0");
        let sha = openvsx::sha256_hex(&bytes);
        let ext = s.install_bytes(bytes, sha.clone(), ExtensionSource::Vsix, None, None, None).await.unwrap();
        assert_eq!(ext.id, "demo.theme-demo");
        assert!(ext.enabled);
        assert_eq!(ext.sha256, sha);
        assert_eq!(ext.contributions.themes[0].id, "Demo Dark");
        assert!(ext.contributions.unsupported.iter().any(|u| u.kind == "commands"));
        assert!(s.root().join("demo.theme-demo-1.0.0/package.json").is_file());

        let t = s.theme(&ThemeRef { extension_id: ext.id.clone(), theme_id: "Demo Dark".into() }).await.unwrap();
        assert_eq!(t.colors["editor.background"], "#123456");
        let it = s.icon_theme(&ThemeRef { extension_id: ext.id.clone(), theme_id: "demo-icons".into() }).await.unwrap();
        assert_eq!(it.definitions["f"].icon_path.as_deref(), Some("icons/f.svg"));
        let svg = s.read_asset(&ext.id, "icons/f.svg").await.unwrap();
        assert_eq!(svg.mime, "image/svg+xml");
        assert!(s.read_asset(&ext.id, "../x.json").await.is_err());
        assert!(matches!(s.read_asset(&ext.id, "package.exe").await, Err(ExtError::Unsupported(_))));
        let sn = s.snippets(&ext.id, "snippets/rust.json").await.unwrap();
        assert_eq!(sn[0].prefixes, vec!["main"]);
        assert_eq!(s.readme(&ext.id).await.unwrap().as_deref(), Some("# Demo"));

        // Preferences validate references.
        let prefs = EditorPrefs {
            editor_theme: Some(ThemeRef { extension_id: ext.id.clone(), theme_id: "Demo Dark".into() }),
            icon_theme: Some(ThemeRef { extension_id: ext.id.clone(), theme_id: "demo-icons".into() }),
        };
        s.set_prefs(prefs.clone()).await.unwrap();
        assert!(s
            .set_prefs(EditorPrefs {
                editor_theme: Some(ThemeRef { extension_id: ext.id.clone(), theme_id: "nope".into() }),
                icon_theme: None
            })
            .await
            .is_err());

        // Disable → contributions are no longer served.
        s.set_enabled(&ext.id, false).await.unwrap();
        assert!(matches!(s.read_asset(&ext.id, "icons/f.svg").await, Err(ExtError::Conflict(_))));
        s.set_enabled(&ext.id, true).await.unwrap();

        // Update replaces the folder.
        let bytes = theme_vsix("1.1.0");
        let sha = openvsx::sha256_hex(&bytes);
        let ext2 = s.install_bytes(bytes, sha, ExtensionSource::Vsix, None, None, None).await.unwrap();
        assert_eq!(ext2.version, "1.1.0");
        assert!(!s.root().join("demo.theme-demo-1.0.0").exists());
        assert_eq!(s.list().await.unwrap().len(), 1);

        s.uninstall(&ext.id).await.unwrap();
        assert!(s.list().await.unwrap().is_empty());
        assert!(!s.root().join("demo.theme-demo-1.1.0").exists());
        assert_eq!(s.prefs().await.unwrap(), EditorPrefs::default(), "prefs pointing at it are cleared");
    }

    #[tokio::test]
    async fn rejects_unexpected_package_and_zip_slip() {
        let (_d, s) = svc().await;
        let bytes = theme_vsix("1.0.0");
        let err = s
            .install_bytes(bytes, "x".into(), ExtensionSource::OpenVsx, Some(true), None, Some("other.ext"))
            .await
            .unwrap_err();
        assert!(matches!(err, ExtError::Unsafe(_)));
        let evil = make_zip(&[
            ("extension/package.json", br#"{"name":"a","publisher":"b","version":"1.0.0"}"#, None),
            ("extension/../../../pwned.txt", b"x", None),
        ]);
        assert!(matches!(
            s.install_bytes(evil, "x".into(), ExtensionSource::Vsix, None, None, None).await,
            Err(ExtError::Unsafe(_))
        ));
        assert!(s.list().await.unwrap().is_empty());
        let staging = s.root().join(".staging");
        assert_eq!(std::fs::read_dir(&staging).map(Iterator::count).unwrap_or(0), 0, "staging is cleaned up");
    }

    #[tokio::test]
    async fn install_vsix_validates_path() {
        let (d, s) = svc().await;
        assert!(s.install_vsix(Path::new("relative.vsix")).await.is_err());
        let txt = d.path().join("x.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(s.install_vsix(&txt).await.is_err());
        let vsix = d.path().join("demo.vsix");
        std::fs::write(&vsix, theme_vsix("2.0.0")).unwrap();
        let ext = s.install_vsix(&vsix).await.unwrap();
        assert_eq!(ext.version, "2.0.0");
        assert_eq!(ext.source, ExtensionSource::Vsix);
    }
}

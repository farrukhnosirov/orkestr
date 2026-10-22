//! Resolution of spoken names to things the OS can open: known apps
//! (`data/apps.json`), Start-menu apps (indexed at runtime), known folders
//! (`data/folders.json`, `shell:` URIs), sites and search engines
//! (`data/sites.json`), URLs, URIs and paths.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use orkestr_protocol::methods::computer::WindowInfo;

use crate::backend::InstalledApp;
use crate::text::{similarity, tokenize};

/// A known app.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownApp {
    /// Key (`notepad`).
    pub key: String,
    /// English names.
    pub en: Vec<String>,
    /// Russian names.
    pub ru: Vec<String>,
    /// Launch target.
    pub launch: String,
    /// Process names.
    #[serde(default)]
    pub processes: Vec<String>,
    /// Window title fragments (apps hosted by mmc / explorer).
    #[serde(default)]
    pub titles: Vec<String>,
    /// Never kill (shell processes).
    #[serde(default)]
    pub no_kill: bool,
}

impl KnownApp {
    fn names(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.key).chain(self.en.iter()).chain(self.ru.iter())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Folder {
    key: String,
    shell: String,
    en: Vec<String>,
    ru: Vec<String>,
}

/// A site.
#[derive(Debug, Clone, Deserialize)]
pub struct Site {
    /// Key.
    pub key: String,
    /// Home URL.
    pub url: String,
    /// English names.
    pub en: Vec<String>,
    /// Russian names.
    pub ru: Vec<String>,
    /// Search URL template (`{q}`).
    #[serde(default)]
    pub search: Option<String>,
}

#[derive(Deserialize)]
struct AppsFile {
    apps: Vec<KnownApp>,
}
#[derive(Deserialize)]
struct FoldersFile {
    folders: Vec<Folder>,
}
#[derive(Deserialize)]
struct SitesFile {
    sites: Vec<Site>,
}

/// Known apps table.
pub fn known_apps() -> &'static [KnownApp] {
    static V: OnceLock<Vec<KnownApp>> = OnceLock::new();
    V.get_or_init(|| {
        serde_json::from_str::<AppsFile>(include_str!("../data/apps.json")).map(|f| f.apps).unwrap_or_default()
    })
}

fn folders() -> &'static [Folder] {
    static V: OnceLock<Vec<Folder>> = OnceLock::new();
    V.get_or_init(|| {
        serde_json::from_str::<FoldersFile>(include_str!("../data/folders.json")).map(|f| f.folders).unwrap_or_default()
    })
}

/// Sites table.
pub fn sites() -> &'static [Site] {
    static V: OnceLock<Vec<Site>> = OnceLock::new();
    V.get_or_init(|| {
        serde_json::from_str::<SitesFile>(include_str!("../data/sites.json")).map(|f| f.sites).unwrap_or_default()
    })
}

/// Normalized comparison key of a name (stems / concepts).
#[must_use]
pub fn name_key(s: &str) -> String {
    tokenize(s).into_iter().map(|t| t.key).collect::<Vec<_>>().join(" ")
}

/// Name similarity 0..1 (stem-normalized; containment of whole words counts high).
#[must_use]
pub fn name_score(query: &str, name: &str) -> f32 {
    let (q, n) = (name_key(query), name_key(name));
    if q.is_empty() || n.is_empty() {
        return 0.0;
    }
    if q == n {
        return 1.0;
    }
    let qw: Vec<&str> = q.split(' ').collect();
    let nw: Vec<&str> = n.split(' ').collect();
    let contains = |a: &[&str], b: &[&str]| b.iter().all(|w| a.contains(w));
    if contains(&nw, &qw) || contains(&qw, &nw) {
        return 0.9;
    }
    similarity(&q, &n)
}

/// The known app a spoken name refers to.
#[must_use]
pub fn find_known_app(query: &str) -> Option<&'static KnownApp> {
    let mut best: Option<(f32, &KnownApp)> = None;
    for a in known_apps() {
        for n in a.names() {
            let s = name_score(query, n);
            if s >= 0.8 && best.is_none_or(|(b, _)| s > b) {
                best = Some((s, a));
            }
        }
    }
    best.map(|(_, a)| a)
}

/// Best Start-menu app for a name.
#[must_use]
pub fn find_installed<'a>(query: &str, apps: &'a [InstalledApp]) -> Option<&'a InstalledApp> {
    let mut best: Option<(f32, &InstalledApp)> = None;
    for a in apps {
        let s = name_score(query, &a.name);
        if s >= 0.8 && best.is_none_or(|(b, _)| s > b) {
            best = Some((s, a));
        }
    }
    best.map(|(_, a)| a)
}

/// True for `https://…`, `www.…` and bare domains (`ya.ru`, `github.com/x`).
#[must_use]
pub fn looks_like_url(s: &str) -> bool {
    let t = s.trim();
    if t.contains(char::is_whitespace) || t.is_empty() {
        return false;
    }
    if t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.") {
        return true;
    }
    let host = t.split('/').next().unwrap_or("");
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && parts.last().is_some_and(|tld| tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic))
        && !host.ends_with(".exe")
        && !host.ends_with(".lnk")
        && !host.ends_with(".msc")
}

/// `https://` prefixed URL.
#[must_use]
pub fn normalize_url(s: &str) -> String {
    let t = s.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        t.to_owned()
    } else {
        format!("https://{t}")
    }
}

/// True for `scheme:` URIs (not `C:\…`).
#[must_use]
pub fn is_uri(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else { return false };
    scheme.len() >= 2
        && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '+')
}

/// Site by spoken name.
#[must_use]
pub fn find_site(query: &str) -> Option<&'static Site> {
    let mut best: Option<(f32, &Site)> = None;
    for s in sites() {
        for n in std::iter::once(&s.key).chain(s.en.iter()).chain(s.ru.iter()) {
            let sc = name_score(query, n);
            if sc >= 0.85 && best.is_none_or(|(b, _)| sc > b) {
                best = Some((sc, s));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// URL of a site name or address (`None` when it is neither).
#[must_use]
pub fn site_url(query: &str) -> Option<String> {
    if looks_like_url(query) {
        return Some(normalize_url(query));
    }
    find_site(query).map(|s| s.url.clone())
}

fn encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Search URL (`engine`: site key / name; default Google).
#[must_use]
pub fn search_url(query: &str, engine: Option<&str>) -> String {
    let tpl = engine
        .and_then(|e| sites().iter().find(|s| s.key == e).or_else(|| find_site(e)))
        .and_then(|s| s.search.clone())
        .unwrap_or_else(|| "https://www.google.com/search?q={q}".to_owned());
    tpl.replace("{q}", &encode(query))
}

/// `ms-settings:` URI of a page id or spoken page name.
#[must_use]
pub fn settings_uri(page: &str) -> String {
    let p = page.trim();
    if p.is_empty() {
        return "ms-settings:".into();
    }
    if let Some(rest) = p.strip_prefix("ms-settings:") {
        return format!("ms-settings:{rest}");
    }
    if p.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        return format!("ms-settings:{p}");
    }
    // spoken name → look up the catalogue table
    #[derive(Deserialize)]
    struct Page {
        page: String,
        en: Vec<String>,
        ru: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Pages {
        pages: Vec<Page>,
    }
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    let pages = PAGES.get_or_init(|| {
        serde_json::from_str::<Pages>(include_str!("../data/settings.json")).map(|p| p.pages).unwrap_or_default()
    });
    let mut best: Option<(f32, &str)> = None;
    for pg in pages {
        for n in pg.en.iter().chain(pg.ru.iter()) {
            let s = name_score(p, n);
            if s >= 0.75 && best.is_none_or(|(b, _)| s > b) {
                best = Some((s, &pg.page));
            }
        }
    }
    format!("ms-settings:{}", best.map_or("", |(_, p)| p))
}

/// Expands `%VAR%` environment references.
#[must_use]
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('%') {
            let var = &after[..j];
            match std::env::var(var) {
                Ok(v) if !var.is_empty() => out.push_str(&v),
                _ => {
                    out.push('%');
                    out.push_str(var);
                    out.push('%');
                }
            }
            rest = &after[j + 1..];
        } else {
            out.push('%');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Known folder (`downloads`, `загрузки`, …) → its `shell:` URI / path.
#[must_use]
pub fn known_folder(query: &str) -> Option<String> {
    let q = query.trim();
    if let Some(f) = folders().iter().find(|f| f.key == q) {
        return Some(expand_env(&f.shell));
    }
    let mut best: Option<(f32, &Folder)> = None;
    for f in folders() {
        for n in f.en.iter().chain(f.ru.iter()) {
            let s = name_score(q, n);
            if s >= 0.85 && best.is_none_or(|(b, _)| s > b) {
                best = Some((s, f));
            }
        }
    }
    best.map(|(_, f)| expand_env(&f.shell))
}

/// Base directory for relative file paths (Documents, else home).
#[must_use]
pub fn documents_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(Path::to_path_buf).or_else(|| Some(u.home_dir().to_path_buf())))
        .unwrap_or_else(std::env::temp_dir)
}

fn user_dir(key: &str) -> Option<PathBuf> {
    let u = directories::UserDirs::new()?;
    let p = match key {
        "downloads" => u.download_dir(),
        "documents" => u.document_dir(),
        "desktop" => u.desktop_dir(),
        "pictures" => u.picture_dir(),
        "music" => u.audio_dir(),
        "videos" => u.video_dir(),
        "home" => Some(u.home_dir()),
        _ => None,
    }?;
    Some(p.to_path_buf())
}

/// A file-system path for file operations: absolute stays, `%VAR%` expands,
/// `<known folder>/<rest>` maps to the folder, otherwise relative to Documents.
#[must_use]
pub fn file_path(spoken: &str) -> PathBuf {
    let s = expand_env(spoken.trim().trim_matches('"'));
    let p = PathBuf::from(&s);
    if p.is_absolute() {
        return p;
    }
    let norm = s.replace('\\', "/");
    if let Some((head, rest)) = norm.split_once('/') {
        let key = folders().iter().find(|f| f.key == head || name_score(head, &f.key) >= 0.9).map(|f| f.key.clone());
        let key = key.or_else(|| {
            folders()
                .iter()
                .find(|f| f.en.iter().chain(f.ru.iter()).any(|n| name_score(head, n) >= 0.85))
                .map(|f| f.key.clone())
        });
        if let Some(dir) = key.as_deref().and_then(user_dir) {
            return dir.join(rest);
        }
    }
    documents_dir().join(s)
}

/// What to pass to `shell_open` for a launch target.
#[must_use]
pub fn launch_target(target: &str, installed: &[InstalledApp]) -> String {
    let t = expand_env(target.trim());
    if looks_like_url(&t) {
        return normalize_url(&t);
    }
    if is_uri(&t) || Path::new(&t).is_absolute() {
        return t;
    }
    if let Some(folder) = known_folder(&t) {
        return folder;
    }
    if let Some(app) = find_known_app(&t) {
        let direct = is_uri(&app.launch)
            || app.launch.ends_with(".exe")
            || app.launch.ends_with(".msc")
            || app.launch.ends_with(".cpl");
        if direct {
            return app.launch.clone();
        }
        // Start-menu entry first (handles per-user installs), else App Paths / PATH.
        for n in std::iter::once(&app.launch).chain(app.en.iter()) {
            if let Some(i) = find_installed(n, installed) {
                return i.target.clone();
            }
        }
        return app.launch.clone();
    }
    if let Some(i) = find_installed(&t, installed) {
        return i.target.clone();
    }
    t
}

/// True when `w` belongs to the app named `query`.
#[must_use]
pub fn window_matches(query: &str, w: &WindowInfo) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return false;
    }
    let process = crate::procs::norm(&w.process);
    let title = w.title.to_lowercase();
    if let Some(app) = find_known_app(q) {
        return app.processes.iter().any(|p| crate::procs::norm(p) == process)
            || app.titles.iter().any(|t| title.contains(&t.to_lowercase()));
    }
    let qn = crate::procs::norm(q);
    process == qn
        || (qn.chars().count() >= 4 && similarity(&process, &qn) >= 0.8)
        || (q.chars().count() >= 3 && title.contains(&q.to_lowercase()))
        || name_score(q, &w.title) >= 0.9
}

/// Process names of an app (for `KillProcess`); unknown names pass through.
#[must_use]
pub fn process_names(query: &str) -> Vec<String> {
    find_known_app(query).map_or_else(|| vec![crate::procs::norm(query)], |a| a.processes.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_load() {
        assert!(known_apps().len() >= 80);
        assert!(sites().len() >= 60);
        assert!(folders().len() >= 15);
    }

    #[test]
    fn spoken_app_names_resolve() {
        assert_eq!(find_known_app("блокнот").unwrap().key, "notepad");
        assert_eq!(find_known_app("Блокнота").unwrap().key, "notepad");
        assert_eq!(find_known_app("калькулятор").unwrap().key, "calculator");
        assert_eq!(find_known_app("google chrome").unwrap().key, "chrome");
        assert_eq!(find_known_app("телеграм").unwrap().key, "telegram");
        assert!(find_known_app("абракадабра").is_none());
    }

    #[test]
    fn launch_targets() {
        let installed = vec![InstalledApp { name: "Telegram Desktop".into(), target: "shell:AppsFolder\\tg".into() }];
        assert_eq!(launch_target("notepad", &installed), "notepad.exe");
        assert_eq!(launch_target("блокнот", &installed), "notepad.exe");
        assert_eq!(launch_target("телеграм", &installed), "shell:AppsFolder\\tg");
        assert_eq!(launch_target("ya.ru", &installed), "https://ya.ru");
        assert_eq!(launch_target("ms-settings:display", &installed), "ms-settings:display");
        assert_eq!(launch_target("загрузки", &installed), "shell:Downloads");
        assert_eq!(launch_target("someunknowntool", &installed), "someunknowntool");
    }

    #[test]
    fn urls_and_uris() {
        assert!(looks_like_url("github.com/orkestr"));
        assert!(looks_like_url("https://x.y"));
        assert!(!looks_like_url("notepad.exe"));
        assert!(!looks_like_url("hello world"));
        assert!(is_uri("ms-settings:display"));
        assert!(!is_uri("C:\\x"));
        assert_eq!(site_url("ютуб").as_deref(), Some("https://www.youtube.com"));
        assert_eq!(search_url("rust lang", None), "https://www.google.com/search?q=rust+lang");
        assert!(search_url("котики", Some("youtube")).starts_with("https://www.youtube.com/results?search_query=%D0"));
    }

    #[test]
    fn settings_pages() {
        assert_eq!(settings_uri("display"), "ms-settings:display");
        assert_eq!(settings_uri("звука"), "ms-settings:sound");
        assert_eq!(settings_uri("bluetooth"), "ms-settings:bluetooth");
        assert_eq!(settings_uri(""), "ms-settings:");
    }

    #[test]
    fn windows_match_apps() {
        let w = WindowInfo {
            handle: "1".into(),
            title: "Untitled - Notepad".into(),
            process: "Notepad.exe".into(),
            pid: 1,
            active: false,
            minimized: false,
        };
        assert!(window_matches("блокнот", &w));
        assert!(window_matches("notepad", &w));
        assert!(!window_matches("chrome", &w));
    }

    #[test]
    fn env_and_paths() {
        std::env::set_var("ORKESTR_TEST_X", "abc");
        assert_eq!(expand_env("%ORKESTR_TEST_X%/y"), "abc/y");
        assert_eq!(expand_env("100%"), "100%");
        assert!(file_path("notes.txt").ends_with("notes.txt"));
    }
}

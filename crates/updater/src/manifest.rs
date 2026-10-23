//! Release manifests in the format of `tauri-plugin-updater` (`latest.json`).
//!
//! Static form (what `scripts/release/build-installer.ps1` emits and the
//! future update server serves):
//! ```json
//! { "version": "0.1.1", "notes": "…", "pub_date": "2026-10-23T12:00:00Z",
//!   "platforms": { "windows-x86_64": { "url": "https://…/Orkestr_0.1.1_x64-setup.exe",
//!                                       "signature": "<base64 minisign>" } } }
//! ```
//! The dynamic form (`{version, url, signature, notes, pub_date}` for the
//! requested target only) is accepted too. Orkestr extras, ignored by the
//! plugin: `notes_i18n` (`{"ru": "…"}`) and `channel`.

use std::collections::BTreeMap;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::{UpdateError, UpdateResult};
use crate::settings::Channel;

/// One platform entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    /// Download URL — absolute (`https://`, `file://`) or, in a local
    /// `latest.json`, relative to the manifest's folder.
    pub url: String,
    /// Base64 minisign signature (contents of the `.sig` file).
    pub signature: String,
}

/// A parsed release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Release version (semver; a leading `v` is tolerated).
    pub version: String,
    /// Release notes (Markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Localized release notes by language (`ru`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notes_i18n: BTreeMap<String, String>,
    /// RFC 3339 publication date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// Channel the release belongs to (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Builds by target key (`windows-x86_64`, `windows-x86_64-nsis`, …).
    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformEntry>,
}

#[derive(Deserialize)]
struct DynamicManifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    notes_i18n: BTreeMap<String, String>,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    url: String,
    signature: String,
}

/// The platform this build updates: `{{target}}`, `{{arch}}` and the
/// installer kind, giving the manifest keys to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `windows`, `darwin`, `linux`.
    pub os: String,
    /// `x86_64`, `aarch64`, …
    pub arch: String,
    /// `nsis`, `msi`, … (bundle type of the running install), if known.
    pub installer: Option<String>,
}

impl Target {
    /// The platform this binary was built for, with the given installer kind.
    #[must_use]
    pub fn current(installer: Option<&str>) -> Self {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        Self { os: os.to_owned(), arch: std::env::consts::ARCH.to_owned(), installer: installer.map(str::to_owned) }
    }

    /// Manifest keys in lookup order (`os-arch-installer`, then `os-arch`) —
    /// the same order `tauri-plugin-updater` uses.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        let base = format!("{}-{}", self.os, self.arch);
        let mut keys = Vec::new();
        if let Some(i) = &self.installer {
            keys.push(format!("{base}-{i}"));
        }
        keys.push(base);
        keys
    }
}

impl Manifest {
    /// Parses a static or dynamic manifest.
    pub fn parse(bytes: &[u8]) -> UpdateResult<Self> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| UpdateError::BadManifest(e.to_string()))?;
        let manifest = if value.get("platforms").is_some() {
            serde_json::from_value::<Self>(value).map_err(|e| UpdateError::BadManifest(e.to_string()))?
        } else {
            let d = serde_json::from_value::<DynamicManifest>(value)
                .map_err(|e| UpdateError::BadManifest(e.to_string()))?;
            let mut platforms = BTreeMap::new();
            // The dynamic form answers for the requested target; store it under a wildcard key.
            platforms.insert(DYNAMIC_KEY.to_owned(), PlatformEntry { url: d.url, signature: d.signature });
            Self {
                version: d.version,
                notes: d.notes,
                notes_i18n: d.notes_i18n,
                pub_date: d.pub_date,
                channel: d.channel,
                platforms,
            }
        };
        manifest.semver()?;
        Ok(manifest)
    }

    /// The release version as semver.
    pub fn semver(&self) -> UpdateResult<Version> {
        parse_version(&self.version).map_err(|e| UpdateError::BadManifest(format!("version {:?}: {e}", self.version)))
    }

    /// The entry for `target` (first matching key) as `(key, entry)`.
    pub fn select(&self, target: &Target) -> UpdateResult<(String, &PlatformEntry)> {
        if let Some(entry) = self.platforms.get(DYNAMIC_KEY) {
            return Ok((target.keys().pop().unwrap_or_default(), entry));
        }
        let keys = target.keys();
        for key in &keys {
            if let Some(entry) = self.platforms.get(key) {
                return Ok((key.clone(), entry));
            }
        }
        Err(UpdateError::NoPlatform(keys.join(", ")))
    }

    /// Notes in the UI language, falling back to the default notes.
    #[must_use]
    pub fn notes_for(&self, language: &str) -> Option<&str> {
        let primary = language.split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase();
        self.notes_i18n.get(&primary).map(String::as_str).or(self.notes.as_deref())
    }
}

/// Key used internally for the single entry of a dynamic manifest.
const DYNAMIC_KEY: &str = "*";

/// Parses `1.2.3` / `v1.2.3`.
pub fn parse_version(s: &str) -> Result<Version, semver::Error> {
    Version::parse(s.trim().trim_start_matches('v'))
}

/// Whether `candidate` is an update for `current` on `channel`: strictly
/// newer, and on the stable channel never a pre-release.
#[must_use]
pub fn is_update(current: &Version, candidate: &Version, channel: Channel) -> bool {
    if channel == Channel::Stable && !candidate.pre.is_empty() {
        return false;
    }
    candidate > current
}

/// Expands the endpoint variables.
#[must_use]
pub fn expand_endpoint(template: &str, target: &Target, current: &Version, channel: Channel) -> String {
    let mut url = template
        .replace("{{target}}", &target.os)
        .replace("{{arch}}", &target.arch)
        .replace("{{current_version}}", &current.to_string())
        .replace("{{bundle_type}}", target.installer.as_deref().unwrap_or("nsis"))
        .replace("{{channel}}", channel.as_str());
    if !template.contains("{{channel}}") && channel != Channel::Stable {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str("channel=");
        url.push_str(channel.as_str());
    }
    url
}

/// Resolves a platform URL of a **local** manifest: absolute URLs stay as
/// they are; relative ones (and bare file names) are taken relative to the
/// manifest's folder and returned as a path.
#[must_use]
pub fn resolve_local(url: &str, manifest_dir: &Path) -> Location {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if parsed.scheme() == "file" {
            if let Ok(p) = parsed.to_file_path() {
                return Location::File(p);
            }
        }
        if parsed.scheme().len() > 1 {
            // `C:\…` parses as scheme `c`; real schemes are longer.
            return Location::Url(url.to_owned());
        }
    }
    let p = Path::new(url);
    Location::File(if p.is_absolute() { p.to_path_buf() } else { manifest_dir.join(p) })
}

/// Where an installer comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Location {
    /// Downloaded over HTTP(S).
    Url(String),
    /// A local file (manual update).
    File(std::path::PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> Target {
        Target { os: "windows".into(), arch: "x86_64".into(), installer: Some("nsis".into()) }
    }

    #[test]
    fn parses_static_and_selects_the_most_specific_key() {
        let m = Manifest::parse(
            br#"{"version":"v0.2.0","notes":"n","platforms":{
                "windows-x86_64":{"url":"https://x/a.exe","signature":"s1"},
                "windows-x86_64-nsis":{"url":"https://x/b.exe","signature":"s2"},
                "darwin-aarch64":{"url":"https://x/c","signature":"s3"}}}"#,
        )
        .unwrap();
        assert_eq!(m.semver().unwrap(), Version::new(0, 2, 0));
        let (key, e) = m.select(&win()).unwrap();
        assert_eq!(key, "windows-x86_64-nsis");
        assert_eq!(e.signature, "s2");
        let msi = Target { installer: Some("msi".into()), ..win() };
        assert_eq!(m.select(&msi).unwrap().0, "windows-x86_64");
        let arm = Target { arch: "aarch64".into(), ..win() };
        assert!(matches!(m.select(&arm), Err(UpdateError::NoPlatform(_))));
    }

    #[test]
    fn parses_dynamic_form_and_rejects_garbage() {
        let m = Manifest::parse(br#"{"version":"1.0.0","url":"https://x/a.exe","signature":"s"}"#).unwrap();
        assert_eq!(m.select(&win()).unwrap().1.url, "https://x/a.exe");
        assert!(matches!(Manifest::parse(b"<html>"), Err(UpdateError::BadManifest(_))));
        assert!(matches!(Manifest::parse(br#"{"version":"latest","platforms":{}}"#), Err(UpdateError::BadManifest(_))));
    }

    #[test]
    fn channels_filter_prereleases() {
        let cur = Version::new(0, 1, 0);
        let beta = Version::parse("0.2.0-beta.1").unwrap();
        assert!(!is_update(&cur, &beta, Channel::Stable));
        assert!(is_update(&cur, &beta, Channel::Beta));
        assert!(!is_update(&cur, &cur, Channel::Beta));
        assert!(!is_update(&Version::new(0, 2, 0), &Version::new(0, 1, 9), Channel::Stable));
    }

    #[test]
    fn expands_endpoint_variables() {
        let v = Version::new(0, 1, 0);
        assert_eq!(
            expand_endpoint(crate::settings::DEFAULT_ENDPOINT, &win(), &v, Channel::Stable),
            "https://updates.orkestr.dev/windows/x86_64/0.1.0"
        );
        assert_eq!(
            expand_endpoint("https://u/{{channel}}/latest.json", &win(), &v, Channel::Beta),
            "https://u/beta/latest.json"
        );
        assert_eq!(
            expand_endpoint("https://u/l.json?a=1", &win(), &v, Channel::Beta),
            "https://u/l.json?a=1&channel=beta"
        );
    }

    #[test]
    fn localized_notes_fall_back() {
        let mut m = Manifest::parse(br#"{"version":"1.0.0","notes":"en","platforms":{}}"#).unwrap();
        assert_eq!(m.notes_for("ru"), Some("en"));
        m.notes_i18n.insert("ru".into(), "ру".into());
        assert_eq!(m.notes_for("ru-RU"), Some("ру"));
        assert_eq!(m.notes_for("en"), Some("en"));
    }

    #[test]
    fn resolves_local_urls() {
        let dir = Path::new("C:/rel");
        assert_eq!(resolve_local("Orkestr.exe", dir), Location::File(dir.join("Orkestr.exe")));
        assert_eq!(resolve_local("https://x/a.exe", dir), Location::Url("https://x/a.exe".into()));
        #[cfg(windows)]
        assert_eq!(resolve_local("C:\\x\\a.exe", dir), Location::File("C:\\x\\a.exe".into()));
    }
}

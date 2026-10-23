//! Checking for updates (remote endpoint or a local file) and downloading /
//! verifying the installer.
//!
//! Every installer is verified against the updater public key **before** it
//! is offered for installation; `tauri-plugin-updater` verifies it again
//! right before launching it. The only exception is a local installer the
//! user picked that has no `.sig` next to it: it is reported as unsigned and
//! the UI requires an explicit confirmation (it is equivalent to the user
//! double-clicking the file themselves).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

use crate::error::{UpdateError, UpdateResult};
use crate::manifest::{expand_endpoint, is_update, parse_version, resolve_local, Location, Manifest, Target};
use crate::settings::{validate_endpoint, Channel};
use crate::signature;

/// Refuse downloads larger than this (an installer is ~10–40 MB).
pub const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// Where to look for an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateSource {
    /// The configured endpoint template.
    Remote {
        /// Endpoint template (variables expanded by the checker).
        endpoint: String,
    },
    /// A local `latest.json` or installer (`.exe` / `.msi`) picked by the user.
    Local {
        /// Absolute path.
        path: PathBuf,
    },
}

/// How an update was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    /// The remote endpoint.
    Remote,
    /// A local `latest.json`.
    LocalManifest,
    /// A local installer file.
    LocalInstaller,
}

/// An available update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// Version on offer.
    pub version: String,
    /// Version running now.
    pub current_version: String,
    /// Release notes (Markdown).
    pub notes: Option<String>,
    /// Localized release notes.
    pub notes_i18n: BTreeMap<String, String>,
    /// Publication date (RFC 3339) if announced.
    pub pub_date: Option<String>,
    /// Manifest key used (`windows-x86_64-nsis`, …).
    pub platform_key: String,
    /// Where the installer is.
    pub location: Location,
    /// Base64 signature; `None` only for an unsigned local installer.
    pub signature: Option<String>,
    /// How it was found.
    pub source: SourceKind,
    /// Installer file name.
    pub file_name: String,
    /// Size in bytes when known.
    pub size: Option<u64>,
    /// Whether it is a pre-release.
    pub prerelease: bool,
    /// Normalized manifest for `tauri-plugin-updater` (`Updater::restore_update`):
    /// absolute `https://` / `file://` URL, `null` for unsigned installers.
    pub manifest: serde_json::Value,
}

impl UpdateInfo {
    /// The offered version as semver.
    pub fn semver(&self) -> UpdateResult<Version> {
        parse_version(&self.version).map_err(|e| UpdateError::BadManifest(e.to_string()))
    }
}

/// Result of a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Nothing newer (`latest` = what the source announced, if anything).
    UpToDate {
        /// Version the source offers.
        latest: Option<String>,
    },
    /// An update is available.
    Available(Box<UpdateInfo>),
}

/// Checks and downloads updates for one installed build.
#[derive(Debug, Clone)]
pub struct Checker {
    client: reqwest::Client,
    pubkey: String,
    target: Target,
    current: Version,
    channel: Channel,
}

impl Checker {
    /// `pubkey` = base64 updater public key (as in `tauri.conf.json`).
    pub fn new(pubkey: impl Into<String>, target: Target, current: Version, channel: Channel) -> UpdateResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30 * 60))
            .user_agent(format!("Orkestr/{current} ({}; {})", target.os, target.arch))
            .build()
            .map_err(|e| UpdateError::Io(e.to_string()))?;
        Ok(Self { client, pubkey: pubkey.into(), target, current, channel })
    }

    /// The running version.
    #[must_use]
    pub fn current(&self) -> &Version {
        &self.current
    }

    /// Checks one source.
    pub async fn check(&self, source: &UpdateSource) -> UpdateResult<CheckOutcome> {
        match source {
            UpdateSource::Remote { endpoint } => self.check_remote(endpoint).await,
            UpdateSource::Local { path } => {
                let path = path.clone();
                let this = self.clone();
                tokio::task::spawn_blocking(move || this.check_local(&path))
                    .await
                    .map_err(|e| UpdateError::Io(e.to_string()))?
            }
        }
    }

    async fn check_remote(&self, template: &str) -> UpdateResult<CheckOutcome> {
        validate_endpoint(template)?;
        let url = expand_endpoint(template, &self.target, &self.current, self.channel);
        tracing::info!(%url, "checking for updates");
        let resp = self
            .client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| UpdateError::from_http(&e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(CheckOutcome::UpToDate { latest: None });
        }
        if !status.is_success() {
            return Err(UpdateError::Server(status.as_u16()));
        }
        let bytes = resp.bytes().await.map_err(|e| UpdateError::from_http(&e))?;
        let manifest = Manifest::parse(&bytes)?;
        let base = reqwest::Url::parse(&url).map_err(|e| UpdateError::InvalidEndpoint(e.to_string()))?;
        self.offer(&manifest, SourceKind::Remote, |entry_url| {
            // Relative URLs in a served manifest resolve against the manifest URL.
            base.join(entry_url)
                .map(|u| Location::Url(u.to_string()))
                .map_err(|e| UpdateError::BadManifest(e.to_string()))
        })
    }

    fn check_local(&self, path: &Path) -> UpdateResult<CheckOutcome> {
        if path.is_dir() {
            // A release folder (e.g. a network share): its latest.json, else the newest installer.
            let manifest = path.join("latest.json");
            if manifest.is_file() {
                return self.check_local(&manifest);
            }
            let newest = std::fs::read_dir(path)?
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let lower = name.to_ascii_lowercase();
                    let installer =
                        lower.starts_with("orkestr") && (lower.ends_with(".exe") || lower.ends_with(".msi"));
                    installer.then(|| version_from_file_name(&name).map(|v| (v, e.path()))).flatten()
                })
                .max_by(|a, b| a.0.cmp(&b.0));
            return match newest {
                Some((_, p)) => self.check_local(&p),
                None => Err(UpdateError::BadManifest(format!(
                    "{}: no latest.json or Orkestr installer in this folder",
                    path.display()
                ))),
            };
        }
        if !path.is_file() {
            return Err(UpdateError::Io(format!("{} does not exist", path.display())));
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        if ext == "json" {
            let bytes = std::fs::read(path)?;
            let manifest = Manifest::parse(&bytes)?;
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            return self.offer(&manifest, SourceKind::LocalManifest, |u| Ok(resolve_local(u, &dir)));
        }
        if ext != "exe" && ext != "msi" {
            return Err(UpdateError::BadManifest(format!(
                "{}: pick an Orkestr installer (.exe, .msi) or a latest.json",
                path.display()
            )));
        }
        self.check_local_installer(path)
    }

    fn check_local_installer(&self, path: &Path) -> UpdateResult<CheckOutcome> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        let sig_path = PathBuf::from(format!("{}.sig", path.display()));
        let signature = std::fs::read_to_string(&sig_path).ok().map(|s| s.trim().to_owned());
        let version = match &signature {
            Some(sig) => signed_version_of(sig)?,
            None => version_from_file_name(&file_name)
                .ok_or_else(|| UpdateError::BadManifest(format!("cannot tell the version of {file_name}")))?,
        };
        let size = std::fs::metadata(path).ok().map(|m| m.len());
        if let Some(sig) = &signature {
            let data = std::fs::read(path)?;
            signature::verify(&data, sig, &self.pubkey, &version)?;
        }
        if !is_update(&self.current, &version, Channel::Beta) {
            return Ok(CheckOutcome::UpToDate { latest: Some(version.to_string()) });
        }
        let key = self.target.keys().into_iter().next().unwrap_or_default();
        let manifest = match &signature {
            Some(sig) => normalized_manifest(&version, None, None, &key, &file_url(path)?, sig),
            None => serde_json::Value::Null,
        };
        Ok(CheckOutcome::Available(Box::new(UpdateInfo {
            version: version.to_string(),
            current_version: self.current.to_string(),
            notes: None,
            notes_i18n: BTreeMap::new(),
            pub_date: None,
            platform_key: key,
            location: Location::File(path.to_path_buf()),
            signature,
            source: SourceKind::LocalInstaller,
            file_name,
            size,
            prerelease: !version.pre.is_empty(),
            manifest,
        })))
    }

    fn offer(
        &self,
        manifest: &Manifest,
        source: SourceKind,
        resolve: impl Fn(&str) -> UpdateResult<Location>,
    ) -> UpdateResult<CheckOutcome> {
        let version = manifest.semver()?;
        // A manually picked manifest may offer a beta even on the stable channel: the user chose it.
        let channel = if source == SourceKind::Remote { self.channel } else { Channel::Beta };
        if !is_update(&self.current, &version, channel) {
            return Ok(CheckOutcome::UpToDate { latest: Some(version.to_string()) });
        }
        let (key, entry) = manifest.select(&self.target)?;
        if entry.signature.trim().is_empty() {
            return Err(UpdateError::BadSignature("the manifest has no signature for this platform".into()));
        }
        let location = resolve(&entry.url)?;
        let (absolute, file_name, size) = match &location {
            Location::Url(u) => {
                let name = reqwest::Url::parse(u)
                    .ok()
                    .and_then(|u| u.path_segments().and_then(|mut s| s.next_back().map(str::to_owned)))
                    .filter(|n| !n.is_empty());
                (u.clone(), name, None)
            }
            Location::File(p) => {
                let name = p.file_name().and_then(|n| n.to_str()).map(str::to_owned);
                (file_url(p)?, name, std::fs::metadata(p).ok().map(|m| m.len()))
            }
        };
        let file_name = sanitize_file_name(file_name.as_deref().unwrap_or_default(), &version);
        Ok(CheckOutcome::Available(Box::new(UpdateInfo {
            version: version.to_string(),
            current_version: self.current.to_string(),
            notes: manifest.notes.clone(),
            notes_i18n: manifest.notes_i18n.clone(),
            pub_date: manifest.pub_date.clone(),
            platform_key: key.clone(),
            location,
            signature: Some(entry.signature.trim().to_owned()),
            source,
            file_name,
            size,
            prerelease: !version.pre.is_empty(),
            manifest: normalized_manifest(
                &version,
                manifest.notes.as_deref(),
                manifest.pub_date.as_deref(),
                &key,
                &absolute,
                entry.signature.trim(),
            ),
        })))
    }

    /// Fetches the installer (remote: into `dest_dir`; local: in place) and
    /// verifies its signature. `progress(downloaded, total)`.
    /// Unsigned local installers are returned only with `allow_unsigned`.
    pub async fn fetch(
        &self,
        info: &UpdateInfo,
        dest_dir: &Path,
        allow_unsigned: bool,
        cancel: &CancellationToken,
        mut progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> UpdateResult<PathBuf> {
        let version = info.semver()?;
        let path = match &info.location {
            Location::File(p) => {
                let len = std::fs::metadata(p)?.len();
                progress(len, Some(len));
                p.clone()
            }
            Location::Url(url) => self.download(url, dest_dir, &info.file_name, cancel, &mut progress).await?,
        };
        match &info.signature {
            Some(sig) => {
                let (path2, sig, pk) = (path.clone(), sig.clone(), self.pubkey.clone());
                tokio::task::spawn_blocking(move || -> UpdateResult<()> {
                    let data = std::fs::read(&path2)?;
                    signature::verify(&data, &sig, &pk, &version).map(|_| ())
                })
                .await
                .map_err(|e| UpdateError::Io(e.to_string()))?
                .inspect_err(|_| {
                    if matches!(info.location, Location::Url(_)) {
                        // Never keep a tampered download around.
                        let _ = std::fs::remove_file(&path);
                    }
                })?;
            }
            None if allow_unsigned => {}
            None => return Err(UpdateError::Unsigned),
        }
        Ok(path)
    }

    async fn download(
        &self,
        url: &str,
        dest_dir: &Path,
        file_name: &str,
        cancel: &CancellationToken,
        progress: &mut (impl FnMut(u64, Option<u64>) + Send),
    ) -> UpdateResult<PathBuf> {
        tokio::fs::create_dir_all(dest_dir).await?;
        let dest = dest_dir.join(file_name);
        let part = dest_dir.join(format!("{file_name}.part"));
        let resp = tokio::select! {
            () = cancel.cancelled() => return Err(UpdateError::Cancelled),
            r = self.client.get(url).send() => r.map_err(|e| UpdateError::from_http(&e))?,
        };
        if !resp.status().is_success() {
            return Err(UpdateError::Server(resp.status().as_u16()));
        }
        let total = resp.content_length();
        if total.is_some_and(|t| t > MAX_DOWNLOAD_BYTES) {
            return Err(UpdateError::BadManifest("the installer is unreasonably large".into()));
        }
        let mut file = tokio::fs::File::create(&part).await?;
        let mut stream = resp.bytes_stream();
        let mut done: u64 = 0;
        progress(0, total);
        let result: UpdateResult<()> = async {
            loop {
                let chunk = tokio::select! {
                    () = cancel.cancelled() => return Err(UpdateError::Cancelled),
                    c = stream.next() => c,
                };
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(|e| UpdateError::from_http(&e))?;
                done += chunk.len() as u64;
                if done > MAX_DOWNLOAD_BYTES {
                    return Err(UpdateError::BadManifest("the installer is unreasonably large".into()));
                }
                file.write_all(&chunk).await?;
                progress(done, total);
            }
            file.flush().await?;
            Ok(())
        }
        .await;
        drop(file);
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        if total.is_some_and(|t| t != done) {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(UpdateError::Offline(format!("download interrupted ({done} of {} bytes)", total.unwrap_or(0))));
        }
        tokio::fs::rename(&part, &dest).await?;
        Ok(dest)
    }
}

/// The version recorded in a (not yet verified) signature's trusted comment.
fn signed_version_of(signature_b64: &str) -> UpdateResult<Version> {
    use base64::Engine as _;
    let text = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| UpdateError::BadSignature("the .sig file is not a valid signature".into()))?;
    let sig = minisign_verify::Signature::decode(&text)
        .map_err(|e| UpdateError::BadSignature(format!("invalid signature: {e}")))?;
    let v = signature::signed_version(sig.trusted_comment())
        .ok_or_else(|| UpdateError::BadSignature("the signature does not record the release version".into()))?;
    parse_version(v).map_err(|e| UpdateError::BadSignature(format!("signed version {v:?}: {e}")))
}

/// `Orkestr_0.1.1_x64-setup.exe` / `Orkestr_0.2.0-beta.1_x64_en-US.msi` → version.
#[must_use]
pub fn version_from_file_name(name: &str) -> Option<Version> {
    name.split('_').find_map(|part| parse_version(part).ok())
}

fn sanitize_file_name(name: &str, version: &Version) -> String {
    let clean: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    if clean.is_empty() || clean.starts_with('.') {
        format!("Orkestr_{version}_setup.exe")
    } else {
        clean
    }
}

fn file_url(path: &Path) -> UpdateResult<String> {
    reqwest::Url::from_file_path(path)
        .map(|u| u.to_string())
        .map_err(|()| UpdateError::Io(format!("{} is not an absolute path", path.display())))
}

/// The manifest shape `tauri-plugin-updater` parses (`RemoteRelease`).
fn normalized_manifest(
    version: &Version,
    notes: Option<&str>,
    pub_date: Option<&str>,
    key: &str,
    url: &str,
    signature: &str,
) -> serde_json::Value {
    let mut m = serde_json::json!({
        "version": version.to_string(),
        "platforms": { key: { "url": url, "signature": signature } },
    });
    if let Some(n) = notes {
        m["notes"] = n.into();
    }
    // The plugin rejects dates that are not RFC 3339; drop those.
    if let Some(d) = pub_date.filter(|d| chrono::DateTime::parse_from_rfc3339(d).is_ok()) {
        m["pub_date"] = d.into();
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_from_installer_names() {
        assert_eq!(version_from_file_name("Orkestr_0.1.1_x64-setup.exe"), Some(Version::new(0, 1, 1)));
        assert_eq!(
            version_from_file_name("Orkestr_0.2.0-beta.1_x64_en-US.msi"),
            Some(Version::parse("0.2.0-beta.1").unwrap())
        );
        assert_eq!(version_from_file_name("setup.exe"), None);
    }

    #[test]
    fn sanitizes_download_names() {
        let v = Version::new(1, 0, 0);
        assert_eq!(sanitize_file_name("Orkestr_1.0.0_x64-setup.exe", &v), "Orkestr_1.0.0_x64-setup.exe");
        assert_eq!(sanitize_file_name("..\\..\\evil.exe", &v), "Orkestr_1.0.0_setup.exe");
        assert_eq!(sanitize_file_name("", &v), "Orkestr_1.0.0_setup.exe");
    }
}

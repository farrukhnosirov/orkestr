//! VSCodium release metadata (GitHub Releases API) and checksum files.
//!
//! Source of truth: <https://github.com/VSCodium/vscodium/releases> (MIT).
//! Download URLs are pinned to that repository's release-download prefix, so
//! a tampered API response cannot redirect the install to another host
//! (GitHub's own redirect to its asset CDN is followed by the HTTP client).

use std::cmp::Ordering;

use serde::Deserialize;

use crate::error::{HostError, HostResult};
use crate::target::ServerTarget;

/// GitHub API base of the VSCodium releases.
pub const RELEASES_API: &str = "https://api.github.com/repos/VSCodium/vscodium/releases";
/// Every asset URL must start with this prefix.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/VSCodium/vscodium/releases/download/";
/// Human release page.
pub const RELEASES_PAGE: &str = "https://github.com/VSCodium/vscodium/releases";
/// License of VSCodium builds.
pub const LICENSE: &str = "MIT";
/// Extension gallery configured in VSCodium's `product.json`.
pub const EXTENSION_REGISTRY: &str = "https://open-vsx.org";
/// Refuse absurd archive sizes (the real ones are ~130 MB).
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// The release asset for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// Version (tag).
    pub version: String,
    /// Archive file name.
    pub asset_name: String,
    /// Archive size.
    pub size_bytes: u64,
    /// Archive URL.
    pub download_url: String,
    /// `.sha256` URL.
    pub sha256_url: String,
    /// Publish time.
    pub published_at: Option<String>,
    /// Release page.
    pub release_page: String,
}

/// A version string we accept: digits and dots only (it becomes a directory name).
#[must_use]
pub fn is_valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 40
        && v.chars().all(|c| c.is_ascii_digit() || c == '.')
        && !v.starts_with('.')
        && !v.ends_with('.')
        && !v.contains("..")
}

/// Finds the target's archive + checksum in a GitHub release JSON document.
pub fn pick_asset(release_json: &str, target: &ServerTarget) -> HostResult<ReleaseAsset> {
    let release: GhRelease = serde_json::from_str(release_json)
        .map_err(|e| HostError::Release(format!("unexpected release metadata: {e}")))?;
    if release.draft {
        return Err(HostError::Release("the release is a draft".into()));
    }
    let version = release.tag_name.trim().to_owned();
    if !is_valid_version(&version) {
        return Err(HostError::Release(format!("unexpected release tag `{version}`")));
    }
    let name = target.asset_name(&version);
    let sha_name = format!("{name}.sha256");
    let archive = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| HostError::Release(format!("release {version} has no `{name}` asset for this platform")))?;
    let sha = release
        .assets
        .iter()
        .find(|a| a.name == sha_name)
        .ok_or_else(|| HostError::Release(format!("release {version} publishes no checksum for `{name}`")))?;
    for url in [&archive.browser_download_url, &sha.browser_download_url] {
        if !url.starts_with(DOWNLOAD_PREFIX) {
            return Err(HostError::Release(format!("refusing a download URL outside the VSCodium repository: {url}")));
        }
    }
    if archive.size == 0 || archive.size > MAX_ARCHIVE_BYTES {
        return Err(HostError::Release(format!("unexpected archive size {} bytes", archive.size)));
    }
    Ok(ReleaseAsset {
        version,
        asset_name: name,
        size_bytes: archive.size,
        download_url: archive.browser_download_url.clone(),
        sha256_url: sha.browser_download_url.clone(),
        published_at: release.published_at,
        release_page: release.html_url.unwrap_or_else(|| RELEASES_PAGE.to_owned()),
    })
}

/// Parses a `sha256sum`-style file (`<hex>  <name>`, `<hex> *<name>` or just
/// `<hex>`) and returns the lowercase hex digest. When the file names a file,
/// it must be `asset_name`.
pub fn parse_sha256_file(text: &str, asset_name: &str) -> HostResult<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| HostError::Checksum("the checksum file is empty".into()))?;
    let mut parts = line.split_whitespace();
    let digest = parts.next().unwrap_or_default().to_ascii_lowercase();
    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(HostError::Checksum("the checksum file does not contain a sha256 digest".into()));
    }
    if let Some(name) = parts.next() {
        let name = name.trim_start_matches('*');
        if name != asset_name {
            return Err(HostError::Checksum(format!("the checksum file is for `{name}`, not `{asset_name}`")));
        }
    }
    Ok(digest)
}

/// Compares the computed digest with the published one.
pub fn verify_digest(actual_hex: &str, expected_hex: &str) -> HostResult<()> {
    if actual_hex.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(HostError::Checksum(format!(
            "sha256 mismatch: expected {expected_hex}, got {actual_hex} — the download was corrupted or tampered with"
        )))
    }
}

/// Numeric, segment-wise version comparison (`1.135.06055` > `1.99.1`).
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let seg = |v: &str| -> Vec<u64> { v.split('.').map(|s| s.parse::<u64>().unwrap_or(0)).collect() };
    let (a, b) = (seg(a), seg(b));
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        match x.cmp(&y) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::select_target;
    use orkestr_domain::{Arch, OsKind};

    fn release_json(tag: &str, prefix: &str) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/VSCodium/vscodium/releases/tag/{tag}"),
            "published_at": "2026-09-30T10:00:00Z",
            "assets": [
                {"name": format!("vscodium-reh-web-linux-x64-{tag}.tar.gz"), "size": 136, "browser_download_url": format!("{prefix}{tag}/vscodium-reh-web-linux-x64-{tag}.tar.gz")},
                {"name": format!("vscodium-reh-web-win32-x64-{tag}.tar.gz"), "size": 129_574_089_u64, "browser_download_url": format!("{prefix}{tag}/vscodium-reh-web-win32-x64-{tag}.tar.gz")},
                {"name": format!("vscodium-reh-web-win32-x64-{tag}.tar.gz.sha256"), "size": 112, "browser_download_url": format!("{prefix}{tag}/vscodium-reh-web-win32-x64-{tag}.tar.gz.sha256")},
                {"name": format!("vscodium-reh-web-win32-x64-{tag}.tar.gz.sha1"), "size": 88, "browser_download_url": format!("{prefix}{tag}/x.sha1")}
            ]
        })
        .to_string()
    }

    #[test]
    fn picks_the_archive_and_checksum_of_the_target() {
        let win = select_target(OsKind::Windows, Arch::X64).unwrap();
        let a = pick_asset(&release_json("1.135.06055", DOWNLOAD_PREFIX), &win).unwrap();
        assert_eq!(a.version, "1.135.06055");
        assert_eq!(a.asset_name, "vscodium-reh-web-win32-x64-1.135.06055.tar.gz");
        assert_eq!(a.size_bytes, 129_574_089);
        assert!(a.sha256_url.ends_with(".tar.gz.sha256"));
        assert!(a.download_url.starts_with(DOWNLOAD_PREFIX));
    }

    #[test]
    fn missing_checksum_or_target_is_an_error() {
        let linux = select_target(OsKind::Linux, Arch::X64).unwrap();
        let e = pick_asset(&release_json("1.2.3", DOWNLOAD_PREFIX), &linux).unwrap_err();
        assert!(e.to_string().contains("checksum"), "{e}");
        let mac = select_target(OsKind::MacOs, Arch::Arm64).unwrap();
        let e = pick_asset(&release_json("1.2.3", DOWNLOAD_PREFIX), &mac).unwrap_err();
        assert!(e.to_string().contains("no `vscodium-reh-web-darwin-arm64"), "{e}");
    }

    #[test]
    fn foreign_download_hosts_and_bad_tags_are_refused() {
        let win = select_target(OsKind::Windows, Arch::X64).unwrap();
        let e = pick_asset(&release_json("1.2.3", "https://evil.example/download/"), &win).unwrap_err();
        assert!(e.to_string().contains("outside the VSCodium repository"), "{e}");
        let e = pick_asset(&release_json("../../x", DOWNLOAD_PREFIX), &win).unwrap_err();
        assert!(e.to_string().contains("unexpected release tag"), "{e}");
        assert!(pick_asset("{not json", &win).is_err());
    }

    #[test]
    fn version_validation() {
        assert!(is_valid_version("1.135.06055"));
        for bad in ["", "..", "1..2", ".1", "1.", "1.2-rc", "../x", "1/2", "a"] {
            assert!(!is_valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn parses_sha256_files() {
        let hex = "579121d3043b94fbd054aee3f1155b53b00dc2786995604a685fda9ba7e2b560";
        let name = "vscodium-reh-web-win32-x64-1.135.06055.tar.gz";
        assert_eq!(parse_sha256_file(&format!("{hex}  {name}\n"), name).unwrap(), hex);
        assert_eq!(parse_sha256_file(&format!("{hex} *{name}"), name).unwrap(), hex);
        assert_eq!(parse_sha256_file(&format!("\n{}\n", hex.to_uppercase()), name).unwrap(), hex);
        assert!(parse_sha256_file(&format!("{hex}  other.tar.gz"), name).is_err());
        assert!(parse_sha256_file("", name).is_err());
        assert!(parse_sha256_file("deadbeef  x", name).is_err());
        assert!(parse_sha256_file(&format!("{}  {name}", "z".repeat(64)), name).is_err());
    }

    #[test]
    fn digest_verification() {
        assert!(verify_digest("ABCDEF", "abcdef").is_ok());
        let e = verify_digest("00", "11").unwrap_err();
        assert!(e.to_string().contains("mismatch"));
    }

    #[test]
    fn version_ordering() {
        assert_eq!(compare_versions("1.135.06055", "1.99.1"), Ordering::Greater);
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3", "1.10.0"), Ordering::Less);
    }
}

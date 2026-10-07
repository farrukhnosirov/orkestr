//! Open VSX registry client (`https://open-vsx.org/api`).
//!
//! The Microsoft Visual Studio Marketplace is never used: its terms only
//! allow Microsoft products to download from it. Open VSX is the
//! vendor-neutral registry run by the Eclipse Foundation; every `.vsix` it
//! serves has a published SHA-256 checksum, which [`OpenVsxClient::download`]
//! verifies before anything is unpacked.

use std::time::Duration;

use base64::Engine as _;
use futures::StreamExt;
use orkestr_protocol::methods::extensions::{ExtensionSort, MarketplaceExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{ExtError, ExtResult};

/// Default registry.
pub const DEFAULT_BASE: &str = "https://open-vsx.org";
/// Max `.vsix` size.
pub const MAX_VSIX_BYTES: u64 = 400 << 20;
/// Max icon size.
const MAX_ICON_BYTES: usize = 512 << 10;
/// Max README / manifest size.
const MAX_TEXT_BYTES: usize = 2 << 20;

/// The Open VSX target platform of this machine (`win32-x64`, `darwin-arm64`, …).
#[must_use]
pub fn current_target() -> String {
    let os = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "arm" => "armhf",
        "x86" => "ia32",
        other => other,
    };
    format!("{os}-{arch}")
}

/// Hex SHA-256.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Parses the body of an Open VSX `.sha256` file (`<hex>` optionally followed by a file name).
pub fn parse_sha256_file(body: &str) -> ExtResult<String> {
    let token = body.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ExtError::Network("the published SHA-256 checksum is malformed".into()));
    }
    Ok(token)
}

/// Verifies `bytes` against the published checksum.
pub fn verify_sha256(bytes: &[u8], expected: &str) -> ExtResult<String> {
    let actual = sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(ExtError::Checksum { expected: expected.to_ascii_lowercase(), actual });
    }
    Ok(actual)
}

/// One resolved, downloadable build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// Namespace (as published).
    pub namespace: String,
    /// Name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Target platform of the build.
    pub target: String,
    /// `.vsix` URL.
    pub download_url: String,
    /// `.sha256` URL.
    pub sha256_url: String,
    /// Verified publisher.
    pub verified: bool,
}

/// Open VSX client.
#[derive(Debug, Clone)]
pub struct OpenVsxClient {
    /// Built on first use (TLS roots are loaded lazily, keeping core start fast).
    http: std::sync::Arc<std::sync::OnceLock<reqwest::Client>>,
    base: String,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty())
}

fn check_segment(seg: &str) -> ExtResult<()> {
    if seg.is_empty()
        || seg.len() > 128
        || seg.chars().all(|c| c == '.')
        || !seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ExtError::Invalid(format!("`{seg}` is not a valid Open VSX identifier")));
    }
    Ok(())
}

/// Builds a search hit / header from an Open VSX JSON object.
#[must_use]
pub fn summary_of(v: &Value) -> Option<MarketplaceExtension> {
    let namespace = s(v, "namespace")?;
    let name = s(v, "name")?;
    Some(MarketplaceExtension {
        id: format!("{namespace}.{name}").to_ascii_lowercase(),
        display_name: s(v, "displayName").unwrap_or_else(|| name.clone()),
        description: s(v, "description"),
        version: s(v, "version").unwrap_or_default(),
        publisher_display_name: s(v, "namespaceDisplayName"),
        verified: v.get("verified").and_then(Value::as_bool).unwrap_or(false),
        downloads: v.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
        rating: v.get("averageRating").and_then(Value::as_f64),
        review_count: v.get("reviewCount").and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
        icon_url: v.pointer("/files/icon").and_then(Value::as_str).map(str::to_owned),
        license: s(v, "license"),
        published_at: s(v, "timestamp"),
        installed_version: None,
        namespace,
        name,
    })
}

impl OpenVsxClient {
    /// Client for `base` (default [`DEFAULT_BASE`]; `ORKESTR_OPENVSX_URL` overrides).
    pub fn new(base: Option<String>) -> ExtResult<Self> {
        let base = base
            .or_else(|| std::env::var("ORKESTR_OPENVSX_URL").ok())
            .unwrap_or_else(|| DEFAULT_BASE.to_owned())
            .trim_end_matches('/')
            .to_owned();
        Ok(Self { http: std::sync::Arc::new(std::sync::OnceLock::new()), base })
    }

    fn http(&self) -> ExtResult<&reqwest::Client> {
        if let Some(c) = self.http.get() {
            return Ok(c);
        }
        let built = reqwest::Client::builder()
            .user_agent(concat!("Orkestr/", env!("CARGO_PKG_VERSION"), " (+open-vsx client)"))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| ExtError::Network(e.to_string()))?;
        Ok(self.http.get_or_init(|| built))
    }

    /// Registry base URL.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn get_json(&self, url: &str) -> ExtResult<Value> {
        let resp = self.http()?.get(url).send().await.map_err(|e| ExtError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ExtError::NotFound(format!("Open VSX: {url}")));
        }
        if !status.is_success() {
            return Err(ExtError::Network(format!("HTTP {status} for {url}")));
        }
        let bytes = resp.bytes().await.map_err(|e| ExtError::Network(e.to_string()))?;
        if bytes.len() > 16 << 20 {
            return Err(ExtError::TooLarge("Open VSX response".into()));
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| ExtError::Network(format!("bad JSON: {e}")))?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(ExtError::NotFound(format!("Open VSX: {err}")));
        }
        Ok(v)
    }

    async fn get_text(&self, url: &str, max: usize) -> ExtResult<String> {
        self.ensure_registry_url(url)?;
        let resp = self.http()?.get(url).send().await.map_err(|e| ExtError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ExtError::Network(format!("HTTP {} for {url}", resp.status())));
        }
        let bytes = resp.bytes().await.map_err(|e| ExtError::Network(e.to_string()))?;
        if bytes.len() > max {
            return Err(ExtError::TooLarge(format!("{url} is larger than {max} bytes")));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Only URLs on the registry itself are fetched (no SSRF through API data):
    /// same scheme, host and port as the base, no credentials, under `/api/`.
    fn ensure_registry_url(&self, url: &str) -> ExtResult<()> {
        let refuse = || ExtError::Invalid(format!("refusing to fetch `{url}`: not an Open VSX registry URL"));
        let base = reqwest::Url::parse(&self.base).map_err(|_| refuse())?;
        let u = reqwest::Url::parse(url).map_err(|_| refuse())?;
        let same_origin = u.scheme() == base.scheme()
            && u.host_str() == base.host_str()
            && u.port_or_known_default() == base.port_or_known_default();
        let api_prefix = format!("{}/api/", base.path().trim_end_matches('/'));
        if !same_origin || !u.username().is_empty() || u.password().is_some() || !u.path().starts_with(&api_prefix) {
            return Err(refuse());
        }
        if u.path().split('/').any(|seg| seg == ".." || seg == "%2e%2e" || seg == "%2E%2E") {
            return Err(refuse());
        }
        Ok(())
    }

    /// Searches the registry.
    pub async fn search(
        &self,
        query: &str,
        offset: u32,
        size: u32,
        sort: ExtensionSort,
        category: Option<&str>,
    ) -> ExtResult<(u64, Vec<MarketplaceExtension>)> {
        let (sort_by, order) = match sort {
            ExtensionSort::Relevance => ("relevance", "desc"),
            ExtensionSort::Downloads => ("downloadCount", "desc"),
            ExtensionSort::Rating => ("averageRating", "desc"),
            ExtensionSort::Recent => ("timestamp", "desc"),
        };
        let mut url = reqwest::Url::parse(&format!("{}/api/-/search", self.base))
            .map_err(|e| ExtError::Invalid(format!("bad registry URL: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("query", query.trim())
                .append_pair("offset", &offset.to_string())
                .append_pair("size", &size.clamp(1, 50).to_string())
                .append_pair("sortBy", sort_by)
                .append_pair("sortOrder", order)
                .append_pair("includeAllVersions", "false");
            if let Some(c) = category.filter(|c| !c.trim().is_empty()) {
                q.append_pair("category", c.trim());
            }
        }
        let v = self.get_json(url.as_str()).await?;
        let total = v.get("totalSize").and_then(Value::as_u64).unwrap_or(0);
        let items = v
            .get("extensions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|e| !e.get("deprecated").and_then(Value::as_bool).unwrap_or(false))
                    .filter_map(summary_of)
                    .collect()
            })
            .unwrap_or_default();
        Ok((total, items))
    }

    /// Raw extension metadata (latest version).
    pub async fn metadata(&self, namespace: &str, name: &str) -> ExtResult<Value> {
        check_segment(namespace)?;
        check_segment(name)?;
        self.get_json(&format!("{}/api/{namespace}/{name}", self.base)).await
    }

    /// Resolves the build to install for `target` (falls back to `universal`).
    pub async fn resolve_build(
        &self,
        namespace: &str,
        name: &str,
        version: Option<&str>,
        target: &str,
    ) -> ExtResult<Build> {
        check_segment(namespace)?;
        check_segment(name)?;
        if let Some(v) = version {
            check_segment(v)?;
        }
        let meta = self.metadata(namespace, name).await?;
        let targets: Vec<String> =
            meta.get("downloads").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let ver = version.unwrap_or("latest");
        let url = if targets.iter().any(|t| t == target) {
            format!("{}/api/{namespace}/{name}/{target}/{ver}", self.base)
        } else if targets.is_empty() || targets.iter().any(|t| t == "universal") {
            format!("{}/api/{namespace}/{name}/{ver}", self.base)
        } else {
            return Err(ExtError::Unsupported(format!(
                "{namespace}.{name} has no build for {target} (available: {})",
                targets.join(", ")
            )));
        };
        let v = self.get_json(&url).await?;
        let download_url = v
            .pointer("/files/download")
            .and_then(Value::as_str)
            .ok_or_else(|| ExtError::NotFound(format!("{namespace}.{name}: no download")))?
            .to_owned();
        let sha256_url = v
            .pointer("/files/sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ExtError::Unsupported(format!(
                    "{namespace}.{name}: Open VSX publishes no SHA-256 checksum for this build"
                ))
            })?
            .to_owned();
        Ok(Build {
            namespace: s(&v, "namespace").unwrap_or_else(|| namespace.to_owned()),
            name: s(&v, "name").unwrap_or_else(|| name.to_owned()),
            version: s(&v, "version").unwrap_or_else(|| ver.to_owned()),
            target: s(&v, "targetPlatform").unwrap_or_else(|| "universal".into()),
            download_url,
            sha256_url,
            verified: v.get("verified").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// Downloads a build and verifies its SHA-256. `progress(received, total)`.
    pub async fn download(
        &self,
        build: &Build,
        mut progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> ExtResult<(Vec<u8>, String)> {
        self.ensure_registry_url(&build.download_url)?;
        let expected = parse_sha256_file(&self.get_text(&build.sha256_url, 4096).await?)?;
        let resp = self.http()?.get(&build.download_url).send().await.map_err(|e| ExtError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ExtError::Network(format!("HTTP {} downloading {}", resp.status(), build.download_url)));
        }
        let total = resp.content_length();
        if total.is_some_and(|t| t > MAX_VSIX_BYTES) {
            return Err(ExtError::TooLarge(format!("the .vsix is larger than {MAX_VSIX_BYTES} bytes")));
        }
        let mut buf: Vec<u8> = Vec::with_capacity(usize::try_from(total.unwrap_or(0).min(64 << 20)).unwrap_or(0));
        let mut stream = resp.bytes_stream();
        let mut last = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ExtError::Network(e.to_string()))?;
            buf.extend_from_slice(&chunk);
            if buf.len() as u64 > MAX_VSIX_BYTES {
                return Err(ExtError::TooLarge(format!("the .vsix is larger than {MAX_VSIX_BYTES} bytes")));
            }
            let n = buf.len() as u64;
            if n - last >= 256 * 1024 {
                last = n;
                progress(n, total);
            }
        }
        progress(buf.len() as u64, total);
        let actual = verify_sha256(&buf, &expected)?;
        Ok((buf, actual))
    }

    /// Fetches a text file from the registry (README / manifest).
    pub async fn text_file(&self, url: &str) -> ExtResult<String> {
        self.get_text(url, MAX_TEXT_BYTES).await
    }

    /// Fetches an icon as a `data:` URL.
    pub async fn icon(&self, url: &str) -> ExtResult<Option<String>> {
        self.ensure_registry_url(url)?;
        let resp = self.http()?.get(url).send().await.map_err(|e| ExtError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let lower = url.to_ascii_lowercase();
        let mime = match ct.as_str() {
            "image/png" | "image/jpeg" | "image/webp" | "image/svg+xml" | "image/gif" => ct.clone(),
            _ if lower.ends_with(".png") => "image/png".into(),
            _ if lower.ends_with(".jpg") || lower.ends_with(".jpeg") => "image/jpeg".into(),
            _ if lower.ends_with(".svg") => "image/svg+xml".into(),
            _ => return Ok(None),
        };
        let bytes = resp.bytes().await.map_err(|e| ExtError::Network(e.to_string()))?;
        if bytes.len() > MAX_ICON_BYTES {
            return Ok(None);
        }
        Ok(Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_verification() {
        let data = b"hello vsix";
        let good = sha256_hex(data);
        assert_eq!(verify_sha256(data, &good).unwrap(), good);
        assert_eq!(verify_sha256(data, &good.to_ascii_uppercase()).unwrap(), good);
        let bad = "0".repeat(64);
        match verify_sha256(data, &bad) {
            Err(ExtError::Checksum { expected, actual }) => {
                assert_eq!(expected, bad);
                assert_eq!(actual, good);
            }
            other => panic!("expected a checksum error, got {other:?}"),
        }
    }

    #[test]
    fn sha256_file_parsing() {
        let h = "f4d8c28fc64874b1b959cc90dbd39c84d5d35c9cf203e32b7a758dc9abf5b1cb";
        assert_eq!(parse_sha256_file(h).unwrap(), h);
        assert_eq!(parse_sha256_file(&format!("{}  x.vsix\n", h.to_uppercase())).unwrap(), h);
        assert!(parse_sha256_file("nope").is_err());
        assert!(parse_sha256_file("").is_err());
    }

    #[test]
    fn registry_url_guard() {
        let c = OpenVsxClient::new(Some("https://open-vsx.org".into())).unwrap();
        assert!(c.ensure_registry_url("https://open-vsx.org/api/a/b/1.0.0/file/icon.png").is_ok());
        assert!(c
            .ensure_registry_url("https://open-vsx.org/api/a/b/win32-x64/1.0.0/file/a.b-1.0.0@win32-x64.vsix")
            .is_ok());
        assert!(c.ensure_registry_url("https://evil.example/api/x").is_err());
        assert!(c.ensure_registry_url("https://open-vsx.org.evil.example/api/x").is_err());
        assert!(c.ensure_registry_url("http://127.0.0.1:8080/x").is_err());
        assert!(c.ensure_registry_url("https://open-vsx.org@evil.example/api/x").is_err());
    }

    #[test]
    fn summary_from_json() {
        let v = serde_json::json!({
            "namespace": "dracula-theme", "name": "theme-dracula", "version": "2.25.1", "verified": true,
            "downloadCount": 10, "averageRating": 4.5, "displayName": "Dracula Theme Official",
            "files": { "icon": "https://open-vsx.org/api/x/icon.png" }
        });
        let s = summary_of(&v).unwrap();
        assert_eq!(s.id, "dracula-theme.theme-dracula");
        assert!(s.verified);
        assert_eq!(s.rating, Some(4.5));
        assert!(summary_of(&serde_json::json!({})).is_none());
    }

    #[test]
    fn target_is_well_formed() {
        let t = current_target();
        assert!(t.contains('-'));
    }

    #[test]
    fn rejects_bad_segments() {
        assert!(check_segment("ok-name_1.2").is_ok());
        assert!(check_segment("../x").is_err());
        assert!(check_segment("a/b").is_err());
        assert!(check_segment("").is_err());
        assert!(check_segment("..").is_err());
    }
}

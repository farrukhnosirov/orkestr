//! `.orkp` packages: zip with `orkestr-plugin.json` (the manifest),
//! `signature.json` (see [`crate::signing`]) and payload files (`plugin.wasm`,
//! `widgets/*`, agent templates, ...).
//!
//! Parsing is done fully in memory with the guards of
//! `orkestr-extensions::archive` (zip slip, links, zip bombs): every entry name
//! is normalized and confined (no `..`, absolute paths, drive letters,
//! backslashes, `:` streams, reserved device names, control characters,
//! case-insensitive duplicates), symlinks are refused, and the entry count,
//! per-file size, total size and compression ratio are capped while the
//! **actual** decompressed bytes are counted.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use orkestr_domain::{Capability, PluginKind, PluginManifest, PLUGIN_ABI_VERSION};

use crate::signing::{self, PackageSignature, PublisherKeyPair, SignerInfo, TrustStore};
use crate::PluginError;

/// Manifest file name at the package root.
pub const MANIFEST_FILE: &str = "orkestr-plugin.json";
/// Signature file name at the package root.
pub const SIGNATURE_FILE: &str = "signature.json";
/// Max unpacked size of a package.
pub const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024;
/// Max size of one file.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Max package (zip) size.
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
/// Max entries in a package.
pub const MAX_ENTRIES: usize = 2_048;
/// Max decompressed / compressed ratio for entries over 1 MiB.
pub const MAX_RATIO: u64 = 100;
/// Env var enabling unsigned / untrusted packages (development only).
pub const DEV_ENV: &str = "ORKESTR_PLUGINS_DEV";

/// Max limits a manifest may ask for.
pub const MAX_MEMORY_MB: u32 = 512;
/// Max fuel per call.
pub const MAX_FUEL: u64 = 100_000_000_000;
/// Max timeout per call.
pub const MAX_TIMEOUT_MS: u32 = 60_000;

const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A parsed, not yet trusted package.
#[derive(Debug, Clone)]
pub struct Package {
    /// Manifest.
    pub manifest: PluginManifest,
    /// sha256 of the package bytes (hex).
    pub sha256: String,
    /// Canonical content digest (what the signature covers).
    pub content_digest: String,
    /// Signature, if the package has one.
    pub signature: Option<PackageSignature>,
    /// Every file (normalized path → bytes), manifest and signature included.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// The wasm module bytes of a code plugin.
    #[must_use]
    pub fn wasm(&self) -> Option<&[u8]> {
        self.manifest.entry.as_deref().and_then(|e| self.files.get(e)).map(Vec::as_slice)
    }
}

/// Whether development mode (unsigned / untrusted packages) is on.
#[must_use]
pub fn dev_mode() -> bool {
    std::env::var(DEV_ENV).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
}

/// Normalizes and confines an archive entry name.
///
/// # Errors
/// Unsafe name.
pub fn normalize_entry(raw: &str) -> Result<String, PluginError> {
    let unsafe_name = |why: &str| PluginError::Package(format!("unsafe entry name `{raw}`: {why}"));
    if raw.is_empty() || raw.len() > 255 {
        return Err(unsafe_name("empty or too long"));
    }
    if raw.contains('\\') || raw.contains(':') || raw.starts_with('/') {
        return Err(unsafe_name("absolute path, drive, stream or backslash"));
    }
    if raw.chars().any(char::is_control) {
        return Err(unsafe_name("control character"));
    }
    let trimmed = raw.trim_end_matches('/');
    let mut parts = Vec::new();
    for c in trimmed.split('/') {
        if c.is_empty() || c == "." || c == ".." {
            return Err(unsafe_name("empty, `.` or `..` component"));
        }
        if c.ends_with('.') || c.ends_with(' ') {
            return Err(unsafe_name("component ends with a dot or space"));
        }
        let stem = c.split('.').next().unwrap_or_default().to_ascii_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err(unsafe_name("reserved device name"));
        }
        parts.push(c);
    }
    Ok(parts.join("/"))
}

/// Reads every file of a zip with the guards above.
///
/// # Errors
/// Not a zip / unsafe / too large.
pub fn read_zip(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(PluginError::Package(format!("package is larger than {MAX_PACKAGE_BYTES} bytes")));
    }
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| PluginError::Package(format!("not a zip: {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(PluginError::Package(format!("{} entries (limit {MAX_ENTRIES})", zip.len())));
    }
    let mut files = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| PluginError::Package(e.to_string()))?;
        let raw = entry.name().to_owned();
        if entry.is_symlink() || entry.unix_mode().is_some_and(|m| m & S_IFMT == S_IFLNK) {
            return Err(PluginError::Package(format!("entry `{raw}` is a symbolic link")));
        }
        let name = normalize_entry(&raw)?;
        if entry.is_dir() || raw.ends_with('/') {
            continue;
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(PluginError::Package(format!("duplicate entry `{raw}`")));
        }
        let declared = entry.size();
        if declared > MAX_FILE_BYTES {
            return Err(PluginError::Package(format!("`{name}` is {declared} bytes (limit {MAX_FILE_BYTES})")));
        }
        let compressed = entry.compressed_size().max(1);
        if declared > (1 << 20) && declared / compressed > MAX_RATIO {
            return Err(PluginError::Package(format!("`{name}` has a suspicious compression ratio (zip bomb?)")));
        }
        let budget = MAX_FILE_BYTES.min(MAX_UNPACKED_BYTES.saturating_sub(total));
        let mut buf = Vec::with_capacity(usize::try_from(declared.min(budget)).unwrap_or(0));
        let n = (&mut entry)
            .take(budget.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|e| PluginError::Package(format!("`{name}`: {e}")))? as u64;
        if n > budget {
            return Err(PluginError::Package(format!(
                "`{name}` decompresses beyond the size limit (total limit {MAX_UNPACKED_BYTES} bytes)"
            )));
        }
        total += n;
        files.insert(name, buf);
    }
    Ok(files)
}

fn valid_package_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    id.len() <= 128
        && parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && p.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Tool name rule `^[a-z][a-z0-9_]{0,63}$`.
#[must_use]
pub fn valid_tool_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.starts_with(|c: char| c.is_ascii_lowercase())
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn valid_scope(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-*/".contains(c))
}

/// Kinds that run wasm code.
#[must_use]
pub fn is_code_kind(k: PluginKind) -> bool {
    matches!(k, PluginKind::Tool | PluginKind::DeploymentProvider | PluginKind::PlatformIntegration)
}

/// Validates a manifest against the package files.
///
/// # Errors
/// [`PluginError::Abi`] for another ABI, [`PluginError::Unimplemented`] for
/// deferred kinds, [`PluginError::Package`] otherwise.
#[allow(clippy::too_many_lines)]
pub fn validate_manifest(m: &PluginManifest, files: &BTreeMap<String, Vec<u8>>) -> Result<(), PluginError> {
    let bad = |s: String| Err(PluginError::Package(s));
    if m.abi != PLUGIN_ABI_VERSION {
        return Err(PluginError::Abi(m.abi));
    }
    if !valid_package_id(&m.id) {
        return bad(format!("invalid package id `{}` (expected `publisher.name`, lowercase)", m.id));
    }
    if semver::Version::parse(&m.version).is_err() {
        return bad(format!("invalid version `{}` (semver)", m.version));
    }
    if m.name.trim().is_empty() || m.name.chars().count() > 100 || m.publisher.trim().is_empty() {
        return bad("name and publisher are required (name max 100 chars)".into());
    }
    if m.description.chars().count() > 2_000 {
        return bad("description too long (max 2000)".into());
    }
    if m.kinds.is_empty() {
        return bad("no kinds declared".into());
    }
    for k in &m.kinds {
        match k {
            PluginKind::Provider => return Err(PluginError::Unimplemented("plugin kind Provider")),
            PluginKind::Panel => return Err(PluginError::Unimplemented("plugin kind Panel")),
            _ => {}
        }
    }
    let code = m.kinds.iter().any(|k| is_code_kind(*k));
    match (&m.entry, code) {
        (Some(e), _) => {
            let lower = e.to_ascii_lowercase();
            if [".js", ".mjs", ".cjs", ".ts"].iter().any(|x| lower.ends_with(x)) {
                // JS / QuickJS plugins are deferred (RR-G14): only wasm runs.
                return Err(PluginError::Unimplemented("JavaScript (QuickJS) plugins"));
            }
            let Some(bytes) = files.get(e) else { return bad(format!("entry `{e}` is not in the package")) };
            if !e.ends_with(".wasm") || !bytes.starts_with(b"\0asm") {
                return bad(format!("entry `{e}` is not a wasm module"));
            }
        }
        (None, true) => return bad("code plugins (Tool, DeploymentProvider, PlatformIntegration) need `entry`".into()),
        (None, false) => {}
    }
    if m.kinds.contains(&PluginKind::Tool) && m.tools.is_empty() {
        return bad("kind Tool declares no tools".into());
    }
    let mut names = std::collections::HashSet::new();
    for t in &m.tools {
        if !valid_tool_name(&t.name) || !names.insert(t.name.as_str()) {
            return bad(format!("invalid or duplicate tool name `{}`", t.name));
        }
        if t.description.chars().count() > 2_000 || !t.input_schema.is_object() {
            return bad(format!("tool `{}`: description max 2000 chars and an object input schema", t.name));
        }
    }
    let mut wids = std::collections::HashSet::new();
    for w in &m.widgets {
        if !valid_tool_name(&w.id.replace('-', "_")) || !wids.insert(w.id.as_str()) {
            return bad(format!("invalid or duplicate widget id `{}`", w.id));
        }
        if !files.contains_key(&w.entry_html) {
            return bad(format!("widget `{}`: `{}` is not in the package", w.id, w.entry_html));
        }
        if w.sizes.is_empty() || w.sizes.iter().any(|s| s.w == 0 || s.w > 12 || s.h == 0 || s.h > 24) {
            return bad(format!("widget `{}`: sizes must be 1-12 x 1-24", w.id));
        }
    }
    for t in &m.agent_templates {
        if !files.contains_key(t) {
            return bad(format!("agent template `{t}` is not in the package"));
        }
    }
    if !m.deploy_providers.is_empty() != m.kinds.contains(&PluginKind::DeploymentProvider) {
        return bad("kind DeploymentProvider and `deployProviders` go together".into());
    }
    let mut provs = std::collections::HashSet::new();
    for d in &m.deploy_providers {
        if !valid_tool_name(&d.replace('-', "_")) || !provs.insert(d.as_str()) {
            return bad(format!("invalid or duplicate deploy provider id `{d}`"));
        }
    }
    if !m.subscribes.is_empty() && m.entry.is_none() {
        return bad("`subscribes` needs a code plugin (`entry`)".into());
    }
    let mut subs = std::collections::HashSet::new();
    for k in &m.subscribes {
        if !crate::events::subscribable(k) || !subs.insert(k.as_str()) {
            return bad(format!(
                "`subscribes`: `{k}` is not a subscribable event kind (or listed twice); see orkestr_plugins::events::SUBSCRIBABLE"
            ));
        }
    }
    for p in &m.permissions {
        if p.reason.chars().count() > 500 {
            return bad("permission reason too long (max 500)".into());
        }
        match (&p.scope, p.capability) {
            (Some(s), _) if !valid_scope(s) => return bad(format!("invalid permission scope `{s}`")),
            (None, Capability::Network | Capability::Secrets | Capability::Read) => {
                return bad(format!("{:?} permissions need a scope (host, secret name or RPC method)", p.capability))
            }
            _ => {}
        }
    }
    let l = m.limits;
    if l.memory_mb == 0 || l.memory_mb > MAX_MEMORY_MB {
        return bad(format!("limits.memoryMb must be 1-{MAX_MEMORY_MB}"));
    }
    if l.fuel_per_call == 0 || l.fuel_per_call > MAX_FUEL {
        return bad(format!("limits.fuelPerCall must be 1-{MAX_FUEL}"));
    }
    if l.timeout_ms == 0 || l.timeout_ms > MAX_TIMEOUT_MS {
        return bad(format!("limits.timeoutMs must be 1-{MAX_TIMEOUT_MS}"));
    }
    Ok(())
}

/// Parses a package from bytes (structure + manifest; no signature check).
///
/// # Errors
/// Invalid package / manifest.
pub fn parse(bytes: &[u8]) -> Result<Package, PluginError> {
    let files = read_zip(bytes)?;
    let raw = files.get(MANIFEST_FILE).ok_or_else(|| PluginError::Package(format!("`{MANIFEST_FILE}` is missing")))?;
    let manifest: PluginManifest =
        serde_json::from_slice(raw).map_err(|e| PluginError::Package(format!("invalid manifest: {e}")))?;
    validate_manifest(&manifest, &files)?;
    let signature = files
        .get(SIGNATURE_FILE)
        .map(|s| serde_json::from_slice::<PackageSignature>(s))
        .transpose()
        .map_err(|e| PluginError::Signature(format!("invalid {SIGNATURE_FILE}: {e}")))?;
    Ok(Package {
        manifest,
        sha256: signing::sha256_hex(bytes),
        content_digest: signing::content_digest(&files),
        signature,
        files,
    })
}

/// Outcome of [`verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// Signed by a trusted key.
    Trusted(SignerInfo),
    /// Validly signed by an unknown key (allowed only in dev mode).
    UntrustedDev(SignerInfo),
    /// Unsigned (allowed only in dev mode).
    UnsignedDev,
}

impl Trust {
    /// The signer, if any.
    #[must_use]
    pub fn signer(&self) -> Option<&SignerInfo> {
        match self {
            Trust::Trusted(s) | Trust::UntrustedDev(s) => Some(s),
            Trust::UnsignedDev => None,
        }
    }
}

/// Verifies the package signature against the trust store. A signature that
/// does not match the content is **always** refused; unsigned or untrusted
/// packages pass only when `dev` is true.
///
/// # Errors
/// [`PluginError::Signature`].
pub async fn verify(pkg: &Package, trust: &TrustStore, dev: bool) -> Result<Trust, PluginError> {
    let Some(sig) = &pkg.signature else {
        return if dev {
            Ok(Trust::UnsignedDev)
        } else {
            Err(PluginError::Signature(format!("the package is not signed (unsigned packages need {DEV_ENV}=1)")))
        };
    };
    let message = signing::package_message(&pkg.content_digest);
    let signer = signing::verify(sig, &message)?;
    match trust.verify_trusted(sig, &message).await {
        Ok(_) => Ok(Trust::Trusted(signer)),
        Err(_) if dev => Ok(Trust::UntrustedDev(signer)),
        Err(e) => Err(e),
    }
}

/// Writes a zip of `entries` (in order, deflated). Used by [`build`], the
/// fixture generator and publishers.
///
/// # Errors
/// Zip writer failure.
pub fn write_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, PluginError> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        w.start_file(name.as_str(), opts).map_err(|e| PluginError::Internal(e.to_string()))?;
        w.write_all(bytes)?;
    }
    Ok(w.finish().map_err(|e| PluginError::Internal(e.to_string()))?.into_inner())
}

/// Builds a `.orkp`: `manifest` + `files` (payload paths → bytes), signed with
/// `key` when given.
///
/// # Errors
/// Invalid manifest / payload.
pub fn build(
    manifest: &PluginManifest,
    files: &BTreeMap<String, Vec<u8>>,
    key: Option<&PublisherKeyPair>,
) -> Result<Vec<u8>, PluginError> {
    let mut all: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (k, v) in files {
        let n = normalize_entry(k)?;
        if n == MANIFEST_FILE || n == SIGNATURE_FILE {
            continue;
        }
        all.insert(n, v.clone());
    }
    all.insert(
        MANIFEST_FILE.to_owned(),
        serde_json::to_vec_pretty(manifest).map_err(|e| PluginError::Internal(e.to_string()))?,
    );
    validate_manifest(manifest, &all)?;
    if let Some(k) = key {
        let sig = k.sign(&signing::package_message(&signing::content_digest(&all)));
        all.insert(
            SIGNATURE_FILE.to_owned(),
            serde_json::to_vec_pretty(&sig).map_err(|e| PluginError::Internal(e.to_string()))?,
        );
    }
    write_zip(&all.into_iter().collect::<Vec<_>>())
}

/// Writes the package files under `dest` (created fresh; replaced if present).
///
/// # Errors
/// I/O.
pub fn extract_to(pkg: &Package, dest: &Path) -> Result<(), PluginError> {
    let staging = dest.with_extension("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    let result = (|| -> Result<(), PluginError> {
        for (name, bytes) in &pkg.files {
            let target = name.split('/').fold(staging.clone(), |p, c| p.join(c));
            if !target.starts_with(&staging) {
                return Err(PluginError::Package(format!("entry `{name}` escapes the package folder")));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?.write_all(bytes)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::rename(&staging, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{PluginLimits, ToolDecl};

    pub(crate) fn manifest() -> PluginManifest {
        PluginManifest {
            id: "acme.hello".into(),
            name: "Hello".into(),
            version: "1.0.0".into(),
            publisher: "Acme".into(),
            description: String::new(),
            abi: 1,
            kinds: vec![PluginKind::Tool],
            entry: Some("plugin.wasm".into()),
            permissions: vec![],
            tools: vec![ToolDecl {
                name: "hello".into(),
                description: "Says hello".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            widgets: vec![],
            deploy_providers: vec![],
            agent_templates: vec![],
            subscribes: vec![],
            limits: PluginLimits::default(),
        }
    }

    fn payload() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([("plugin.wasm".to_owned(), b"\0asm\x01\0\0\0".to_vec())])
    }

    #[test]
    fn entry_names_are_confined() {
        for bad in ["../x", "a/../../x", "/etc/passwd", "C:/x", "a\\b", "a/./b", "con.txt", "x:stream", "a//b", "a."] {
            assert!(normalize_entry(bad).is_err(), "{bad}");
        }
        assert_eq!(normalize_entry("widgets/a.html").unwrap(), "widgets/a.html");
    }

    #[test]
    fn traversal_entry_is_rejected() {
        let z = write_zip(&[(MANIFEST_FILE.into(), b"{}".to_vec()), ("../evil.txt".into(), b"x".to_vec())]).unwrap();
        assert!(matches!(parse(&z), Err(PluginError::Package(m)) if m.contains("unsafe")));
    }

    #[test]
    fn zip_bomb_is_rejected() {
        let big = vec![0u8; 40 * 1024 * 1024];
        let z = write_zip(&[("big.bin".into(), big)]).unwrap();
        assert!(matches!(read_zip(&z), Err(PluginError::Package(_))));
    }

    #[test]
    fn abi_and_kinds_are_checked() {
        let mut m = manifest();
        m.abi = 2;
        assert!(matches!(build(&m, &payload(), None), Err(PluginError::Abi(2))));
        let mut m = manifest();
        m.kinds = vec![PluginKind::Panel];
        assert!(matches!(build(&m, &payload(), None), Err(PluginError::Unimplemented(_))));
        let mut m = manifest();
        m.entry = Some("plugin.js".into());
        let js = BTreeMap::from([("plugin.js".to_owned(), b"export function call() {}".to_vec())]);
        assert!(matches!(build(&m, &js, None), Err(PluginError::Unimplemented(f)) if f.contains("JavaScript")));
        let mut m = manifest();
        m.permissions.push(orkestr_domain::PermissionRequest {
            capability: Capability::Network,
            scope: None,
            reason: "x".into(),
        });
        assert!(build(&m, &payload(), None).is_err());
    }

    #[tokio::test]
    async fn signed_ok_tampered_and_unsigned_refused() {
        let dir = tempfile::tempdir().unwrap();
        let db = orkestr_storage::Db::open(dir.path().join("t.db")).unwrap();
        db.migrate(crate::store::migrations()).await.unwrap();
        let trust = TrustStore::new(db);
        let key = PublisherKeyPair::generate();
        trust.trust(&key.public_key_b64(), "Acme", "test").await.unwrap();
        let bytes = build(&manifest(), &payload(), Some(&key)).unwrap();
        let pkg = parse(&bytes).unwrap();
        assert!(matches!(verify(&pkg, &trust, false).await.unwrap(), Trust::Trusted(_)));
        // Tamper: change the wasm, keep the signature.
        let mut files = read_zip(&bytes).unwrap();
        files.insert("plugin.wasm".into(), b"\0asm\x01\0\0\0\0".to_vec());
        let tampered = parse(&write_zip(&files.into_iter().collect::<Vec<_>>()).unwrap()).unwrap();
        assert!(matches!(verify(&tampered, &trust, false).await, Err(PluginError::Signature(_))));
        assert!(matches!(verify(&tampered, &trust, true).await, Err(PluginError::Signature(_))));
        // Unsigned: only in dev mode.
        let unsigned = parse(&build(&manifest(), &payload(), None).unwrap()).unwrap();
        assert!(verify(&unsigned, &trust, false).await.is_err());
        assert_eq!(verify(&unsigned, &trust, true).await.unwrap(), Trust::UnsignedDev);
        // Untrusted key: only in dev mode.
        let other = parse(&build(&manifest(), &payload(), Some(&PublisherKeyPair::generate())).unwrap()).unwrap();
        assert!(verify(&other, &trust, false).await.is_err());
        assert!(matches!(verify(&other, &trust, true).await.unwrap(), Trust::UntrustedDev(_)));
        // Extraction.
        let out = dir.path().join("x");
        extract_to(&pkg, &out).unwrap();
        assert!(out.join("plugin.wasm").is_file() && out.join(MANIFEST_FILE).is_file());
    }
}

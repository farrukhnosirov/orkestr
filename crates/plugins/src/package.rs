//! `.orkp` packages: zip with `orkestr-plugin.json`, `signature.json` and
//! payload files. Safe extraction (no `..`, absolute paths, symlinks; size and
//! entry-count caps against zip bombs). Reference: `orkestr-extensions` `archive.rs`.

use orkestr_domain::PluginManifest;

use crate::PluginError;

/// Manifest file name at the package root.
pub const MANIFEST_FILE: &str = "orkestr-plugin.json";
/// Signature file name at the package root.
pub const SIGNATURE_FILE: &str = "signature.json";
/// Max unpacked size of a package.
pub const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024;
/// Max entries in a package.
pub const MAX_ENTRIES: usize = 2_048;

/// A parsed, not yet trusted package.
#[derive(Debug, Clone)]
pub struct Package {
    /// Manifest.
    pub manifest: PluginManifest,
    /// sha256 of the package bytes (hex).
    pub sha256: String,
}

/// Parses a package from bytes (no signature check).
///
/// # Errors
/// Not implemented in the step-0 skeleton.
pub fn parse(_bytes: &[u8]) -> Result<Package, PluginError> {
    Err(PluginError::Unimplemented("plugins.package"))
}

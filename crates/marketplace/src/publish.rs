//! Local publishing with the vault-held publisher key. **Owner: WP2.**
//!
//! [`describe`] derives the index metadata of a package file; the service
//! signs the digest with the publisher key and either rewrites a directory
//! registry or POSTs a [`PublishRequest`] to a gateway (`/registry/publish`).

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use orkestr_domain::{PermissionRequest, PluginKind};
use orkestr_protocol::methods::marketplace::PackageKind;

use crate::MarketplaceError;

/// Agent template envelope format (`templates.export`).
pub const TEMPLATE_FORMAT: &str = "orkestr.agent-template";

/// Index metadata of a package (everything but key, digest and URL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMeta {
    /// Package id.
    pub id: String,
    /// Kind.
    pub kind: PackageKind,
    /// Display name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Publisher.
    pub publisher: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Requested permissions.
    #[serde(default)]
    pub permissions: Vec<PermissionRequest>,
}

/// Body of `POST /registry/publish`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequest {
    /// Metadata.
    pub entry: PackageMeta,
    /// Publisher public key (base64).
    pub public_key: String,
    /// Publisher signature (base64) over the raw sha256 digest of the package.
    pub signature: String,
    /// Package bytes (base64).
    pub package_base64: String,
}

/// `slug("Strict Code Reviewer")` = `strict-code-reviewer`.
#[must_use]
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn invalid(m: impl Into<String>) -> MarketplaceError {
    MarketplaceError::Invalid(m.into())
}

/// Derives the metadata of a package file.
///
/// # Errors
/// Not a valid package of `kind`.
pub fn describe(kind: PackageKind, bytes: &[u8], publisher: &str) -> Result<PackageMeta, MarketplaceError> {
    match kind {
        PackageKind::AgentTemplate => describe_template(bytes, publisher),
        PackageKind::Plugin | PackageKind::Widget => {
            let pkg = orkestr_plugins::package::parse(bytes).map_err(|e| match e {
                orkestr_plugins::PluginError::Unimplemented(f) => MarketplaceError::Unimplemented(f),
                other => invalid(format!("not a plugin package: {other}")),
            })?;
            let m = pkg.manifest;
            if kind == PackageKind::Widget && !m.kinds.contains(&PluginKind::Widget) {
                return Err(invalid("a widget package must declare the Widget kind"));
            }
            let mut tags: Vec<String> = m.kinds.iter().map(|k| slug(&format!("{k:?}"))).collect();
            tags.sort();
            tags.dedup();
            Ok(PackageMeta {
                id: m.id,
                kind,
                name: m.name,
                version: m.version,
                publisher: if m.publisher.is_empty() { publisher.to_owned() } else { m.publisher },
                description: m.description,
                tags,
                permissions: m.permissions,
            })
        }
    }
}

fn describe_template(bytes: &[u8], publisher: &str) -> Result<PackageMeta, MarketplaceError> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| invalid(format!("agent template is not JSON: {e}")))?;
    if v["format"] != TEMPLATE_FORMAT {
        return Err(invalid(format!("agent template must have format `{TEMPLATE_FORMAT}`")));
    }
    let t = &v["template"];
    let name = t["name"].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| invalid("template.name missing"))?;
    let version = t["version"].as_str().unwrap_or("1.0.0").to_owned();
    if semver::Version::parse(&version).is_err() {
        return Err(invalid(format!("template version `{version}` is not semver")));
    }
    let mut tags = vec!["agent-template".to_owned()];
    if let Some(role) = t["role"].as_str() {
        tags.push(slug(role));
    }
    Ok(PackageMeta {
        id: format!("{}.{}", slug(publisher), slug(name)),
        kind: PackageKind::AgentTemplate,
        name: name.to_owned(),
        version,
        publisher: publisher.to_owned(),
        description: t["description"].as_str().unwrap_or_default().to_owned(),
        tags,
        permissions: Vec::new(),
    })
}

/// Whether all permission rules of a template envelope are denials (then
/// keeping them on import only narrows what the agent may do).
#[must_use]
pub fn template_permissions_only_deny(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).ok().is_some_and(|v| {
        v["template"]["permissions"].as_array().is_none_or(|rules| rules.iter().all(|r| r["effect"] == "Deny"))
    })
}

/// Semver ordering (non-semver versions sort lexically, below semver ones).
#[must_use]
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Greater,
        (Err(_), Ok(_)) => Ordering::Less,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_versions() {
        assert_eq!(slug("Strict Code  Reviewer!"), "strict-code-reviewer");
        assert_eq!(version_cmp("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(version_cmp("1.0.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn describes_templates() {
        let json = br#"{"format":"orkestr.agent-template","schemaVersion":1,"template":{"name":"Docs Writer","role":"Researcher","version":"1.2.0","description":"d","permissions":[{"effect":"Deny"}]}}"#;
        let m = describe(PackageKind::AgentTemplate, json, "Acme Inc").unwrap();
        assert_eq!(m.id, "acme-inc.docs-writer");
        assert_eq!(m.version, "1.2.0");
        assert!(m.tags.contains(&"researcher".to_owned()));
        assert!(template_permissions_only_deny(json));
        assert!(describe(PackageKind::AgentTemplate, b"{}", "x").is_err());
    }
}

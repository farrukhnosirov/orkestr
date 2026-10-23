//! Extensions in Orkestr's own VS Code extensions directory, managed through
//! the installed server's CLI (`server-main.js --install-extension` /
//! `--list-extensions`). Extensions come from Open VSX (the VSCodium
//! default registry), never from the Microsoft Marketplace.
//!
//! Ids are validated strictly before they reach the command line, so a
//! value can never be parsed as another CLI flag or a local `.vsix` path.

use orkestr_protocol::methods::vscode::VsCodeExtension;

use crate::error::{HostError, HostResult};

/// Maximum length of an extension id (`publisher.name@version`).
const MAX_ID_LEN: usize = 200;

fn is_name_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_version(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().next().is_some_and(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
}

/// Validates and normalizes an Open VSX id `publisher.name[@version]`
/// (the id part is lowercased; the version is kept as given).
///
/// # Errors
/// [`HostError::Invalid`] for anything that is not a plain extension id.
pub fn normalize_extension_id(raw: &str) -> HostResult<String> {
    let raw = raw.trim();
    let invalid = || HostError::Invalid(format!("not an extension id (expected publisher.name): {raw:.80}"));
    if raw.is_empty() || raw.len() > MAX_ID_LEN {
        return Err(invalid());
    }
    let (id, version) = match raw.split_once('@') {
        Some((id, v)) => (id, Some(v)),
        None => (raw, None),
    };
    let (publisher, name) = id.split_once('.').ok_or_else(invalid)?;
    if !is_name_part(publisher) || !is_name_part(name) {
        return Err(invalid());
    }
    if let Some(v) = version {
        if !is_version(v) {
            return Err(invalid());
        }
    }
    let id = id.to_ascii_lowercase();
    Ok(version.map_or_else(|| id.clone(), |v| format!("{id}@{v}")))
}

/// Parses `--list-extensions --show-versions` output (`publisher.name@1.2.3`
/// per line; other lines are ignored).
#[must_use]
pub fn parse_extension_list(stdout: &str) -> Vec<VsCodeExtension> {
    let mut out: Vec<VsCodeExtension> = stdout
        .lines()
        .filter_map(|line| {
            let norm = normalize_extension_id(line).ok()?;
            let (id, version) = match norm.split_once('@') {
                Some((id, v)) => (id.to_owned(), Some(v.to_owned())),
                None => (norm, None),
            };
            Some(VsCodeExtension { id, version, disabled: false, restart_required: false })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_validated_and_lowercased() {
        assert_eq!(normalize_extension_id(" Anthropic.claude-code ").unwrap(), "anthropic.claude-code");
        assert_eq!(normalize_extension_id("redhat.vscode-yaml@1.15.0").unwrap(), "redhat.vscode-yaml@1.15.0");
        for bad in [
            "",
            "claude",
            "--install-extension",
            "-a.b",
            "a.b c",
            "C:\\x\\ext.vsix",
            "../a.b",
            "a.b@",
            "a.b@--force",
            "a./b",
            "https://x.y",
        ] {
            assert!(normalize_extension_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn list_output_is_parsed() {
        let out = "Extensions installed on Orkestr:\nanthropic.claude-code@2.0.1\nRedhat.vscode-yaml@1.15.0\n\n";
        let list = parse_extension_list(out);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "anthropic.claude-code");
        assert_eq!(list[0].version.as_deref(), Some("2.0.1"));
        assert_eq!(list[1].id, "redhat.vscode-yaml");
    }
}

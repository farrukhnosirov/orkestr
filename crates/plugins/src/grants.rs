//! Plugin permission grants: stored in the `PermissionEngine` rule store with
//! principal `plugin:<id>` (ADR-0034). **Owner: WP1.**
//!
//! A decision becomes a `Global` permission rule whose resource is
//! `plugin:<id>/<scope>` (`*` = whole capability) with effect `Allow` or
//! `Deny`. The rules are visible (and revocable) in the Permissions panel. They
//! never match an agent check: agent resources are paths, URLs, commands or
//! secret names, never `plugin:` strings. The host evaluates them itself with
//! [`GrantSet`] (Deny wins; host scopes match exactly or by `*.` suffix).

use orkestr_domain::{Capability, PermissionEffect, PermissionRule, PermissionScope, PluginId};
use orkestr_security::permissions::RuleFilter;
use orkestr_security::Security;

use crate::PluginError;

/// Principal string of a plugin in the permission engine.
#[must_use]
pub fn principal(id: &PluginId) -> String {
    format!("plugin:{id}")
}

/// Rule resource of a grant.
#[must_use]
pub fn rule_resource(id: &PluginId, scope: Option<&str>) -> String {
    format!("{}/{}", principal(id), scope.unwrap_or("*"))
}

/// Whether a granted `scope` covers `target` for `cap`.
#[must_use]
pub fn scope_matches(cap: Capability, scope: &str, target: &str) -> bool {
    if scope == "*" {
        return true;
    }
    if cap == Capability::Network {
        let (s, t) = (scope.to_ascii_lowercase(), target.to_ascii_lowercase());
        return match s.strip_prefix("*.") {
            Some(suffix) => t.ends_with(&format!(".{suffix}")),
            None => s == t,
        };
    }
    scope == target
}

/// One stored grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRule {
    /// Rule id in the permission store.
    pub rule_id: String,
    /// Capability.
    pub capability: Capability,
    /// Scope (`*` = any).
    pub scope: String,
    /// Allow (`true`) or deny.
    pub allowed: bool,
}

/// The grants of one plugin, loaded from the permission rule store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantSet {
    /// Rules.
    pub rules: Vec<GrantRule>,
}

impl GrantSet {
    /// Loads the (unexpired) rules of principal `plugin:<id>`.
    ///
    /// # Errors
    /// Storage.
    pub async fn load(security: &Security, id: &PluginId) -> Result<Self, PluginError> {
        let prefix = format!("{}/", principal(id));
        let now = orkestr_domain::now();
        let rules = security
            .permissions
            .list(&RuleFilter::default())
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))?
            .into_iter()
            .filter(|r| r.expires_at.is_none_or(|e| e > now) && matches!(r.rule.scope, PermissionScope::Global))
            .filter_map(|r| {
                let scope = r.rule.resource.as_deref()?.strip_prefix(&prefix)?.to_owned();
                let allowed = match r.rule.effect {
                    PermissionEffect::Allow => true,
                    PermissionEffect::Deny => false,
                    PermissionEffect::Ask => return None,
                };
                Some(GrantRule { rule_id: r.id, capability: r.rule.capability, scope, allowed })
            })
            .collect();
        Ok(Self { rules })
    }

    /// Whether `cap` on `target` is granted (some Allow matches, no Deny matches).
    #[must_use]
    pub fn allows(&self, cap: Capability, target: &str) -> bool {
        let matching = || self.rules.iter().filter(|r| r.capability == cap && scope_matches(cap, &r.scope, target));
        matching().any(|r| r.allowed) && !matching().any(|r| !r.allowed)
    }
}

/// Stores one decision (replacing an earlier decision for the same capability + scope).
///
/// # Errors
/// Storage / permission store refusal.
pub async fn store_decision(
    security: &Security,
    id: &PluginId,
    package_id: &str,
    capability: Capability,
    scope: Option<&str>,
    allow: bool,
) -> Result<(), PluginError> {
    let resource = rule_resource(id, scope);
    let existing = GrantSet::load(security, id).await?;
    for r in existing.rules.iter().filter(|r| r.capability == capability && r.scope == scope.unwrap_or("*")) {
        security
            .permissions
            .delete(&r.rule_id, security.user())
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))?;
    }
    let rule = PermissionRule {
        capability,
        effect: if allow { PermissionEffect::Allow } else { PermissionEffect::Deny },
        resource: Some(resource),
        scope: PermissionScope::Global,
    };
    security
        .permissions
        .set(None, rule, Some(format!("plugin grant: {package_id} ({})", principal(id))), None, security.user())
        .await
        .map_err(|e| PluginError::Internal(e.to_string()))?;
    Ok(())
}

/// Removes every grant of a plugin (uninstall).
///
/// # Errors
/// Storage.
pub async fn revoke_all(security: &Security, id: &PluginId) -> Result<usize, PluginError> {
    let set = GrantSet::load(security, id).await?;
    for r in &set.rules {
        security
            .permissions
            .delete(&r.rule_id, security.user())
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))?;
    }
    Ok(set.rules.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_scopes() {
        assert!(scope_matches(Capability::Network, "api.example.com", "API.example.com"));
        assert!(!scope_matches(Capability::Network, "api.example.com", "evil.example.com"));
        assert!(scope_matches(Capability::Network, "*.example.com", "a.b.example.com"));
        assert!(!scope_matches(Capability::Network, "*.example.com", "example.com.evil.io"));
        assert!(!scope_matches(Capability::Network, "*.example.com", "badexample.com"));
        assert!(scope_matches(Capability::Secrets, "*", "X"));
        assert!(!scope_matches(Capability::Secrets, "A", "B"));
    }

    #[test]
    fn deny_wins() {
        let set = GrantSet {
            rules: vec![
                GrantRule {
                    rule_id: "1".into(),
                    capability: Capability::Network,
                    scope: "*.x.io".into(),
                    allowed: true,
                },
                GrantRule {
                    rule_id: "2".into(),
                    capability: Capability::Network,
                    scope: "bad.x.io".into(),
                    allowed: false,
                },
            ],
        };
        assert!(set.allows(Capability::Network, "ok.x.io"));
        assert!(!set.allows(Capability::Network, "bad.x.io"));
        assert!(!set.allows(Capability::Secrets, "ok.x.io"));
    }
}

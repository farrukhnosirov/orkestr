//! Authorization of remote calls (remote-access.md §7, §10): `MethodPolicy`
//! enforcement, per-device token buckets, step-up, topic filtering.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use orkestr_domain::{ApprovalId, DeviceId, RiskLevel};
use orkestr_protocol::policy::{policy_for, MethodPolicy, RateClass, Role, StepUp};
use orkestr_protocol::{topic, Event, RpcError};
use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::rpc::{RemotePrincipal, RpcContext};

/// Token-bucket parameters `(capacity, refill per second)`; `None` = unlimited.
#[must_use]
pub fn bucket_params(class: RateClass) -> Option<(f64, f64)> {
    match class {
        RateClass::Read => Some((50.0, 50.0)),
        RateClass::Write => Some((10.0, 10.0)),
        RateClass::Expensive => Some((5.0, 1.0)),
        RateClass::Auth => Some((5.0, 0.2)),
        RateClass::Emergency => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Per-device, per-class token buckets.
#[derive(Debug, Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<(DeviceId, RateClass), Bucket>>,
}

impl RateLimiter {
    /// Takes one token; `Err(retry_after)` when empty.
    ///
    /// # Errors
    /// The bucket is empty.
    pub fn check(&self, device: DeviceId, class: RateClass) -> Result<(), Duration> {
        self.check_at(device, class, Instant::now())
    }

    /// [`Self::check`] at an explicit instant (tests).
    ///
    /// # Errors
    /// The bucket is empty.
    pub fn check_at(&self, device: DeviceId, class: RateClass, now: Instant) -> Result<(), Duration> {
        let Some((cap, rate)) = bucket_params(class) else { return Ok(()) };
        let mut map = self.buckets.lock();
        let b = map.entry((device, class)).or_insert(Bucket { tokens: cap, at: now });
        let elapsed = now.saturating_duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + elapsed * rate).min(cap);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / rate))
        }
    }

    /// Drops a device's buckets (revocation).
    pub fn forget(&self, device: DeviceId) {
        self.buckets.lock().retain(|(d, _), _| *d != device);
    }
}

/// Outcome of the static checks (no I/O).
///
/// # Errors
/// `forbidden` with a reason.
pub fn check_static(rp: &RemotePrincipal, method: &str, params: &Value) -> Result<MethodPolicy, RpcError> {
    let Some(policy) = policy_for(method) else {
        return Err(RpcError::forbidden(format!("`{method}` has no remote policy")));
    };
    if policy.local_only {
        return Err(RpcError::forbidden(format!("`{method}` is local-only"))
            .with_data(serde_json::json!({ "localOnly": true })));
    }
    if rp.role < policy.min_role {
        return Err(RpcError::forbidden(format!("`{method}` requires role {:?}", policy.min_role))
            .with_data(serde_json::json!({ "requiredRole": policy.min_role })));
    }
    if method.starts_with("terminal.") && !rp.remote_terminal {
        return Err(RpcError::forbidden("remote terminal is not enabled for this device")
            .with_data(serde_json::json!({ "remoteTerminal": false })));
    }
    if !rp.project_ids.is_empty() {
        if let Some(p) = params.get("projectId").and_then(Value::as_str) {
            if !rp.project_ids.iter().any(|id| id.to_string() == p) {
                return Err(RpcError::forbidden("project is outside this device's allowlist"));
            }
        }
    }
    Ok(policy)
}

/// Hash of the params a step-up is bound to (`stepUp` removed; keys sorted).
#[must_use]
pub fn params_hash(params: &Value) -> [u8; 32] {
    let mut p = params.clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("stepUp");
    }
    Sha256::digest(serde_json::to_vec(&p).unwrap_or_default()).into()
}

/// Whether a `WhenDangerous` call is dangerous in this instance.
pub async fn is_dangerous(ctx: &RpcContext, method: &str, params: &Value) -> bool {
    if method != "approvals.resolve" {
        // destructive VCS / file / rollback operations: always dangerous remotely
        return true;
    }
    if params.get("decision").and_then(Value::as_str) == Some("Reject") {
        return false;
    }
    let Some(id) = params.get("approvalId").and_then(Value::as_str).and_then(|s| ApprovalId::parse(s).ok()) else {
        return true;
    };
    match ctx.services.security.approvals.get(id).await {
        Ok(Some(a)) => a.risk >= RiskLevel::High,
        _ => true,
    }
}

/// Full authorization of a remote call. Removes `params.stepUp`.
///
/// # Errors
/// `forbidden` (policy / step-up) or `rate_limited`.
pub async fn authorize(
    ctx: &RpcContext,
    rp: &RemotePrincipal,
    method: &str,
    params: &mut Value,
) -> Result<(), RpcError> {
    let policy = check_static(rp, method, params)?;
    let remote = &ctx.services.remote;
    if let Err(retry) = remote.limiter().check(rp.device_id, policy.rate_class) {
        let ms = u64::try_from(retry.as_millis()).unwrap_or(u64::MAX).max(1);
        return Err(RpcError::new(orkestr_protocol::RpcErrorCode::RateLimited, "rate limited")
            .with_data(serde_json::json!({ "retryAfterMs": ms })));
    }
    let step_up = params.as_object_mut().and_then(|o| o.remove("stepUp"));
    let needed = match policy.step_up {
        StepUp::Never => false,
        StepUp::Always => true,
        StepUp::WhenDangerous => is_dangerous(ctx, method, params).await,
    };
    if !needed {
        return Ok(());
    }
    let hash = params_hash(params);
    match step_up {
        Some(assertion) => remote.verify_step_up(rp.device_id, method, &hash, &assertion).map_err(|reason| {
            let challenge = remote.issue_step_up(rp.device_id, method, &hash);
            RpcError::forbidden(format!("step-up failed: {reason}"))
                .with_data(serde_json::json!({ "stepUpRequired": true, "challenge": challenge, "reason": reason }))
        }),
        None => {
            let challenge = remote.issue_step_up(rp.device_id, method, &hash);
            Err(RpcError::forbidden("step-up required")
                .with_data(serde_json::json!({ "stepUpRequired": true, "challenge": challenge })))
        }
    }
}

/// Whether a remote device may subscribe to `t` (remote-access.md §7.4).
#[must_use]
pub fn topic_allowed(rp: &RemotePrincipal, t: &str) -> bool {
    if t == topic::ALL {
        return rp.role >= Role::Admin && rp.project_ids.is_empty();
    }
    match t.split_once(':') {
        Some(("lsp", _)) => false,
        Some(("terminal", _)) => rp.remote_terminal,
        Some(("project", id)) => rp.project_ids.is_empty() || rp.project_ids.iter().any(|p| p.to_string() == id),
        _ => true,
    }
}

/// Whether an event may be delivered to a remote device (defence in depth
/// behind [`topic_allowed`]).
#[must_use]
pub fn event_visible(rp: &RemotePrincipal, event: &Event) -> bool {
    if let Some(p) = event.project_id {
        if !rp.project_ids.is_empty() && !rp.project_ids.contains(&p) {
            return false;
        }
    }
    let topics = topic::event_topics(event);
    if topics.iter().any(|t| t.starts_with("lsp:")) {
        return false;
    }
    if !rp.remote_terminal && topics.iter().any(|t| t.starts_with("terminal:")) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::ProjectId;

    fn rp(role: Role) -> RemotePrincipal {
        RemotePrincipal { device_id: DeviceId::new(), role, remote_terminal: false, project_ids: vec![] }
    }

    fn denied(rp: &RemotePrincipal, m: &str) -> bool {
        check_static(rp, m, &Value::Null).is_err()
    }

    #[test]
    fn policy_matrix() {
        let (v, o, a) = (rp(Role::Viewer), rp(Role::Operator), rp(Role::Admin));
        // (method, viewer allowed, operator allowed, admin allowed)
        let matrix = [
            ("projects.list", true, true, true),
            ("tasks.list", true, true, true),
            ("tasks.create", false, true, true),
            ("files.write", false, true, true),
            ("agents.spawn", false, true, true),
            ("emergency.stopAll", true, true, true),
            ("agents.stop", true, true, true),
            ("remote.disable", true, true, true),
            ("remote.revokeAllSessions", true, true, true),
            ("devices.list", true, true, true),
            ("devices.revoke", false, false, true),
            ("audit.list", false, false, true),
            ("remote.enable", false, false, false),
            ("devices.startPairing", false, false, false),
            ("devices.approve", false, false, false),
            ("secrets.list", false, false, false),
            ("providers.setCredential", false, false, false),
            ("sync.configure", false, false, false),
            ("emergency.reset", false, false, false),
            ("nope.unknown", false, false, false),
            ("terminal.open", false, false, false), // remoteTerminal off
        ];
        for (m, ev, eo, ea) in matrix {
            assert_eq!(!denied(&v, m), ev, "viewer {m}");
            assert_eq!(!denied(&o, m), eo, "operator {m}");
            assert_eq!(!denied(&a, m), ea, "admin {m}");
        }
        let mut t = rp(Role::Operator);
        t.remote_terminal = true;
        assert!(!denied(&t, "terminal.open"));
    }

    #[test]
    fn project_scoping() {
        let allowed = ProjectId::new();
        let mut r = rp(Role::Operator);
        r.project_ids = vec![allowed];
        assert!(check_static(&r, "tasks.list", &serde_json::json!({ "projectId": allowed.to_string() })).is_ok());
        assert!(
            check_static(&r, "tasks.list", &serde_json::json!({ "projectId": ProjectId::new().to_string() })).is_err()
        );
        assert!(topic_allowed(&r, &topic::project(allowed)));
        assert!(!topic_allowed(&r, &topic::project(ProjectId::new())));
        assert!(!topic_allowed(&r, "*"));
        assert!(!topic_allowed(&r, &format!("lsp:{}", ProjectId::new())));
        assert!(!topic_allowed(&r, &format!("terminal:{}", ProjectId::new())));
        assert!(topic_allowed(&r, "global"));
    }

    #[test]
    fn rate_limits_per_class() {
        let l = RateLimiter::default();
        let d = DeviceId::new();
        let t0 = Instant::now();
        for _ in 0..5 {
            l.check_at(d, RateClass::Expensive, t0).unwrap();
        }
        let retry = l.check_at(d, RateClass::Expensive, t0).unwrap_err();
        assert!(retry > Duration::from_millis(900) && retry <= Duration::from_secs(1));
        l.check_at(d, RateClass::Expensive, t0 + Duration::from_secs(1)).unwrap();
        for _ in 0..10 {
            l.check_at(d, RateClass::Write, t0).unwrap();
        }
        assert!(l.check_at(d, RateClass::Write, t0).is_err());
        // other device unaffected; emergency never throttled
        l.check_at(DeviceId::new(), RateClass::Write, t0).unwrap();
        for _ in 0..1000 {
            l.check_at(d, RateClass::Emergency, t0).unwrap();
        }
        for _ in 0..50 {
            l.check_at(d, RateClass::Read, t0).unwrap();
        }
        assert!(l.check_at(d, RateClass::Read, t0).is_err());
    }

    #[test]
    fn params_hash_ignores_step_up_and_key_order() {
        let a = serde_json::json!({ "a": 1, "b": 2, "stepUp": { "x": 1 } });
        let b: Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        assert_eq!(params_hash(&a), params_hash(&b));
    }
}

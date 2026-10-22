//! Lease epochs, fencing (both directions) and expiry (Phase 6, SR-81). **Owner: WP-C.**
//!
//! * [`LeaseConfig`] — TTL / heartbeat timings (`ORKESTR_NODE_LEASE_TTL_MS`,
//!   `ORKESTR_NODE_HEARTBEAT_MS`);
//! * [`LeaseBook`] — the coordinator's view: current epoch + owner + lease end
//!   per job, renewals for the next heartbeat, expiry detection, epoch fencing
//!   of incoming progress;
//! * [`PolicySigner`] / [`PolicyVerifier`] — the coordinator signs each offer's
//!   `PolicySnapshot` with its P-256 CIK; the worker verifies with the
//!   coordinator's public key from the pairing record (fail closed).

use std::collections::HashMap;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use orkestr_domain::{JobId, LeaseEpoch, NodeId, Timestamp};
use orkestr_node::exec::policy_signing_bytes;
use orkestr_node::profile::{EnvSource, HEARTBEAT_ENV, LEASE_TTL_ENV};
use orkestr_protocol::methods::jobs::PolicySnapshot;
use orkestr_protocol::methods::nodes::LeaseRenewal;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use parking_lot::Mutex;

/// Lease timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseConfig {
    /// Lease TTL (default 30 s).
    pub ttl: Duration,
    /// Heartbeat interval (default 5 s).
    pub heartbeat: Duration,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self { ttl: Duration::from_secs(30), heartbeat: Duration::from_secs(5) }
    }
}

impl LeaseConfig {
    /// Defaults overridden by `ORKESTR_NODE_LEASE_TTL_MS` / `ORKESTR_NODE_HEARTBEAT_MS`.
    #[must_use]
    pub fn from_env(env: &EnvSource) -> Self {
        let ms = |k: &str| env.get(k).and_then(|v| v.trim().parse::<u64>().ok()).filter(|v| *v > 0);
        let d = Self::default();
        Self {
            ttl: ms(LEASE_TTL_ENV).map_or(d.ttl, Duration::from_millis),
            heartbeat: ms(HEARTBEAT_ENV).map_or(d.heartbeat, Duration::from_millis),
        }
    }

    /// Lease end for a lease granted / renewed at `now`.
    #[must_use]
    pub fn until(&self, now: Timestamp) -> Timestamp {
        now + chrono::Duration::from_std(self.ttl).unwrap_or_else(|_| chrono::Duration::seconds(30))
    }
}

/// One lease on the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// Owner node.
    pub node_id: NodeId,
    /// Current epoch.
    pub epoch: LeaseEpoch,
    /// Lease end.
    pub until: Timestamp,
}

/// Coordinator lease table (in memory; the persisted epoch lives in the job store).
#[derive(Debug, Default)]
pub struct LeaseBook {
    leases: Mutex<HashMap<JobId, Lease>>,
    /// Highest epoch ever issued per job (survives lease release).
    epochs: Mutex<HashMap<JobId, LeaseEpoch>>,
}

impl LeaseBook {
    /// Empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues the next epoch for `job` (never reused, SR-81).
    pub fn next_epoch(&self, job: JobId, persisted: LeaseEpoch) -> LeaseEpoch {
        let mut e = self.epochs.lock();
        let cur = e.get(&job).copied().unwrap_or_default().max(persisted);
        let next = cur.next();
        e.insert(job, next);
        next
    }

    /// Records an epoch issued elsewhere (e.g. `jobs.submit` assigned epoch 1).
    pub fn observe_epoch(&self, job: JobId, epoch: LeaseEpoch) {
        self.epochs.lock().entry(job).and_modify(|e| *e = (*e).max(epoch)).or_insert(epoch);
    }

    /// Grants (or replaces) the lease of `job`.
    pub fn grant(&self, job: JobId, lease: Lease) {
        self.epochs.lock().entry(job).and_modify(|e| *e = (*e).max(lease.epoch)).or_insert(lease.epoch);
        self.leases.lock().insert(job, lease);
    }

    /// Drops the lease of `job` (finished / cancelled / migrating).
    pub fn release(&self, job: JobId) -> Option<Lease> {
        self.leases.lock().remove(&job)
    }

    /// Current lease of `job`.
    #[must_use]
    pub fn get(&self, job: JobId) -> Option<Lease> {
        self.leases.lock().get(&job).copied()
    }

    /// Fencing of incoming job-side messages: only the current epoch from the
    /// current owner is accepted.
    #[must_use]
    pub fn is_current(&self, job: JobId, node: NodeId, epoch: LeaseEpoch) -> bool {
        self.get(job).is_some_and(|l| l.epoch == epoch && l.node_id == node)
    }

    /// Renewals to send with the next heartbeat to `node` (new end = `now + ttl`).
    #[must_use]
    pub fn renewals_for(&self, node: NodeId, now: Timestamp, cfg: &LeaseConfig) -> Vec<LeaseRenewal> {
        let until = cfg.until(now);
        let mut out: Vec<LeaseRenewal> = self
            .leases
            .lock()
            .iter()
            .filter(|(_, l)| l.node_id == node)
            .map(|(job, l)| LeaseRenewal { job_id: *job, epoch: l.epoch, lease_until: until })
            .collect();
        out.sort_by_key(|r| r.job_id.to_string());
        out
    }

    /// Commits renewals after the heartbeat round trip succeeded (renewals
    /// whose epoch is no longer current are ignored).
    pub fn commit_renewals(&self, node: NodeId, renewals: &[LeaseRenewal]) {
        let mut leases = self.leases.lock();
        for r in renewals {
            if let Some(l) = leases.get_mut(&r.job_id) {
                if l.node_id == node && l.epoch == r.epoch && r.lease_until > l.until {
                    l.until = r.lease_until;
                }
            }
        }
    }

    /// Jobs whose lease ended before `now`.
    #[must_use]
    pub fn expired(&self, now: Timestamp) -> Vec<(JobId, Lease)> {
        self.leases.lock().iter().filter(|(_, l)| l.until < now).map(|(j, l)| (*j, *l)).collect()
    }

    /// All leases held by `node`.
    #[must_use]
    pub fn held_by(&self, node: NodeId) -> Vec<(JobId, Lease)> {
        self.leases.lock().iter().filter(|(_, l)| l.node_id == node).map(|(j, l)| (*j, *l)).collect()
    }

    /// Every lease.
    #[must_use]
    pub fn all(&self) -> Vec<(JobId, Lease)> {
        self.leases.lock().iter().map(|(j, l)| (*j, *l)).collect()
    }
}

/// Coordinator side: signs policy snapshots with the CIK.
#[derive(Clone)]
pub struct PolicySigner {
    key: SigningKey,
}

impl std::fmt::Debug for PolicySigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicySigner").finish_non_exhaustive()
    }
}

impl PolicySigner {
    /// Wraps the coordinator CIK.
    #[must_use]
    pub fn new(key: SigningKey) -> Self {
        Self { key }
    }

    /// The verifying (public) key to hand to workers.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        *self.key.verifying_key()
    }

    /// Signs `policy` for `job` at `epoch` (sets `policy.sig`, P1363 base64url).
    pub fn sign(&self, job: JobId, epoch: LeaseEpoch, policy: &mut PolicySnapshot) {
        policy.sig.clear();
        let sig: Signature = self.key.sign(&policy_signing_bytes(job, epoch, policy));
        policy.sig = URL_SAFE_NO_PAD.encode(sig.to_bytes());
    }
}

/// Worker side: the coordinator keys this worker trusts.
#[derive(Debug, Default)]
pub struct PolicyVerifier {
    keys: Mutex<HashMap<NodeId, VerifyingKey>>,
}

impl PolicyVerifier {
    /// Trusts `key` as the CIK of coordinator `node` (set from the pairing record).
    pub fn trust(&self, node: NodeId, key: VerifyingKey) {
        self.keys.lock().insert(node, key);
    }

    /// Forgets a coordinator (revocation).
    pub fn untrust(&self, node: NodeId) {
        self.keys.lock().remove(&node);
    }

    /// Verifies the snapshot signature of an offer from `coordinator`.
    ///
    /// # Errors
    /// Unknown coordinator, malformed or wrong signature (fail closed).
    pub fn verify(
        &self,
        coordinator: NodeId,
        job: JobId,
        epoch: LeaseEpoch,
        policy: &PolicySnapshot,
    ) -> Result<(), String> {
        let key = self.keys.lock().get(&coordinator).copied().ok_or("untrusted coordinator")?;
        let raw = URL_SAFE_NO_PAD.decode(policy.sig.as_bytes()).map_err(|_| "malformed policy signature")?;
        let sig = Signature::from_slice(&raw).map_err(|_| "malformed policy signature")?;
        key.verify(&policy_signing_bytes(job, epoch, policy), &sig).map_err(|_| "bad policy signature".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_policy_verifies_and_is_bound() {
        let signer = PolicySigner::new(SigningKey::random(&mut rand_core06::OsRng));
        let verifier = PolicyVerifier::default();
        let coord = NodeId::new();
        let job = JobId::new();
        let mut p = PolicySnapshot { approval_mode: "ask".into(), ..PolicySnapshot::default() };
        signer.sign(job, LeaseEpoch(3), &mut p);
        assert!(verifier.verify(coord, job, LeaseEpoch(3), &p).is_err(), "untrusted coordinator");
        verifier.trust(coord, signer.verifying_key());
        verifier.verify(coord, job, LeaseEpoch(3), &p).unwrap();
        assert!(verifier.verify(coord, job, LeaseEpoch(4), &p).is_err(), "epoch bound");
        let mut tampered = p.clone();
        tampered.approval_mode = "fullAuto".into();
        assert!(verifier.verify(coord, job, LeaseEpoch(3), &tampered).is_err());
    }

    #[test]
    fn epochs_renewals_expiry() {
        let book = LeaseBook::new();
        let (job, a, b) = (JobId::new(), NodeId::new(), NodeId::new());
        let cfg = LeaseConfig { ttl: Duration::from_secs(10), heartbeat: Duration::from_secs(1) };
        let t0 = chrono::Utc::now();
        let e1 = book.next_epoch(job, LeaseEpoch(0));
        assert_eq!(e1, LeaseEpoch(1));
        book.grant(job, Lease { node_id: a, epoch: e1, until: cfg.until(t0) });
        assert!(book.is_current(job, a, e1));
        assert!(!book.is_current(job, b, e1));
        let r = book.renewals_for(a, t0 + chrono::Duration::seconds(5), &cfg);
        assert_eq!(r.len(), 1);
        book.commit_renewals(a, &r);
        assert!(book.expired(t0 + chrono::Duration::seconds(12)).is_empty());
        assert_eq!(book.expired(t0 + chrono::Duration::seconds(16)).len(), 1);
        let e2 = book.next_epoch(job, LeaseEpoch(0));
        assert_eq!(e2, LeaseEpoch(2));
        book.grant(job, Lease { node_id: b, epoch: e2, until: cfg.until(t0) });
        assert!(!book.is_current(job, a, e1), "old epoch fenced");
        book.commit_renewals(a, &r);
        assert_eq!(book.get(job).unwrap().node_id, b);
    }
}

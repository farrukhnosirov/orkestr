//! Version vectors (`sync-protocol.md` §5.1) and a hybrid logical clock (§5.2).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Causal relation of two version vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Causality {
    /// Same history.
    Equal,
    /// `self` happened before `other`.
    Before,
    /// `self` happened after `other`.
    After,
    /// Neither dominates.
    Concurrent,
}

/// A version vector: device id → counter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionVector(pub BTreeMap<String, u64>);

impl VersionVector {
    /// Increments `me`'s counter.
    pub fn bump(&mut self, me: &str) {
        *self.0.entry(me.to_owned()).or_insert(0) += 1;
    }

    /// Pointwise max.
    pub fn merge(&mut self, other: &Self) {
        for (k, v) in &other.0 {
            let e = self.0.entry(k.clone()).or_insert(0);
            *e = (*e).max(*v);
        }
    }

    /// Counter of a device (0 when absent).
    #[must_use]
    pub fn get(&self, device: &str) -> u64 {
        self.0.get(device).copied().unwrap_or(0)
    }

    /// Compares two vectors (missing entries count as 0).
    #[must_use]
    pub fn compare(&self, other: &Self) -> Causality {
        let mut less = false;
        let mut greater = false;
        for k in self.0.keys().chain(other.0.keys()) {
            match self.get(k).cmp(&other.get(k)) {
                Ordering::Less => less = true,
                Ordering::Greater => greater = true,
                Ordering::Equal => {}
            }
        }
        match (less, greater) {
            (false, false) => Causality::Equal,
            (true, false) => Causality::Before,
            (false, true) => Causality::After,
            (true, true) => Causality::Concurrent,
        }
    }

    /// Canonical text form used inside signatures.
    #[must_use]
    pub fn canonical(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
    }
}

/// A hybrid logical clock producing totally ordered, lexicographically
/// comparable stamps `wall_ms(16) - logical(8) - device`.
#[derive(Debug, Clone, Default)]
pub struct Hlc {
    last_ms: u64,
    logical: u64,
}

impl Hlc {
    /// Next stamp for `device`, never smaller than `observed` (a stamp seen before).
    pub fn tick(&mut self, device: &str, observed: Option<&str>) -> String {
        let wall = u64::try_from(orkestr_domain::now().timestamp_millis()).unwrap_or(0);
        if let Some((ms, lg)) = observed.and_then(parse_hlc) {
            if (ms, lg) > (self.last_ms, self.logical) {
                self.last_ms = ms;
                self.logical = lg;
            }
        }
        if wall > self.last_ms {
            self.last_ms = wall;
            self.logical = 0;
        } else {
            self.logical += 1;
        }
        format!("{:016}-{:08}-{device}", self.last_ms, self.logical)
    }
}

fn parse_hlc(s: &str) -> Option<(u64, u64)> {
    let mut it = s.splitn(3, '-');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vv(pairs: &[(&str, u64)]) -> VersionVector {
        VersionVector(pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect())
    }

    fn samples() -> Vec<VersionVector> {
        vec![
            vv(&[]),
            vv(&[("a", 1)]),
            vv(&[("a", 2)]),
            vv(&[("b", 1)]),
            vv(&[("a", 1), ("b", 1)]),
            vv(&[("a", 3), ("b", 0)]),
            vv(&[("a", 2), ("b", 5), ("c", 1)]),
        ]
    }

    fn merged(a: &VersionVector, b: &VersionVector) -> VersionVector {
        let mut m = a.clone();
        m.merge(b);
        m
    }

    #[test]
    fn merge_laws() {
        let s = samples();
        for a in &s {
            // idempotent
            assert_eq!(merged(a, a).compare(a), Causality::Equal);
            for b in &s {
                // commutative
                assert_eq!(merged(a, b).compare(&merged(b, a)), Causality::Equal);
                // upper bound
                assert!(matches!(merged(a, b).compare(a), Causality::After | Causality::Equal));
                for c in &s {
                    // associative
                    assert_eq!(merged(&merged(a, b), c).compare(&merged(a, &merged(b, c))), Causality::Equal);
                }
            }
        }
    }

    #[test]
    fn compare_is_antisymmetric_and_bump_advances() {
        let s = samples();
        for a in &s {
            for b in &s {
                let expected = match a.compare(b) {
                    Causality::Before => Causality::After,
                    Causality::After => Causality::Before,
                    x => x,
                };
                assert_eq!(b.compare(a), expected);
            }
            let mut n = a.clone();
            n.bump("z");
            assert_eq!(n.compare(a), Causality::After);
        }
        assert_eq!(vv(&[("a", 1)]).compare(&vv(&[("b", 1)])), Causality::Concurrent);
        assert_eq!(vv(&[("a", 0)]).compare(&vv(&[])), Causality::Equal);
    }

    #[test]
    fn hlc_is_monotonic() {
        let mut h = Hlc::default();
        let a = h.tick("d", None);
        let b = h.tick("d", None);
        assert!(b > a);
        let far = format!("{:016}-{:08}-x", u64::MAX / 2, 3);
        let c = h.tick("d", Some(&far));
        assert!(c > far);
    }
}

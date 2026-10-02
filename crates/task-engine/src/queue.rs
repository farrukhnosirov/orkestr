//! Priority queue of `Ready` tasks (§11, task-state-machine.md §7.1).
//!
//! Order: effective priority (Critical > High > Normal > Low > Background)
//! descending, then FIFO (enqueue time, then insertion order).
//!
//! **Aging** prevents starvation: a waiting entry gains one priority level per
//! [`AgingPolicy::interval`], capped at `High` (aging never creates
//! `Critical`). Entries are re-ranked lazily at read time against the `now`
//! passed in, so the queue is fully deterministic in tests.

use chrono::Duration;
use orkestr_domain::{TaskId, TaskPriority, Timestamp};

/// Aging configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgingPolicy {
    /// Waiting time per level gained. `Duration::zero()` disables aging.
    pub interval: Duration,
    /// Highest level aging can reach.
    pub cap: TaskPriority,
}

impl Default for AgingPolicy {
    fn default() -> Self {
        Self { interval: Duration::minutes(20), cap: TaskPriority::High }
    }
}

impl AgingPolicy {
    /// No aging.
    #[must_use]
    pub fn disabled() -> Self {
        Self { interval: Duration::zero(), cap: TaskPriority::High }
    }
}

fn level(p: TaskPriority) -> u8 {
    match p {
        TaskPriority::Background => 0,
        TaskPriority::Low => 1,
        TaskPriority::Normal => 2,
        TaskPriority::High => 3,
        TaskPriority::Critical => 4,
    }
}

fn from_level(l: u8) -> TaskPriority {
    match l {
        0 => TaskPriority::Background,
        1 => TaskPriority::Low,
        2 => TaskPriority::Normal,
        3 => TaskPriority::High,
        _ => TaskPriority::Critical,
    }
}

/// Effective priority of an entry of base priority `base` enqueued at
/// `enqueued_at`, evaluated at `now`.
#[must_use]
pub fn effective_priority(
    base: TaskPriority,
    enqueued_at: Timestamp,
    now: Timestamp,
    aging: AgingPolicy,
) -> TaskPriority {
    if base >= aging.cap || aging.interval <= Duration::zero() {
        return base;
    }
    let waited = now.signed_duration_since(enqueued_at);
    if waited <= Duration::zero() {
        return base;
    }
    let steps = waited.num_milliseconds() / aging.interval.num_milliseconds().max(1);
    let boosted = i64::from(level(base)).saturating_add(steps).min(i64::from(level(aging.cap)));
    from_level(u8::try_from(boosted).unwrap_or(level(aging.cap)))
}

/// A queued task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueEntry {
    /// Task.
    pub task_id: TaskId,
    /// Base priority.
    pub priority: TaskPriority,
    /// When it entered the queue (`ready_since`).
    pub enqueued_at: Timestamp,
}

/// Priority queue with aging. Duplicate task ids are replaced.
#[derive(Debug, Clone, Default)]
pub struct PriorityQueue {
    entries: Vec<(u64, QueueEntry)>,
    next_seq: u64,
    aging: AgingPolicy,
}

impl PriorityQueue {
    /// Empty queue with an aging policy.
    #[must_use]
    pub fn new(aging: AgingPolicy) -> Self {
        Self { entries: Vec::new(), next_seq: 0, aging }
    }

    /// Builds a queue from entries (insertion order = iteration order).
    #[must_use]
    pub fn from_entries(aging: AgingPolicy, entries: impl IntoIterator<Item = QueueEntry>) -> Self {
        let mut q = Self::new(aging);
        for e in entries {
            q.push(e);
        }
        q
    }

    /// Adds (or replaces) an entry.
    pub fn push(&mut self, entry: QueueEntry) {
        self.remove(entry.task_id);
        self.entries.push((self.next_seq, entry));
        self.next_seq += 1;
    }

    /// Removes a task. Returns whether it was queued.
    pub fn remove(&mut self, task_id: TaskId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(_, e)| e.task_id != task_id);
        before != self.entries.len()
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn sorted(&self, now: Timestamp) -> Vec<(TaskPriority, u64, QueueEntry)> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .map(|(seq, e)| (effective_priority(e.priority, e.enqueued_at, now, self.aging), *seq, *e))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.enqueued_at.cmp(&b.2.enqueued_at)).then(a.1.cmp(&b.1)));
        v
    }

    /// Entries in service order at `now`, with their effective priority.
    #[must_use]
    pub fn ordered(&self, now: Timestamp) -> Vec<(QueueEntry, TaskPriority)> {
        self.sorted(now).into_iter().map(|(p, _, e)| (e, p)).collect()
    }

    /// The next entry at `now` without removing it.
    #[must_use]
    pub fn peek(&self, now: Timestamp) -> Option<QueueEntry> {
        self.sorted(now).first().map(|(_, _, e)| *e)
    }

    /// Removes and returns the next entry at `now`.
    pub fn pop(&mut self, now: Timestamp) -> Option<QueueEntry> {
        let next = self.peek(now)?;
        self.remove(next.task_id);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::now;
    use TaskPriority::*;

    fn entry(p: TaskPriority, at: Timestamp) -> QueueEntry {
        QueueEntry { task_id: TaskId::new(), priority: p, enqueued_at: at }
    }

    #[test]
    fn strict_priority_order_without_aging() {
        let t0 = now();
        let entries = [entry(Low, t0), entry(Critical, t0), entry(Background, t0), entry(Normal, t0), entry(High, t0)];
        let mut q = PriorityQueue::from_entries(AgingPolicy::disabled(), entries);
        let order: Vec<_> = std::iter::from_fn(|| q.pop(t0)).map(|e| e.priority).collect();
        assert_eq!(order, vec![Critical, High, Normal, Low, Background]);
        assert!(q.is_empty());
    }

    #[test]
    fn fifo_within_level() {
        let t0 = now();
        let a = entry(Normal, t0);
        let b = entry(Normal, t0 + Duration::seconds(1));
        let c = entry(Normal, t0 + Duration::seconds(1)); // same time → insertion order
        let mut q = PriorityQueue::from_entries(AgingPolicy::disabled(), [c, b, a]);
        assert_eq!(q.pop(t0).map(|e| e.task_id), Some(a.task_id));
        assert_eq!(q.pop(t0).map(|e| e.task_id), Some(c.task_id));
        assert_eq!(q.pop(t0).map(|e| e.task_id), Some(b.task_id));
    }

    #[test]
    fn aging_boosts_and_caps() {
        let t0 = now();
        let aging = AgingPolicy::default();
        assert_eq!(effective_priority(Low, t0, t0 + Duration::minutes(19), aging), Low);
        assert_eq!(effective_priority(Low, t0, t0 + Duration::minutes(20), aging), Normal);
        assert_eq!(effective_priority(Low, t0, t0 + Duration::minutes(40), aging), High);
        assert_eq!(effective_priority(Background, t0, t0 + Duration::days(30), aging), High, "capped at High");
        assert_eq!(effective_priority(Critical, t0, t0 + Duration::days(30), aging), Critical);
        assert_eq!(effective_priority(Normal, t0, t0 - Duration::minutes(5), aging), Normal, "clock skew");
    }

    #[test]
    fn aging_prevents_starvation() {
        let t0 = now();
        let mut q = PriorityQueue::new(AgingPolicy::default());
        let old_low = entry(Low, t0);
        q.push(old_low);
        // A steady stream of fresh Normal tasks arrives; after 20 minutes the
        // old Low task ranks as Normal and wins by FIFO.
        let fresh = entry(Normal, t0 + Duration::minutes(25));
        q.push(fresh);
        assert_eq!(q.peek(t0 + Duration::minutes(10)).map(|e| e.task_id), Some(fresh.task_id), "before aging");
        assert_eq!(q.peek(t0 + Duration::minutes(25)).map(|e| e.task_id), Some(old_low.task_id), "after aging");
        // Even fresh High work is overtaken once it reached the cap.
        let fresh_high = entry(High, t0 + Duration::minutes(45));
        q.push(fresh_high);
        assert_eq!(q.pop(t0 + Duration::minutes(45)).map(|e| e.task_id), Some(old_low.task_id));
        // Critical always beats aged work.
        let crit = entry(Critical, t0 + Duration::minutes(50));
        q.push(crit);
        assert_eq!(q.pop(t0 + Duration::minutes(50)).map(|e| e.task_id), Some(crit.task_id));
    }

    #[test]
    fn push_replaces_and_remove() {
        let t0 = now();
        let mut q = PriorityQueue::new(AgingPolicy::disabled());
        let mut e = entry(Low, t0);
        q.push(e);
        e.priority = Critical;
        q.push(e);
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek(t0).map(|x| x.priority), Some(Critical));
        assert!(q.remove(e.task_id));
        assert!(!q.remove(e.task_id));
        assert_eq!(q.ordered(t0).len(), 0);
    }
}

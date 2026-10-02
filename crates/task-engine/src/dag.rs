//! Task DAG (§11, task-state-machine.md §6): pure graph algorithms over a
//! snapshot of a project's tasks and dependency edges.
//!
//! * Edges are `task → depends_on` (finish-to-start). `Soft` edges order work
//!   but never block; cycle detection and topological order consider every
//!   edge, readiness and blocking consider only hard edges.
//! * Parent/child (epic decomposition) is a separate relation ([`Dag::subtree`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use orkestr_domain::{DependencyKind, Task, TaskDependency, TaskId, TaskStatus};

/// A node of the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagNode {
    /// Id.
    pub id: TaskId,
    /// Status.
    pub status: TaskStatus,
    /// Weight for the critical path (estimate in minutes, unknown = 1).
    pub weight: u32,
    /// Parent task.
    pub parent: Option<TaskId>,
}

impl DagNode {
    /// Builds a node from a task.
    #[must_use]
    pub fn from_task(t: &Task) -> Self {
        Self { id: t.id, status: t.status, weight: t.estimate_minutes.unwrap_or(1).max(1), parent: t.parent_id }
    }
}

/// Immutable DAG snapshot.
#[derive(Debug, Clone, Default)]
pub struct Dag {
    nodes: BTreeMap<TaskId, DagNode>,
    /// task → prerequisites (with kind).
    deps: BTreeMap<TaskId, BTreeMap<TaskId, DependencyKind>>,
    /// prerequisite → dependents.
    dependents: BTreeMap<TaskId, BTreeSet<TaskId>>,
}

/// How a task should react to the state of its dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A prerequisite reached `Completed`: `Planned` dependents with all hard
    /// dependencies completed are promoted to `Ready`.
    DependencyCompleted,
    /// Anything else (dependency failed/cancelled, edge added/removed): no
    /// promotion of `Planned` tasks (they still need approval).
    Other,
}

/// Decides the automatic status change of a task given the statuses of its
/// **hard** dependencies (§6.2, §6.3):
///
/// * any dependency `Failed`/`Cancelled` → `Planned|Ready` become `Blocked`;
/// * all dependencies `Completed` → `Blocked` becomes `Ready`; `Planned`
///   becomes `Ready` on [`Trigger::DependencyCompleted`] (if it has deps);
/// * some dependency still pending → `Ready` becomes `Blocked`.
#[must_use]
pub fn dependency_reaction(
    status: TaskStatus,
    hard_dep_statuses: &[TaskStatus],
    trigger: Trigger,
) -> Option<(TaskStatus, &'static str)> {
    use TaskStatus::{Blocked, Cancelled, Completed, Failed, Planned, Ready};
    let cancelled = hard_dep_statuses.contains(&Cancelled);
    let failed = hard_dep_statuses.contains(&Failed);
    if cancelled || failed {
        return matches!(status, Planned | Ready)
            .then_some((Blocked, if failed { "dependency_failed" } else { "dependency_cancelled" }));
    }
    let all_done = hard_dep_statuses.iter().all(|s| *s == Completed);
    if all_done {
        return match status {
            Blocked => Some((Ready, "dependencies_completed")),
            Planned if trigger == Trigger::DependencyCompleted && !hard_dep_statuses.is_empty() => {
                Some((Ready, "dependencies_completed"))
            }
            _ => None,
        };
    }
    (status == Ready).then_some((Blocked, "dependency_pending"))
}

impl Dag {
    /// Builds a snapshot. Edges referencing unknown nodes are ignored.
    #[must_use]
    pub fn new(nodes: impl IntoIterator<Item = DagNode>, edges: impl IntoIterator<Item = TaskDependency>) -> Self {
        let nodes: BTreeMap<TaskId, DagNode> = nodes.into_iter().map(|n| (n.id, n)).collect();
        let mut deps: BTreeMap<TaskId, BTreeMap<TaskId, DependencyKind>> = BTreeMap::new();
        let mut dependents: BTreeMap<TaskId, BTreeSet<TaskId>> = BTreeMap::new();
        for e in edges {
            if e.task_id == e.depends_on || !nodes.contains_key(&e.task_id) || !nodes.contains_key(&e.depends_on) {
                continue;
            }
            deps.entry(e.task_id).or_default().insert(e.depends_on, e.kind);
            dependents.entry(e.depends_on).or_default().insert(e.task_id);
        }
        Self { nodes, deps, dependents }
    }

    /// Builds a snapshot from tasks (their `dependencies` are ignored; pass edges).
    #[must_use]
    pub fn from_tasks(tasks: &[Task], edges: &[TaskDependency]) -> Self {
        Self::new(tasks.iter().map(DagNode::from_task), edges.iter().copied())
    }

    /// Node lookup.
    #[must_use]
    pub fn node(&self, id: TaskId) -> Option<&DagNode> {
        self.nodes.get(&id)
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True without nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Prerequisites of a task (all kinds).
    pub fn prerequisites(&self, id: TaskId) -> impl Iterator<Item = (TaskId, DependencyKind)> + '_ {
        self.deps.get(&id).into_iter().flat_map(|m| m.iter().map(|(k, v)| (*k, *v)))
    }

    /// Hard prerequisites of a task.
    pub fn hard_prerequisites(&self, id: TaskId) -> impl Iterator<Item = TaskId> + '_ {
        self.prerequisites(id).filter(|(_, k)| *k == DependencyKind::FinishToStart).map(|(t, _)| t)
    }

    /// Direct dependents of a task (all kinds).
    pub fn dependents(&self, id: TaskId) -> impl Iterator<Item = TaskId> + '_ {
        self.dependents.get(&id).into_iter().flat_map(|s| s.iter().copied())
    }

    /// Statuses of the hard prerequisites of `id`.
    #[must_use]
    pub fn hard_dependency_statuses(&self, id: TaskId) -> Vec<TaskStatus> {
        self.hard_prerequisites(id).filter_map(|d| self.nodes.get(&d).map(|n| n.status)).collect()
    }

    /// Whether every hard prerequisite is `Completed`.
    #[must_use]
    pub fn dependencies_satisfied(&self, id: TaskId) -> bool {
        crate::dependencies_satisfied(&self.hard_dependency_statuses(id))
    }

    /// Path `from → … → to` following prerequisite edges, if any (DFS).
    #[must_use]
    pub fn find_path(&self, from: TaskId, to: TaskId) -> Option<Vec<TaskId>> {
        let mut stack: Vec<(TaskId, Vec<TaskId>)> = vec![(from, vec![from])];
        let mut seen = BTreeSet::new();
        while let Some((cur, path)) = stack.pop() {
            if cur == to {
                return Some(path);
            }
            if !seen.insert(cur) {
                continue;
            }
            for (next, _) in self.prerequisites(cur) {
                if !seen.contains(&next) {
                    let mut p = path.clone();
                    p.push(next);
                    stack.push((next, p));
                }
            }
        }
        None
    }

    /// If adding `task → depends_on` would create a cycle, returns the cycle
    /// as `[task, depends_on, …, task]` (a self-edge returns `[task, task]`).
    #[must_use]
    pub fn cycle_if_added(&self, task: TaskId, depends_on: TaskId) -> Option<Vec<TaskId>> {
        if task == depends_on {
            return Some(vec![task, task]);
        }
        let mut path = self.find_path(depends_on, task)?;
        path.insert(0, task);
        Some(path)
    }

    /// Topological order, prerequisites first; ties broken by id (UUID v7 =
    /// creation order). `Err` carries the nodes left in a cycle.
    pub fn topological_order(&self) -> Result<Vec<TaskId>, Vec<TaskId>> {
        let mut indegree: BTreeMap<TaskId, usize> =
            self.nodes.keys().map(|id| (*id, self.deps.get(id).map_or(0, BTreeMap::len))).collect();
        let mut ready: BTreeSet<TaskId> = indegree.iter().filter(|(_, d)| **d == 0).map(|(id, _)| *id).collect();
        let mut out = Vec::with_capacity(self.nodes.len());
        while let Some(id) = ready.pop_first() {
            out.push(id);
            for dep in self.dependents(id) {
                if let Some(d) = indegree.get_mut(&dep) {
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(dep);
                    }
                }
            }
        }
        if out.len() == self.nodes.len() {
            Ok(out)
        } else {
            let done: BTreeSet<TaskId> = out.into_iter().collect();
            Err(self.nodes.keys().filter(|k| !done.contains(k)).copied().collect())
        }
    }

    /// Tasks in `Planned`, `Ready` or `Blocked` whose hard prerequisites are all
    /// `Completed` — runnable once approved. Sorted by id.
    #[must_use]
    pub fn ready_set(&self) -> Vec<TaskId> {
        self.nodes
            .values()
            .filter(|n| matches!(n.status, TaskStatus::Planned | TaskStatus::Ready | TaskStatus::Blocked))
            .filter(|n| self.dependencies_satisfied(n.id))
            .map(|n| n.id)
            .collect()
    }

    fn remaining_weight(n: &DagNode) -> Option<u32> {
        match n.status {
            TaskStatus::Cancelled => None,
            TaskStatus::Completed => Some(0),
            _ => Some(n.weight),
        }
    }

    /// For every node, the longest remaining weighted chain starting at it and
    /// following dependents (inclusive). Cancelled nodes are excluded (0) and
    /// completed nodes weigh 0. Returns `(length, next node on the chain)`.
    fn downstream(&self) -> BTreeMap<TaskId, (u32, Option<TaskId>)> {
        let mut memo: BTreeMap<TaskId, (u32, Option<TaskId>)> = BTreeMap::new();
        // Reverse topological order ensures dependents are computed first.
        let order = match self.topological_order() {
            Ok(o) => o,
            Err(_) => return self.nodes.keys().map(|k| (*k, (0, None))).collect(),
        };
        for id in order.into_iter().rev() {
            let Some(node) = self.nodes.get(&id) else { continue };
            let Some(w) = Self::remaining_weight(node) else {
                memo.insert(id, (0, None));
                continue;
            };
            let mut best: (u32, Option<TaskId>) = (0, None);
            for d in self.dependents(id) {
                let Some(dn) = self.nodes.get(&d) else { continue };
                if dn.status == TaskStatus::Cancelled {
                    continue;
                }
                let len = memo.get(&d).map_or(0, |v| v.0);
                if best.1.is_none() || len > best.0 {
                    best = (len, Some(d));
                }
            }
            memo.insert(id, (w.saturating_add(best.0), best.1));
        }
        memo
    }

    /// Longest remaining weighted chain starting at each node (inclusive).
    #[must_use]
    pub fn downstream_lengths(&self) -> BTreeMap<TaskId, u32> {
        self.downstream().into_iter().map(|(k, v)| (k, v.0)).collect()
    }

    /// The remaining critical path (longest weighted chain of unfinished
    /// work) and its length. Empty for an empty or cyclic graph.
    #[must_use]
    pub fn critical_path(&self) -> (Vec<TaskId>, u32) {
        let memo = self.downstream();
        let Some((start, (len, _))) =
            memo.iter().filter(|(_, v)| v.0 > 0).max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(b.0.cmp(a.0)))
        else {
            return (Vec::new(), 0);
        };
        let mut path = vec![*start];
        let mut cur = *start;
        while let Some((_, Some(next))) = memo.get(&cur) {
            // Skip trailing zero-weight (completed) nodes.
            if memo.get(next).map_or(0, |v| v.0) == 0 {
                break;
            }
            path.push(*next);
            cur = *next;
        }
        // Drop a leading completed node (weight 0) — it is not remaining work.
        while path.len() > 1 && self.nodes.get(&path[0]).is_some_and(|n| n.status == TaskStatus::Completed) {
            path.remove(0);
        }
        (path, *len)
    }

    /// The task and all its descendants via `parent` (BFS order). Empty if
    /// `root` is unknown.
    #[must_use]
    pub fn subtree(&self, root: TaskId) -> Vec<TaskId> {
        if !self.nodes.contains_key(&root) {
            return Vec::new();
        }
        let mut children: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
        for n in self.nodes.values() {
            if let Some(p) = n.parent {
                children.entry(p).or_default().push(n.id);
            }
        }
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut q = VecDeque::from([root]);
        while let Some(id) = q.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            out.push(id);
            if let Some(cs) = children.get(&id) {
                q.extend(cs.iter().copied());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskStatus::*;

    fn ids(n: usize) -> Vec<TaskId> {
        let mut v: Vec<TaskId> = (0..n).map(|_| TaskId::new()).collect();
        v.sort();
        v
    }

    fn node(id: TaskId, status: TaskStatus, weight: u32) -> DagNode {
        DagNode { id, status, weight, parent: None }
    }

    fn hard(task: TaskId, dep: TaskId) -> TaskDependency {
        TaskDependency { task_id: task, depends_on: dep, kind: DependencyKind::FinishToStart }
    }

    #[test]
    fn cycle_detection_simple_and_self() {
        let t = ids(3);
        let dag = Dag::new(t.iter().map(|i| node(*i, Planned, 1)), [hard(t[1], t[0]), hard(t[2], t[1])]);
        assert_eq!(dag.cycle_if_added(t[0], t[0]), Some(vec![t[0], t[0]]));
        // t0 depends on t2 would close t0 → t2 → t1 → t0.
        assert_eq!(dag.cycle_if_added(t[0], t[2]), Some(vec![t[0], t[2], t[1], t[0]]));
        assert_eq!(dag.cycle_if_added(t[2], t[0]), None, "transitive edge is fine");
    }

    #[test]
    fn cycle_detection_long_chain() {
        let t = ids(500);
        let edges: Vec<_> = t.windows(2).map(|w| hard(w[1], w[0])).collect();
        let dag = Dag::new(t.iter().map(|i| node(*i, Planned, 1)), edges);
        let cycle = dag.cycle_if_added(t[0], t[499]).unwrap();
        assert_eq!(cycle.len(), 501);
        assert_eq!(cycle.first(), cycle.last());
        assert!(dag.cycle_if_added(t[499], t[0]).is_none());
        assert_eq!(dag.topological_order().unwrap(), t);
    }

    #[test]
    fn topological_order_diamond_and_cycle_error() {
        let t = ids(4);
        let edges = [hard(t[1], t[0]), hard(t[2], t[0]), hard(t[3], t[1]), hard(t[3], t[2])];
        let dag = Dag::new(t.iter().map(|i| node(*i, Planned, 1)), edges);
        let order = dag.topological_order().unwrap();
        assert_eq!(order, vec![t[0], t[1], t[2], t[3]]);
        // A cyclic snapshot (e.g. corrupted data) is reported, not looped on.
        let cyclic = Dag::new(t.iter().map(|i| node(*i, Planned, 1)), [hard(t[0], t[1]), hard(t[1], t[0])]);
        let err = cyclic.topological_order().unwrap_err();
        assert_eq!(err, vec![t[0], t[1]]);
        assert_eq!(cyclic.critical_path(), (Vec::new(), 0));
    }

    #[test]
    fn ready_set_uses_hard_edges_only() {
        let t = ids(4);
        let nodes = [node(t[0], Completed, 1), node(t[1], Running, 1), node(t[2], Planned, 1), node(t[3], Blocked, 1)];
        let edges = [
            hard(t[2], t[0]),
            hard(t[3], t[1]),
            TaskDependency { task_id: t[2], depends_on: t[1], kind: DependencyKind::Soft },
        ];
        let dag = Dag::new(nodes, edges);
        assert_eq!(dag.ready_set(), vec![t[2]]);
        assert!(!dag.dependencies_satisfied(t[3]));
    }

    #[test]
    fn critical_path_weighted_and_remaining() {
        let t = ids(5);
        // t0(5) → t1(1) → t3(1); t0 → t2(10) → t3; t4 isolated(3)
        let nodes = [
            node(t[0], Completed, 5),
            node(t[1], Planned, 1),
            node(t[2], Planned, 10),
            node(t[3], Planned, 1),
            node(t[4], Planned, 3),
        ];
        let edges = [hard(t[1], t[0]), hard(t[2], t[0]), hard(t[3], t[1]), hard(t[3], t[2])];
        let dag = Dag::new(nodes, edges);
        let (path, len) = dag.critical_path();
        assert_eq!(len, 11, "completed t0 weighs 0");
        assert_eq!(path, vec![t[2], t[3]]);
        let down = dag.downstream_lengths();
        assert_eq!(down[&t[1]], 2);
        assert_eq!(down[&t[4]], 3);
    }

    #[test]
    fn critical_path_skips_cancelled() {
        let t = ids(3);
        let dag = Dag::new(
            [node(t[0], Planned, 2), node(t[1], Cancelled, 50), node(t[2], Planned, 2)],
            [hard(t[1], t[0]), hard(t[2], t[0])],
        );
        assert_eq!(dag.critical_path(), (vec![t[0], t[2]], 4));
    }

    #[test]
    fn subtree_follows_parents() {
        let t = ids(5);
        let mut nodes: Vec<DagNode> = t.iter().map(|i| node(*i, Planned, 1)).collect();
        nodes[1].parent = Some(t[0]);
        nodes[2].parent = Some(t[0]);
        nodes[3].parent = Some(t[1]);
        let dag = Dag::new(nodes, []);
        assert_eq!(dag.subtree(t[0]), vec![t[0], t[1], t[2], t[3]]);
        assert_eq!(dag.subtree(t[4]), vec![t[4]]);
        assert!(dag.subtree(TaskId::new()).is_empty());
    }

    #[test]
    fn dependency_reactions() {
        use Trigger::*;
        // Promotion.
        assert_eq!(
            dependency_reaction(Planned, &[Completed], DependencyCompleted),
            Some((Ready, "dependencies_completed"))
        );
        assert_eq!(dependency_reaction(Planned, &[Completed], Other), None);
        assert_eq!(dependency_reaction(Planned, &[], DependencyCompleted), None, "no deps → needs approval");
        assert_eq!(
            dependency_reaction(Blocked, &[Completed, Completed], Other),
            Some((Ready, "dependencies_completed"))
        );
        assert_eq!(dependency_reaction(Blocked, &[], Other), Some((Ready, "dependencies_completed")));
        // Blocking.
        assert_eq!(dependency_reaction(Planned, &[Completed, Failed], Other), Some((Blocked, "dependency_failed")));
        assert_eq!(dependency_reaction(Ready, &[Cancelled], Other), Some((Blocked, "dependency_cancelled")));
        assert_eq!(dependency_reaction(Ready, &[Running], Other), Some((Blocked, "dependency_pending")));
        assert_eq!(dependency_reaction(Blocked, &[Failed], Other), None);
        assert_eq!(dependency_reaction(Planned, &[Running], DependencyCompleted), None);
        // In-flight tasks are never touched.
        for s in [Running, Waiting, Review, Testing, Completed, Cancelled, Failed, Interrupted, Recovering] {
            assert_eq!(dependency_reaction(s, &[Failed], Other), None, "{s:?}");
            assert_eq!(dependency_reaction(s, &[Completed], DependencyCompleted), None, "{s:?}");
        }
    }
}

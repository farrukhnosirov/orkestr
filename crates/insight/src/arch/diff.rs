//! Snapshot diffs.

use std::collections::{BTreeMap, BTreeSet};

use orkestr_protocol::methods::architecture::{ArchDiff, ArchNode, ArchNodeChange, ArchitectureMap};

/// Difference from `old` (`None` = empty model) to `new`.
#[must_use]
pub fn diff(old: Option<&ArchitectureMap>, new: &ArchitectureMap) -> ArchDiff {
    let empty_nodes: Vec<ArchNode> = Vec::new();
    let old_nodes: BTreeMap<&str, &ArchNode> =
        old.map_or(&empty_nodes, |m| &m.nodes).iter().map(|n| (n.id.as_str(), n)).collect();
    let new_nodes: BTreeMap<&str, &ArchNode> = new.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut d = ArchDiff {
        from_snapshot_id: old.map(|m| m.snapshot_id.clone()),
        to_snapshot_id: Some(new.snapshot_id.clone()),
        ..ArchDiff::default()
    };
    for (id, n) in &new_nodes {
        match old_nodes.get(id) {
            None => d.added_nodes.push((*n).clone()),
            Some(o) => {
                if let Some(c) = node_change(o, n) {
                    d.changed_nodes.push(c);
                }
            }
        }
    }
    for (id, n) in &old_nodes {
        if !new_nodes.contains_key(id) {
            d.removed_nodes.push((*n).clone());
        }
    }
    let old_edges: BTreeMap<&str, _> =
        old.map(|m| m.edges.iter().map(|e| (e.id.as_str(), e)).collect()).unwrap_or_default();
    let new_edges: BTreeMap<&str, _> = new.edges.iter().map(|e| (e.id.as_str(), e)).collect();
    for (id, e) in &new_edges {
        if !old_edges.contains_key(id) {
            d.added_edges.push((*e).clone());
        }
    }
    for (id, e) in &old_edges {
        if !new_edges.contains_key(id) {
            d.removed_edges.push((*e).clone());
        }
    }
    d
}

fn node_change(o: &ArchNode, n: &ArchNode) -> Option<ArchNodeChange> {
    let mut fields = Vec::new();
    if o.name != n.name {
        fields.push("name");
    }
    if o.path != n.path {
        fields.push("path");
    }
    if o.detail != n.detail {
        fields.push("detail");
    }
    if o.files != n.files {
        fields.push("files");
    }
    if o.file_count != n.file_count {
        fields.push("fileCount");
    }
    if o.tags != n.tags {
        fields.push("tags");
    }
    let (added_routes, removed_routes) = set_diff(&o.routes, &n.routes);
    if !added_routes.is_empty() || !removed_routes.is_empty() {
        fields.push("routes");
    }
    let (added_dependencies, removed_dependencies) = set_diff(&o.dependencies, &n.dependencies);
    if !added_dependencies.is_empty() || !removed_dependencies.is_empty() {
        fields.push("dependencies");
    }
    if fields.is_empty() {
        return None;
    }
    Some(ArchNodeChange {
        id: n.id.clone(),
        kind: n.kind,
        name: n.name.clone(),
        fields: fields.into_iter().map(str::to_owned).collect(),
        added_routes,
        removed_routes,
        added_dependencies,
        removed_dependencies,
    })
}

fn set_diff<T: Ord + Clone>(old: &[T], new: &[T]) -> (Vec<T>, Vec<T>) {
    let o: BTreeSet<&T> = old.iter().collect();
    let n: BTreeSet<&T> = new.iter().collect();
    (n.difference(&o).map(|x| (*x).clone()).collect(), o.difference(&n).map(|x| (*x).clone()).collect())
}

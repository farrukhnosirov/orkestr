//! Three-way merge of stream states (`sync-protocol.md` §6, simplified per ADR-0031):
//! files take the changed side; concurrent edits keep the local version in
//! place and materialize the remote one as a *conflict copy* (nothing lost);
//! modify beats delete; records use last-writer-wins by HLC.

use std::collections::{BTreeMap, BTreeSet};

use crate::manifest::TreeEntry;

/// Entries keyed by path.
pub type Tree = BTreeMap<String, TreeEntry>;

/// A working-tree / record mutation to apply locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Apply {
    /// Write this entry (content from its chunks).
    Write(TreeEntry),
    /// Delete the path.
    Delete(String),
}

/// A conflict detected by the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedConflict {
    /// Path kept with the local version.
    pub path: String,
    /// Where the remote version was written.
    pub copy_path: String,
    /// Local content hash.
    pub local: String,
    /// Remote content hash.
    pub remote: String,
}

/// Merge output.
#[derive(Debug, Clone, Default)]
pub struct MergeOut {
    /// The merged state (what the working tree / records hold afterwards).
    pub tree: Tree,
    /// Local mutations to perform.
    pub apply: Vec<Apply>,
    /// Conflicts.
    pub conflicts: Vec<PlannedConflict>,
}

fn same(a: Option<&TreeEntry>, b: Option<&TreeEntry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.content == b.content && a.exec == b.exec,
        _ => false,
    }
}

/// Conflict copy path: `dir/name.conflict-<device>-<hash8>.ext`.
#[must_use]
pub fn conflict_copy_path(path: &str, remote_device: &str, remote_hash: &str) -> String {
    let (dir, file) = path.rsplit_once('/').map_or(("", path), |(d, f)| (d, f));
    let dev: String = remote_device.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let tag = format!("conflict-{dev}-{}", &remote_hash[..remote_hash.len().min(8)]);
    let name = match file.rfind('.') {
        Some(i) if i > 0 => format!("{}.{tag}{}", &file[..i], &file[i..]),
        _ => format!("{file}.{tag}"),
    };
    if dir.is_empty() {
        name
    } else {
        format!("{dir}/{name}")
    }
}

/// Merges `local` and `remote` against their common `base`.
#[must_use]
pub fn merge(base: &Tree, local: &Tree, remote: &Tree, records: bool, remote_device: &str) -> MergeOut {
    let mut out = MergeOut::default();
    let paths: BTreeSet<&String> = local.keys().chain(remote.keys()).collect();
    for path in paths {
        let (b, l, r) = (base.get(path), local.get(path), remote.get(path));
        if same(l, r) {
            if let Some(l) = l {
                out.tree.insert(path.clone(), l.clone());
            }
        } else if same(l, b) {
            take_remote(&mut out, path, r);
        } else if same(r, b) {
            if let Some(l) = l {
                out.tree.insert(path.clone(), l.clone());
            }
        } else if records {
            let l_hlc = l.and_then(|e| e.hlc.as_deref());
            let r_hlc = r.and_then(|e| e.hlc.as_deref());
            if r_hlc > l_hlc {
                take_remote(&mut out, path, r);
            } else if let Some(l) = l {
                out.tree.insert(path.clone(), l.clone());
            }
        } else {
            match (l, r) {
                // modify beats delete (either direction)
                (None, Some(_)) => take_remote(&mut out, path, r),
                (Some(l), None) => {
                    out.tree.insert(path.clone(), l.clone());
                }
                (Some(l), Some(r)) => {
                    out.tree.insert(path.clone(), l.clone());
                    let copy_path = conflict_copy_path(path, remote_device, &r.content);
                    let copy = TreeEntry { path: copy_path.clone(), ..r.clone() };
                    if local.get(&copy_path).map(|e| &e.content) != Some(&r.content) {
                        out.apply.push(Apply::Write(copy.clone()));
                    }
                    out.tree.insert(copy_path.clone(), copy);
                    out.conflicts.push(PlannedConflict {
                        path: path.clone(),
                        copy_path,
                        local: l.content.clone(),
                        remote: r.content.clone(),
                    });
                }
                (None, None) => {}
            }
        }
    }
    out
}

fn take_remote(out: &mut MergeOut, path: &str, r: Option<&TreeEntry>) {
    match r {
        Some(r) => {
            out.tree.insert(path.to_owned(), r.clone());
            out.apply.push(Apply::Write(r.clone()));
        }
        None => out.apply.push(Apply::Delete(path.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, content: &str) -> TreeEntry {
        TreeEntry { path: path.into(), content: content.into(), size: 1, exec: false, chunks: vec![], hlc: None }
    }

    fn t(items: &[(&str, &str)]) -> Tree {
        items.iter().map(|(p, c)| ((*p).to_owned(), e(p, c))).collect()
    }

    #[test]
    fn merge_matrix() {
        let base = t(&[("same", "1"), ("lchg", "1"), ("rchg", "1"), ("both", "1"), ("ldel", "1"), ("rdel_lmod", "1")]);
        let local = t(&[("same", "1"), ("lchg", "2"), ("rchg", "1"), ("both", "L"), ("rdel_lmod", "2"), ("lnew", "n")]);
        let remote =
            t(&[("same", "1"), ("lchg", "1"), ("rchg", "3"), ("both", "R0000000"), ("ldel", "9"), ("rnew", "m")]);
        let m = merge(&base, &local, &remote, false, "dev-B");
        assert_eq!(m.tree["lchg"].content, "2");
        assert_eq!(m.tree["rchg"].content, "3");
        assert_eq!(m.tree["both"].content, "L");
        assert_eq!(m.tree["ldel"].content, "9", "modify beats delete");
        assert_eq!(m.tree["rdel_lmod"].content, "2", "modify beats delete");
        assert!(m.tree.contains_key("lnew") && m.tree.contains_key("rnew"));
        assert_eq!(m.conflicts.len(), 1);
        let copy = &m.conflicts[0].copy_path;
        assert_eq!(copy, "both.conflict-devB-R0000000");
        assert_eq!(m.tree[copy].content, "R0000000");
        assert!(m.apply.contains(&Apply::Write(e("rchg", "3"))));
        assert!(m.apply.iter().any(|a| matches!(a, Apply::Write(x) if &x.path == copy)));
    }

    #[test]
    fn remote_delete_applies_and_records_are_lww() {
        let base = t(&[("a", "1")]);
        let m = merge(&base, &base, &Tree::new(), false, "d");
        assert_eq!(m.apply, vec![Apply::Delete("a".into())]);
        assert!(m.tree.is_empty());

        let mut l = e("k", "L");
        l.hlc = Some("0001".into());
        let mut r = e("k", "R");
        r.hlc = Some("0002".into());
        let lt: Tree = [("k".to_owned(), l)].into();
        let rt: Tree = [("k".to_owned(), r)].into();
        let m = merge(&Tree::new(), &lt, &rt, true, "d");
        assert_eq!(m.tree["k"].content, "R");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn copy_path_shapes() {
        assert_eq!(conflict_copy_path("src/a.rs", "d_1", "abcdef1234"), "src/a.conflict-d1-abcdef12.rs");
        assert_eq!(conflict_copy_path(".gitignore", "x", "ff"), ".gitignore.conflict-x-ff");
    }
}

//! The workspace crate graph and safe crate-level test-impact analysis.
//!
//! Safety argument (crate granularity): in a Cargo workspace, a crate is the
//! unit of compilation. A crate's test outcomes can only change if that crate's
//! own sources changed, or if one of the crates it depends on (transitively)
//! changed. Therefore selecting *the changed crates plus every crate that
//! transitively depends on them* never drops a test whose result could differ —
//! it is safe by construction. (This is the crate-level variant RustyRTS, ICST
//! 2025, measured at ~99.99% of failure-revealing tests selected.)
//!
//! It is deliberately conservative: it may over-select (e.g. it cannot see that
//! a change touched only a comment), never under-select.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// One workspace member crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    /// Package name (as used by `cargo -p`).
    pub name: String,
    /// Absolute path to the crate's directory (parent of its `Cargo.toml`).
    pub manifest_dir: PathBuf,
    /// Names of *workspace* crates this crate directly depends on.
    pub deps: Vec<String>,
}

/// The workspace as a graph of member crates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceGraph {
    /// Absolute workspace root directory.
    pub root: PathBuf,
    /// Member crates.
    pub members: Vec<Member>,
}

impl WorkspaceGraph {
    /// Construct directly (used by the metadata loader and tests).
    #[must_use]
    pub fn new(root: PathBuf, members: Vec<Member>) -> Self {
        Self { root, members }
    }

    /// Index of a member by name.
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.members.iter().position(|m| m.name == name)
    }

    /// Map an absolute file path to the member crate that owns it, choosing the
    /// crate with the longest matching `manifest_dir` prefix (handles nested
    /// crates correctly).
    #[must_use]
    pub fn crate_for_path(&self, abs_path: &Path) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None; // (member idx, prefix len)
        for (i, m) in self.members.iter().enumerate() {
            if abs_path.starts_with(&m.manifest_dir) {
                let len = m.manifest_dir.as_os_str().len();
                if best.is_none_or(|(_, bl)| len > bl) {
                    best = Some((i, len));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Reverse adjacency: for each member, the set of members that depend on it.
    fn reverse_edges(&self) -> Vec<Vec<usize>> {
        let mut rev = vec![Vec::new(); self.members.len()];
        for (i, m) in self.members.iter().enumerate() {
            for dep in &m.deps {
                if let Some(j) = self.index_of(dep) {
                    rev[j].push(i); // i depends on j  =>  j -> i in reverse
                }
            }
        }
        rev
    }

    /// Given a set of directly-changed member indices, return the full affected
    /// set: the changed members plus every member that transitively depends on
    /// them (BFS over reverse edges). The result includes the seeds.
    #[must_use]
    pub fn affected(&self, changed: &BTreeSet<usize>) -> BTreeSet<usize> {
        let rev = self.reverse_edges();
        let mut seen: BTreeSet<usize> = changed.clone();
        let mut queue: VecDeque<usize> = changed.iter().copied().collect();
        while let Some(n) = queue.pop_front() {
            for &dependent in &rev[n] {
                if seen.insert(dependent) {
                    queue.push_back(dependent);
                }
            }
        }
        seen
    }

    /// All member indices (used when a workspace-wide change forces a full run).
    #[must_use]
    pub fn all_indices(&self) -> BTreeSet<usize> {
        (0..self.members.len()).collect()
    }

    /// Member names for a set of indices, in sorted order.
    #[must_use]
    pub fn names(&self, idxs: &BTreeSet<usize>) -> Vec<String> {
        let mut v: Vec<String> = idxs.iter().map(|&i| self.members[i].name.clone()).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, dir: &str, deps: &[&str]) -> Member {
        Member {
            name: name.into(),
            manifest_dir: PathBuf::from(dir),
            deps: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    // graph:  app -> core -> util ;  cli -> core
    fn graph() -> WorkspaceGraph {
        WorkspaceGraph::new(
            PathBuf::from("/ws"),
            vec![
                member("util", "/ws/crates/util", &[]),
                member("core", "/ws/crates/core", &["util"]),
                member("app", "/ws/crates/app", &["core"]),
                member("cli", "/ws/crates/cli", &["core"]),
            ],
        )
    }

    #[test]
    fn change_in_leaf_affects_all_dependents() {
        let g = graph();
        let changed = BTreeSet::from([g.index_of("util").unwrap()]);
        let affected = g.names(&g.affected(&changed));
        assert_eq!(affected, vec!["app", "cli", "core", "util"]);
    }

    #[test]
    fn change_in_top_affects_only_itself() {
        let g = graph();
        let changed = BTreeSet::from([g.index_of("app").unwrap()]);
        let affected = g.names(&g.affected(&changed));
        assert_eq!(affected, vec!["app"]);
    }

    #[test]
    fn change_in_middle_affects_middle_and_up() {
        let g = graph();
        let changed = BTreeSet::from([g.index_of("core").unwrap()]);
        let affected = g.names(&g.affected(&changed));
        assert_eq!(affected, vec!["app", "cli", "core"]);
    }

    #[test]
    fn longest_prefix_wins_for_nested_crates() {
        let g = WorkspaceGraph::new(
            PathBuf::from("/ws"),
            vec![
                member("outer", "/ws", &[]),
                member("inner", "/ws/crates/inner", &[]),
            ],
        );
        let i = g
            .crate_for_path(Path::new("/ws/crates/inner/src/lib.rs"))
            .unwrap();
        assert_eq!(g.members[i].name, "inner");
        let o = g.crate_for_path(Path::new("/ws/build.rs")).unwrap();
        assert_eq!(g.members[o].name, "outer");
    }

    #[test]
    fn path_outside_workspace_maps_to_nothing() {
        let g = graph();
        assert!(g.crate_for_path(Path::new("/elsewhere/x.rs")).is_none());
    }
}

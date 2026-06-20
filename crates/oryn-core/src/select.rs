//! Turn a set of changed files into a safe test-selection plan.

use crate::graph::WorkspaceGraph;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A safe plan for which crates' tests need to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionPlan {
    /// Crates whose own files changed.
    pub changed_crates: Vec<String>,
    /// Full safe set to test: changed crates + all transitive dependents.
    pub affected_crates: Vec<String>,
    /// Crates safely skipped (their results cannot have changed).
    pub skipped_crates: Vec<String>,
    /// True if a workspace-level change forced a full run.
    pub select_all: bool,
    /// Human-readable explanation.
    pub reason: String,
    /// Number of changed files that belong to no crate (docs, CI, etc.).
    pub ignored_files: usize,
}

impl SelectionPlan {
    /// Whether any crate needs testing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.affected_crates.is_empty()
    }
}

/// Files at the workspace root that change global build behavior, forcing a
/// conservative full run when touched.
fn is_workspace_global(rel: &Path) -> bool {
    matches!(
        rel.to_string_lossy().as_ref(),
        "Cargo.toml"
            | "Cargo.lock"
            | "rust-toolchain"
            | "rust-toolchain.toml"
            | ".cargo/config.toml"
            | ".cargo/config"
    )
}

/// Compute the selection plan for `changed` files (paths relative to the repo
/// root, which is assumed to equal the workspace root or contain it).
#[must_use]
pub fn plan(graph: &WorkspaceGraph, repo_root: &Path, changed: &[PathBuf]) -> SelectionPlan {
    let mut changed_idx: BTreeSet<usize> = BTreeSet::new();
    let mut ignored = 0usize;
    let mut force_all = false;

    for rel in changed {
        if is_workspace_global(rel) {
            force_all = true;
            continue;
        }
        let abs = repo_root.join(rel);
        match graph.crate_for_path(&abs) {
            Some(i) => {
                changed_idx.insert(i);
            }
            None => ignored += 1,
        }
    }

    if force_all {
        let all = graph.all_indices();
        return SelectionPlan {
            changed_crates: graph.names(&changed_idx),
            affected_crates: graph.names(&all),
            skipped_crates: Vec::new(),
            select_all: true,
            reason: "a workspace-level file (Cargo.lock / root Cargo.toml / toolchain / cargo config) changed — running everything to stay safe".into(),
            ignored_files: ignored,
        };
    }

    let affected = graph.affected(&changed_idx);
    let skipped: BTreeSet<usize> = graph.all_indices().difference(&affected).copied().collect();

    let reason = if changed_idx.is_empty() {
        "no crate sources changed — nothing to test".to_string()
    } else {
        format!(
            "{} crate(s) changed; testing them + {} transitive dependent(s)",
            changed_idx.len(),
            affected.len().saturating_sub(changed_idx.len())
        )
    };

    SelectionPlan {
        changed_crates: graph.names(&changed_idx),
        affected_crates: graph.names(&affected),
        skipped_crates: graph.names(&skipped),
        select_all: false,
        reason,
        ignored_files: ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Member;

    fn graph() -> WorkspaceGraph {
        WorkspaceGraph::new(
            PathBuf::from("/ws"),
            vec![
                Member {
                    name: "util".into(),
                    manifest_dir: "/ws/crates/util".into(),
                    deps: vec![],
                },
                Member {
                    name: "core".into(),
                    manifest_dir: "/ws/crates/core".into(),
                    deps: vec!["util".into()],
                },
                Member {
                    name: "cli".into(),
                    manifest_dir: "/ws/crates/cli".into(),
                    deps: vec!["core".into()],
                },
            ],
        )
    }

    #[test]
    fn source_change_selects_affected_only() {
        let g = graph();
        let plan = plan(
            &g,
            Path::new("/ws"),
            &[PathBuf::from("crates/core/src/lib.rs")],
        );
        assert!(!plan.select_all);
        assert_eq!(plan.changed_crates, vec!["core"]);
        assert_eq!(plan.affected_crates, vec!["cli", "core"]);
        assert_eq!(plan.skipped_crates, vec!["util"]);
    }

    #[test]
    fn docs_only_change_selects_nothing() {
        let g = graph();
        let plan = plan(&g, Path::new("/ws"), &[PathBuf::from("README.md")]);
        assert!(plan.is_empty());
        assert_eq!(plan.ignored_files, 1);
        assert!(plan.affected_crates.is_empty());
    }

    #[test]
    fn lockfile_change_forces_full_run() {
        let g = graph();
        let plan = plan(&g, Path::new("/ws"), &[PathBuf::from("Cargo.lock")]);
        assert!(plan.select_all);
        assert_eq!(plan.affected_crates, vec!["cli", "core", "util"]);
        assert!(plan.skipped_crates.is_empty());
    }

    #[test]
    fn leaf_change_selects_everything_downstream() {
        let g = graph();
        let plan = plan(
            &g,
            Path::new("/ws"),
            &[PathBuf::from("crates/util/src/lib.rs")],
        );
        assert_eq!(plan.affected_crates, vec!["cli", "core", "util"]);
        assert!(plan.skipped_crates.is_empty());
    }
}

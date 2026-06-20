//! A pure, render-agnostic snapshot of project state for the TUI (and JSON).
//!
//! Assembles selection, fingerprints, the green cache, and test history into one
//! deterministic structure so the terminal UI is a thin renderer over it (and so
//! it can be unit-tested without a terminal).

use crate::flaky::{self, FlakyReport, TestRuns};
use crate::graph::WorkspaceGraph;
use crate::runner::owning_crate;
use crate::select::SelectionPlan;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A crate's status relative to the current change set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrateStatus {
    /// The crate's own sources changed.
    Changed,
    /// Affected via a dependency change (but not itself changed).
    Affected,
    /// Safely skipped — cannot be affected.
    Skipped,
}

/// Per-crate view row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateView {
    /// Crate name.
    pub name: String,
    /// Short (12-char) fingerprint of its dependency closure.
    pub short_fp: String,
    /// Whether its tests are cached green at the current fingerprint.
    pub cached_green: bool,
    /// Status relative to the current diff.
    pub status: CrateStatus,
    /// Number of tests with recorded history owned by this crate.
    pub tests: usize,
    /// Sum of last-known test durations (ms) for this crate.
    pub total_ms: u64,
}

/// A complete project snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dashboard {
    /// Workspace root, displayed.
    pub workspace_root: String,
    /// Total member crates.
    pub crate_count: usize,
    /// Crates affected by the current diff.
    pub affected_count: usize,
    /// Affected crates that are cached green.
    pub cached_count: usize,
    /// Crates safely skipped.
    pub skipped_count: usize,
    /// Human explanation of the current selection.
    pub plan_reason: String,
    /// Per-crate rows (sorted by name).
    pub crates: Vec<CrateView>,
    /// Flaky analysis over recorded history.
    pub flaky: FlakyReport,
}

impl Dashboard {
    /// Build the snapshot from its inputs.
    #[must_use]
    pub fn build(
        graph: &WorkspaceGraph,
        plan: &SelectionPlan,
        fingerprints: &BTreeMap<String, String>,
        store: &Store,
        level: f64,
    ) -> Self {
        let names = graph.names(&graph.all_indices());

        // Aggregate per-crate test counts/durations from history.
        let mut tests_per: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for (id, rec) in &store.tests {
            if let Some(c) = owning_crate(id, &names) {
                let e = tests_per.entry(c).or_default();
                e.0 += 1;
                e.1 += rec.last_duration_ms.unwrap_or(0);
            }
        }

        let changed: std::collections::BTreeSet<&str> =
            plan.changed_crates.iter().map(String::as_str).collect();
        let affected: std::collections::BTreeSet<&str> =
            plan.affected_crates.iter().map(String::as_str).collect();

        let mut crates = Vec::with_capacity(names.len());
        let mut cached_count = 0;
        for name in &names {
            let fp = fingerprints.get(name).cloned().unwrap_or_default();
            let cached_green = store.is_green(name, &fp);
            let status = if changed.contains(name.as_str()) {
                CrateStatus::Changed
            } else if affected.contains(name.as_str()) {
                CrateStatus::Affected
            } else {
                CrateStatus::Skipped
            };
            if cached_green && status != CrateStatus::Skipped {
                cached_count += 1;
            }
            let (tests, total_ms) = tests_per.get(name).copied().unwrap_or((0, 0));
            crates.push(CrateView {
                name: name.clone(),
                short_fp: fp.chars().take(12).collect(),
                cached_green,
                status,
                tests,
                total_ms,
            });
        }

        let runs: Vec<TestRuns> = store
            .tests
            .iter()
            .map(|(id, r)| TestRuns::new(id.clone(), r.passes, r.fails))
            .collect();
        let flaky = flaky::analyze(&runs, level);

        Self {
            workspace_root: graph.root.display().to_string(),
            crate_count: names.len(),
            affected_count: plan.affected_crates.len(),
            cached_count,
            skipped_count: plan.skipped_crates.len(),
            plan_reason: plan.reason.clone(),
            crates,
            flaky,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Member;
    use std::path::PathBuf;

    fn graph() -> WorkspaceGraph {
        WorkspaceGraph::new(
            PathBuf::from("/ws"),
            vec![
                Member {
                    name: "core".into(),
                    manifest_dir: "/ws/core".into(),
                    deps: vec![],
                },
                Member {
                    name: "cli".into(),
                    manifest_dir: "/ws/cli".into(),
                    deps: vec!["core".into()],
                },
            ],
        )
    }

    #[test]
    fn builds_status_and_cache_view() {
        let g = graph();
        let plan = SelectionPlan {
            changed_crates: vec!["core".into()],
            affected_crates: vec!["cli".into(), "core".into()],
            skipped_crates: vec![],
            select_all: false,
            reason: "core changed".into(),
            ignored_files: 0,
        };
        let fps = BTreeMap::from([
            ("core".to_string(), "abcdef0123456789".to_string()),
            ("cli".to_string(), "1111222233334444".to_string()),
        ]);
        let mut store = Store::default();
        store.record_green("cli", "1111222233334444", 1); // cli cached green
        store.observe_test("core::t1", true, 1, Some(20));

        let d = Dashboard::build(&g, &plan, &fps, &store, 0.95);
        assert_eq!(d.crate_count, 2);
        assert_eq!(d.affected_count, 2);
        let core = d.crates.iter().find(|c| c.name == "core").unwrap();
        let cli = d.crates.iter().find(|c| c.name == "cli").unwrap();
        assert_eq!(core.status, CrateStatus::Changed);
        assert_eq!(cli.status, CrateStatus::Affected);
        assert!(cli.cached_green && !core.cached_green);
        assert_eq!(core.tests, 1);
        assert_eq!(core.total_ms, 20);
        assert_eq!(d.cached_count, 1);
    }
}

//! Deterministic free-text → typed [`Mission`] decomposition.
//!
//! The orchestrator runs on a typed [`Mission`] of [`Subtask`]s with dependency
//! edges, but users describe work in prose. [`decompose`] bridges the two with a
//! **deterministic** keyword-signalled heuristic: identical input always yields an
//! identical mission, so a run is reproducible end-to-end (which is the whole
//! point of "route, don't race" — see [`crate::orchestrator`]).
//!
//! The rules are intentionally simple and explainable, not magic:
//!
//! - A goal that touches the whole codebase gets a leading [`SubtaskKind::LargeContext`]
//!   survey pass that everything else depends on.
//! - Every mission gets exactly one core implementation subtask, whose
//!   [`SubtaskKind`] is chosen from the verbs in the goal (debugging / refactor /
//!   mechanical / diff).
//! - A goal that mentions tests gets a trailing [`SubtaskKind::TestGen`] subtask
//!   that depends on the implementation.
//!
//! This is the real pre-processing the engine consumes — no model call, no
//! network, fully unit-tested.

use crate::orchestrator::task::{Mission, Subtask, SubtaskId, SubtaskKind};

/// True when any of `needles` appears in the (already lowercased) `haystack`.
fn mentions(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Pick the [`SubtaskKind`] for the core implementation subtask from the goal's
/// verbs. Order matters: debugging signals win over refactor over mechanical, and
/// a plain feature request falls through to [`SubtaskKind::DiffEdit`].
fn core_kind(lower: &str) -> SubtaskKind {
    if mentions(
        lower,
        &[
            "bug",
            "fix",
            "broken",
            "fails",
            "failing",
            "race",
            "crash",
            "regression",
            "debug",
            "flaky",
        ],
    ) {
        SubtaskKind::Debugging
    } else if mentions(
        lower,
        &[
            "refactor",
            "clean up",
            "cleanup",
            "restructure",
            "simplify",
            "extract",
            "decompose",
            "untangle",
        ],
    ) {
        SubtaskKind::Refactor
    } else if mentions(
        lower,
        &[
            "rename",
            "move ",
            "reformat",
            "format",
            "bump",
            "version",
            "typo",
            "lint",
            "whitespace",
        ],
    ) {
        SubtaskKind::MechanicalEdit
    } else {
        SubtaskKind::DiffEdit
    }
}

/// Decompose a free-text `goal` into a typed, dependency-ordered [`Mission`].
///
/// Always produces at least one subtask (the core implementation). The result is
/// a valid DAG, so [`Mission::topo_order`] never returns a cycle.
pub fn decompose(mission_id: impl Into<String>, goal: &str) -> Mission {
    let goal = goal.trim();
    let lower = goal.to_lowercase();

    let mut subtasks: Vec<Subtask> = Vec::new();
    let mut core_deps: Vec<SubtaskId> = Vec::new();

    // 1. Codebase-wide survey pass when the goal clearly spans the repo.
    if mentions(
        &lower,
        &[
            "across",
            "codebase",
            "whole repo",
            "entire",
            "audit",
            "everywhere",
            "all files",
            "every file",
            "throughout",
        ],
    ) {
        let id = SubtaskId::new("analyze");
        subtasks.push(Subtask {
            id: id.clone(),
            kind: SubtaskKind::LargeContext,
            summary: format!("Survey the codebase and scope the change for: {goal}"),
            deps: vec![],
        });
        core_deps.push(id);
    }

    // 2. The core implementation subtask (always present).
    let core_id = SubtaskId::new("implement");
    let core_summary = if goal.is_empty() {
        "Implement the requested change.".to_string()
    } else {
        goal.to_string()
    };
    subtasks.push(Subtask {
        id: core_id.clone(),
        kind: core_kind(&lower),
        summary: core_summary,
        deps: core_deps,
    });

    // 3. Test subtask when the goal mentions tests / coverage, depending on impl.
    if mentions(
        &lower,
        &[
            "test", "spec", "coverage", "passing", "pass the", "tdd", "assert",
        ],
    ) {
        subtasks.push(Subtask {
            id: SubtaskId::new("tests"),
            kind: SubtaskKind::TestGen,
            summary: format!("Add or extend automated tests proving: {goal}"),
            deps: vec![core_id],
        });
    }

    Mission {
        id: mission_id.into(),
        goal: goal.to_string(),
        subtasks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_goal_yields_single_implement_subtask() {
        let m = decompose("m", "   ");
        assert_eq!(m.subtasks.len(), 1);
        assert_eq!(m.subtasks[0].id, SubtaskId::new("implement"));
        assert_eq!(m.subtasks[0].kind, SubtaskKind::DiffEdit);
        assert!(m.topo_order().is_ok());
    }

    #[test]
    fn bug_goal_is_debugging_plus_tests() {
        let m = decompose("m", "Fix the flaky token-refresh race so the test passes");
        let kinds: Vec<_> = m
            .subtasks
            .iter()
            .map(|s| (s.id.as_str().to_string(), s.kind))
            .collect();
        assert!(kinds.contains(&("implement".into(), SubtaskKind::Debugging)));
        assert!(kinds.contains(&("tests".into(), SubtaskKind::TestGen)));
        // tests depends on implement
        let tests = m
            .subtasks
            .iter()
            .find(|s| s.id.as_str() == "tests")
            .unwrap();
        assert_eq!(tests.deps, vec![SubtaskId::new("implement")]);
        assert!(m.topo_order().is_ok());
    }

    #[test]
    fn codebase_wide_goal_adds_leading_analysis() {
        let m = decompose(
            "m",
            "Audit the entire codebase for unwrap() and replace across all files",
        );
        assert_eq!(m.subtasks[0].id, SubtaskId::new("analyze"));
        assert_eq!(m.subtasks[0].kind, SubtaskKind::LargeContext);
        // implement depends on analyze
        let implement = m
            .subtasks
            .iter()
            .find(|s| s.id.as_str() == "implement")
            .unwrap();
        assert_eq!(implement.deps, vec![SubtaskId::new("analyze")]);
        let order = m.topo_order().unwrap();
        let pos = |s: &str| order.iter().position(|x| x.as_str() == s).unwrap();
        assert!(pos("analyze") < pos("implement"));
    }

    #[test]
    fn refactor_and_mechanical_verbs_route_correctly() {
        assert_eq!(
            decompose("m", "Refactor the auth client")
                .subtasks
                .last()
                .unwrap()
                .kind,
            SubtaskKind::Refactor
        );
        assert_eq!(
            decompose("m", "Rename the helper module")
                .subtasks
                .last()
                .unwrap()
                .kind,
            SubtaskKind::MechanicalEdit
        );
        assert_eq!(
            decompose("m", "Add a dark-mode toggle")
                .subtasks
                .last()
                .unwrap()
                .kind,
            SubtaskKind::DiffEdit
        );
    }

    #[test]
    fn decomposition_is_deterministic() {
        let goal = "Fix the race and add a regression test across the codebase";
        assert_eq!(decompose("m", goal), decompose("m", goal));
    }
}

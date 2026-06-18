//! Typed task-decomposition model for the orchestration pipeline.
//!
//! A [`Mission`] is a goal broken into typed [`Subtask`]s with explicit
//! dependency edges. [`Mission::topo_order`] yields a deterministic
//! topological ordering (ties broken by [`SubtaskId`] lexicographic order)
//! so that the scheduler always produces the same dispatch sequence for a
//! given input.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// SubtaskId
// ---------------------------------------------------------------------------

/// Stable unique identifier for a [`Subtask`].
///
/// Mirrors the [`crate::ids::EventId`] newtype pattern: wraps a plain
/// `String` so handles cannot be confused with arbitrary strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SubtaskId(String);

impl SubtaskId {
    /// Wrap an existing id string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SubtaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// SubtaskKind
// ---------------------------------------------------------------------------

/// The nature of work a subtask requires.
///
/// Used by the scheduler to route each subtask to the most appropriate agent
/// slot. Derives `Ord` so it can serve as a `BTreeMap` key in later pipeline
/// stages.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskKind {
    /// Straightforward textual or structural edits with no logic change.
    MechanicalEdit,
    /// Writing or extending automated tests.
    TestGen,
    /// Applying a precise, bounded diff to existing code.
    DiffEdit,
    /// Tasks that require reading or holding a large context window.
    LargeContext,
    /// Root-cause analysis and repair of failing behaviour.
    Debugging,
    /// Internal restructuring with no observable behaviour change.
    Refactor,
}

impl SubtaskKind {
    /// Every variant of [`SubtaskKind`] in a deterministic order.
    ///
    /// Used as the canonical iteration source by [`crate::orchestrator::capability`]
    /// so that `resolve_matrix` always visits kinds in the same sequence.
    pub const ALL: [SubtaskKind; 6] = [
        SubtaskKind::MechanicalEdit,
        SubtaskKind::TestGen,
        SubtaskKind::DiffEdit,
        SubtaskKind::LargeContext,
        SubtaskKind::Debugging,
        SubtaskKind::Refactor,
    ];
}

// ---------------------------------------------------------------------------
// Subtask
// ---------------------------------------------------------------------------

/// A single unit of work within a [`Mission`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subtask {
    /// Unique identifier within the parent mission.
    pub id: SubtaskId,
    /// What kind of work this subtask entails.
    pub kind: SubtaskKind,
    /// Human-readable description of the work.
    pub summary: String,
    /// Ids of subtasks that must complete before this one may start.
    pub deps: Vec<SubtaskId>,
}

// ---------------------------------------------------------------------------
// Mission
// ---------------------------------------------------------------------------

/// A top-level goal decomposed into an ordered set of [`Subtask`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mission {
    /// Unique identifier for this mission.
    pub id: String,
    /// High-level description of what the mission should achieve.
    pub goal: String,
    /// The subtasks that together realise the goal.
    pub subtasks: Vec<Subtask>,
}

// ---------------------------------------------------------------------------
// CycleError
// ---------------------------------------------------------------------------

/// Returned by [`Mission::topo_order`] when the dependency graph contains a
/// cycle, which would make execution order undefined.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("dependency cycle detected in mission subtasks")]
pub struct CycleError;

// ---------------------------------------------------------------------------
// Mission::topo_order
// ---------------------------------------------------------------------------

impl Mission {
    /// Compute a deterministic topological ordering of the mission's subtasks.
    ///
    /// Among ready nodes (all dependencies satisfied) at each step, the one
    /// whose [`SubtaskId`] sorts first lexicographically is chosen. This
    /// guarantees a stable, reproducible sequence for any given [`Mission`].
    ///
    /// # Errors
    ///
    /// Returns [`CycleError`] if the dependency graph contains a cycle.
    pub fn topo_order(&self) -> Result<Vec<SubtaskId>, CycleError> {
        // Build adjacency and in-degree maps using BTreeMap for determinism.
        // key = id, value = set of ids that depend on it (successors).
        let mut in_degree: BTreeMap<&SubtaskId, usize> = BTreeMap::new();
        let mut successors: BTreeMap<&SubtaskId, Vec<&SubtaskId>> = BTreeMap::new();

        for subtask in &self.subtasks {
            in_degree.entry(&subtask.id).or_insert(0);
            successors.entry(&subtask.id).or_default();
        }

        for subtask in &self.subtasks {
            for dep in &subtask.deps {
                *in_degree.entry(&subtask.id).or_insert(0) += 1;
                successors.entry(dep).or_default().push(&subtask.id);
            }
        }

        // Seed the ready set with all nodes whose in-degree is zero.
        // BTreeSet gives us the lexicographic-smallest-first pick for free.
        let mut ready: BTreeSet<&SubtaskId> = in_degree
            .iter()
            .filter(|&(_, deg)| *deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut order: Vec<SubtaskId> = Vec::with_capacity(self.subtasks.len());

        while let Some(&next) = ready.iter().next() {
            ready.remove(next);
            order.push(next.clone());

            for &successor in successors.get(next).map(Vec::as_slice).unwrap_or(&[]) {
                let deg = in_degree.get_mut(successor).expect("successor in map");
                *deg -= 1;
                if *deg == 0 {
                    ready.insert(successor);
                }
            }
        }

        if order.len() == self.subtasks.len() {
            Ok(order)
        } else {
            Err(CycleError)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> SubtaskId {
        SubtaskId::new(s)
    }

    fn simple_subtask(id_str: &str, kind: SubtaskKind, deps: Vec<&str>) -> Subtask {
        Subtask {
            id: id(id_str),
            kind,
            summary: format!("summary for {id_str}"),
            deps: deps.into_iter().map(id).collect(),
        }
    }

    // -- SubtaskId -----------------------------------------------------------

    #[test]
    fn subtask_id_new_as_str_display() {
        let sid = SubtaskId::new("task-1");
        assert_eq!(sid.as_str(), "task-1");
        assert_eq!(sid.to_string(), "task-1");
    }

    #[test]
    fn subtask_id_clone_eq() {
        let a = id("x");
        let b = a.clone();
        assert_eq!(a, b);
    }

    // -- SubtaskKind serde round-trips --------------------------------------

    #[test]
    fn subtask_kind_serde_all_variants() {
        let cases = [
            (SubtaskKind::MechanicalEdit, "\"mechanical_edit\""),
            (SubtaskKind::TestGen, "\"test_gen\""),
            (SubtaskKind::DiffEdit, "\"diff_edit\""),
            (SubtaskKind::LargeContext, "\"large_context\""),
            (SubtaskKind::Debugging, "\"debugging\""),
            (SubtaskKind::Refactor, "\"refactor\""),
        ];
        for (variant, expected_json) in cases {
            let json = serde_json::to_string(&variant).expect("serialize");
            assert_eq!(json, expected_json, "serialize {variant:?}");
            let back: SubtaskKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, variant, "deserialize {variant:?}");
        }
    }

    // -- Subtask serde round-trip -------------------------------------------

    #[test]
    fn subtask_serde_round_trip() {
        let subtask = simple_subtask("t1", SubtaskKind::Refactor, vec!["t0"]);
        let json = serde_json::to_string(&subtask).expect("serialize");
        let back: Subtask = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, subtask);
    }

    // -- topo_order: empty mission ------------------------------------------

    #[test]
    fn topo_order_empty_mission() {
        let m = Mission {
            id: "m0".into(),
            goal: "nothing".into(),
            subtasks: vec![],
        };
        assert_eq!(m.topo_order().unwrap(), vec![]);
    }

    // -- topo_order: single node -------------------------------------------

    #[test]
    fn topo_order_single_node() {
        let m = Mission {
            id: "m1".into(),
            goal: "one task".into(),
            subtasks: vec![simple_subtask("a", SubtaskKind::TestGen, vec![])],
        };
        assert_eq!(m.topo_order().unwrap(), vec![id("a")]);
    }

    // -- topo_order: respects deps -----------------------------------------

    #[test]
    fn topo_order_respects_dependencies() {
        // b depends on a → order must be [a, b]
        let m = Mission {
            id: "m2".into(),
            goal: "chain".into(),
            subtasks: vec![
                simple_subtask("b", SubtaskKind::DiffEdit, vec!["a"]),
                simple_subtask("a", SubtaskKind::MechanicalEdit, vec![]),
            ],
        };
        let order = m.topo_order().unwrap();
        let pos_a = order.iter().position(|x| x == &id("a")).unwrap();
        let pos_b = order.iter().position(|x| x == &id("b")).unwrap();
        assert!(pos_a < pos_b, "a must precede b");
    }

    // -- topo_order: independent nodes sorted by id -------------------------

    #[test]
    fn topo_order_independent_nodes_sorted_lexicographically() {
        // Three independent nodes: c, a, b — expect alphabetical order
        let m = Mission {
            id: "m3".into(),
            goal: "parallel".into(),
            subtasks: vec![
                simple_subtask("c", SubtaskKind::LargeContext, vec![]),
                simple_subtask("a", SubtaskKind::Debugging, vec![]),
                simple_subtask("b", SubtaskKind::Refactor, vec![]),
            ],
        };
        assert_eq!(
            m.topo_order().unwrap(),
            vec![id("a"), id("b"), id("c")],
        );
    }

    // -- topo_order: diamond dependency ------------------------------------

    #[test]
    fn topo_order_diamond() {
        // a → b, a → c, b → d, c → d  (diamond)
        // Expected: a, b, c, d  (b before c because b < c lexicographically)
        let m = Mission {
            id: "m4".into(),
            goal: "diamond".into(),
            subtasks: vec![
                simple_subtask("d", SubtaskKind::TestGen, vec!["b", "c"]),
                simple_subtask("c", SubtaskKind::Refactor, vec!["a"]),
                simple_subtask("b", SubtaskKind::DiffEdit, vec!["a"]),
                simple_subtask("a", SubtaskKind::MechanicalEdit, vec![]),
            ],
        };
        let order = m.topo_order().unwrap();
        let pos = |s: &str| order.iter().position(|x| x == &id(s)).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
        // Tie-break: b before c
        assert!(pos("b") < pos("c"));
    }

    // -- topo_order: cycle detection ----------------------------------------

    #[test]
    fn topo_order_detects_direct_cycle() {
        // a → b → a
        let m = Mission {
            id: "m5".into(),
            goal: "cycle".into(),
            subtasks: vec![
                simple_subtask("a", SubtaskKind::Debugging, vec!["b"]),
                simple_subtask("b", SubtaskKind::Debugging, vec!["a"]),
            ],
        };
        assert_eq!(m.topo_order(), Err(CycleError));
    }

    #[test]
    fn topo_order_detects_self_loop() {
        let m = Mission {
            id: "m6".into(),
            goal: "self-loop".into(),
            subtasks: vec![simple_subtask("a", SubtaskKind::Refactor, vec!["a"])],
        };
        assert_eq!(m.topo_order(), Err(CycleError));
    }

    #[test]
    fn topo_order_detects_longer_cycle() {
        // a → b → c → a
        let m = Mission {
            id: "m7".into(),
            goal: "three-cycle".into(),
            subtasks: vec![
                simple_subtask("a", SubtaskKind::MechanicalEdit, vec!["c"]),
                simple_subtask("b", SubtaskKind::TestGen, vec!["a"]),
                simple_subtask("c", SubtaskKind::DiffEdit, vec!["b"]),
            ],
        };
        assert_eq!(m.topo_order(), Err(CycleError));
    }

    // -- CycleError display -------------------------------------------------

    #[test]
    fn cycle_error_display() {
        assert_eq!(
            CycleError.to_string(),
            "dependency cycle detected in mission subtasks",
        );
    }

    // -- Mission serde round-trip ------------------------------------------

    #[test]
    fn mission_serde_round_trip() {
        let m = Mission {
            id: "mission-1".into(),
            goal: "build the thing".into(),
            subtasks: vec![
                simple_subtask("s1", SubtaskKind::MechanicalEdit, vec![]),
                simple_subtask("s2", SubtaskKind::TestGen, vec!["s1"]),
            ],
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Mission = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, m);
    }

    // -- SubtaskKind ordering (BTreeMap key use) ----------------------------

    #[test]
    fn subtask_kind_ord_allows_btreemap_key() {
        let mut map: BTreeMap<SubtaskKind, &str> = BTreeMap::new();
        map.insert(SubtaskKind::Refactor, "refactor");
        map.insert(SubtaskKind::TestGen, "test_gen");
        map.insert(SubtaskKind::Debugging, "debugging");
        assert_eq!(map[&SubtaskKind::Refactor], "refactor");
        assert_eq!(map[&SubtaskKind::TestGen], "test_gen");
    }
}

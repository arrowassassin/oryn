//! Fail-fast test ordering.
//!
//! Large replications (Cheng et al., ISSTA 2024) found a trivial heuristic —
//! **run recently-failed tests first, then the fastest tests first** — matches
//! or beats sophisticated ML/RL prioritization on real long-running suites. We
//! implement exactly that, driven by the persistent [`store`](crate::store)
//! history, so failures surface as early as possible.

use crate::store::TestRecord;
use std::collections::BTreeMap;

/// A ranked test with the signals used to order it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    /// Test id.
    pub id: String,
    /// Whether it failed within its recent window.
    pub recently_failed: bool,
    /// Unix time of last failure (newer ranks earlier).
    pub last_fail: Option<u64>,
    /// Last known duration in ms (smaller ranks earlier).
    pub duration_ms: Option<u64>,
}

/// Order `candidates` fail-fast using recorded history.
///
/// Sort key (ascending position = run earlier):
/// 1. recently-failed before never-recently-failed,
/// 2. more-recent failures first,
/// 3. faster tests first (unknown duration treated as fast so new tests run
///    early and cheaply),
/// 4. id, for determinism.
#[must_use]
pub fn order_tests(history: &BTreeMap<String, TestRecord>, candidates: &[String]) -> Vec<String> {
    let mut ranked: Vec<Ranked> = candidates
        .iter()
        .map(|id| {
            let rec = history.get(id);
            Ranked {
                id: id.clone(),
                recently_failed: rec.is_some_and(TestRecord::recently_failed),
                last_fail: rec.and_then(|r| r.last_fail),
                duration_ms: rec.and_then(|r| r.last_duration_ms),
            }
        })
        .collect();

    ranked.sort_by(|a, b| {
        // 1. recently-failed first (true before false)
        b.recently_failed
            .cmp(&a.recently_failed)
            // 2. most recent failure first
            .then(b.last_fail.cmp(&a.last_fail))
            // 3. fastest first (None == 0 == fastest)
            .then(a.duration_ms.unwrap_or(0).cmp(&b.duration_ms.unwrap_or(0)))
            // 4. stable by id
            .then(a.id.cmp(&b.id))
    });

    ranked.into_iter().map(|r| r.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        passes: u64,
        fails: u64,
        last_fail: Option<u64>,
        dur: Option<u64>,
        recent: &[bool],
    ) -> TestRecord {
        TestRecord {
            passes,
            fails,
            last_fail,
            last_duration_ms: dur,
            recent: recent.to_vec(),
        }
    }

    #[test]
    fn recently_failed_runs_first() {
        let mut h = BTreeMap::new();
        h.insert(
            "slow_green".to_string(),
            rec(10, 0, None, Some(500), &[true, true]),
        );
        h.insert(
            "fast_flaky".to_string(),
            rec(8, 2, Some(100), Some(20), &[true, false]),
        );
        let order = order_tests(&h, &["slow_green".into(), "fast_flaky".into()]);
        assert_eq!(order, vec!["fast_flaky", "slow_green"]);
    }

    #[test]
    fn among_non_failing_fastest_first() {
        let mut h = BTreeMap::new();
        h.insert("a".to_string(), rec(5, 0, None, Some(300), &[true]));
        h.insert("b".to_string(), rec(5, 0, None, Some(50), &[true]));
        let order = order_tests(&h, &["a".into(), "b".into()]);
        assert_eq!(order, vec!["b", "a"]);
    }

    #[test]
    fn more_recent_failure_first() {
        let mut h = BTreeMap::new();
        h.insert("old".to_string(), rec(1, 1, Some(100), Some(10), &[false]));
        h.insert("new".to_string(), rec(1, 1, Some(900), Some(10), &[false]));
        let order = order_tests(&h, &["old".into(), "new".into()]);
        assert_eq!(order, vec!["new", "old"]);
    }

    #[test]
    fn unknown_tests_are_ordered_deterministically() {
        let h = BTreeMap::new();
        let order = order_tests(&h, &["z".into(), "a".into(), "m".into()]);
        assert_eq!(order, vec!["a", "m", "z"]);
    }
}

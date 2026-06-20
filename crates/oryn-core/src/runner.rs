//! Pure helpers for turning a test run's outcomes into store updates.
//!
//! The subprocess orchestration lives in the CLI; the *decisions* live here so
//! they are unit-testable.

use crate::junit::TestOutcome;
use std::collections::BTreeMap;

/// Decide, per crate, whether its test suite passed, from JUnit outcomes.
///
/// A test outcome belongs to crate `c` when its id is exactly `c` or begins with
/// `c::` (the JUnit suite name is the binary id, which starts with the package
/// name). A crate passes iff none of its outcomes failed; a crate with **no**
/// outcomes (no tests, or all filtered) trivially passes.
///
/// Returns a map over exactly `crates`.
#[must_use]
pub fn attribute_crates(outcomes: &[TestOutcome], crates: &[String]) -> BTreeMap<String, bool> {
    let mut result: BTreeMap<String, bool> = crates.iter().map(|c| (c.clone(), true)).collect();
    for o in outcomes {
        if o.passed {
            continue;
        }
        if let Some(c) = owning_crate(&o.id, crates) {
            result.insert(c, false);
        }
    }
    result
}

/// Which of `crates` owns test id `id` (longest matching name wins, so
/// `oryn-core` is preferred over a hypothetical `oryn`).
#[must_use]
pub fn owning_crate(id: &str, crates: &[String]) -> Option<String> {
    crates
        .iter()
        .filter(|c| id == c.as_str() || id.starts_with(&format!("{c}::")))
        .max_by_key(|c| c.len())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(id: &str, passed: bool) -> TestOutcome {
        TestOutcome {
            id: id.into(),
            passed,
            flaky: false,
            duration_ms: Some(1),
        }
    }

    #[test]
    fn crate_with_a_failure_is_not_green() {
        let crates = vec!["oryn-core".to_string(), "oryn-cli".to_string()];
        let outs = vec![
            outcome("oryn-core::graph::tests::ok", true),
            outcome("oryn-core::select::tests::bad", false),
            outcome("oryn-cli::main::ok", true),
        ];
        let r = attribute_crates(&outs, &crates);
        assert!(!r["oryn-core"]);
        assert!(r["oryn-cli"]);
    }

    #[test]
    fn crate_with_no_outcomes_passes() {
        let crates = vec!["a".to_string(), "b".to_string()];
        let outs = vec![outcome("a::x", true)];
        let r = attribute_crates(&outs, &crates);
        assert!(r["a"] && r["b"]);
    }

    #[test]
    fn boundary_aware_attribution() {
        // "oryn-core::x" must not be attributed to a crate named "oryn".
        let crates = vec!["oryn".to_string(), "oryn-core".to_string()];
        assert_eq!(
            owning_crate("oryn-core::x", &crates).as_deref(),
            Some("oryn-core")
        );
        assert_eq!(owning_crate("oryn::y", &crates).as_deref(), Some("oryn"));
    }
}

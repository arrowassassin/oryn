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
/// name). A crate passes iff it had **at least one** observed outcome and none
/// failed. A crate with **no** observed outcomes returns `None` (absence of
/// failure is *not* evidence of passing — we must not record it green), so the
/// caller leaves its cache state untouched.
///
/// Returns a map over exactly `crates`.
#[must_use]
pub fn attribute_crates(
    outcomes: &[TestOutcome],
    crates: &[String],
) -> BTreeMap<String, Option<bool>> {
    // None = no outcome observed yet; Some(true/false) = observed pass/fail.
    let mut result: BTreeMap<String, Option<bool>> =
        crates.iter().map(|c| (c.clone(), None)).collect();
    for o in outcomes {
        if let Some(c) = owning_crate(&o.id, crates) {
            let slot = result.entry(c).or_insert(None);
            // A failure is sticky; a pass only upgrades an unobserved crate.
            *slot = Some(slot.unwrap_or(true) && o.passed);
        }
    }
    result
}

/// Which of `crates` owns test id `id` (longest matching name wins, so
/// `oryn-core` is preferred over a hypothetical `oryn`).
///
/// Cargo normalizes `-` to `_` in target/binary names, so the JUnit suite for
/// package `my-crate` may surface as `my_crate::…`; matching is done on a
/// hyphen-insensitive basis to attribute those correctly.
#[must_use]
pub fn owning_crate(id: &str, crates: &[String]) -> Option<String> {
    let norm = |s: &str| s.replace('-', "_");
    let id_n = norm(id);
    crates
        .iter()
        .filter(|c| {
            let cn = norm(c);
            id_n == cn || id_n.starts_with(&format!("{cn}::"))
        })
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
        assert_eq!(r["oryn-core"], Some(false));
        assert_eq!(r["oryn-cli"], Some(true));
    }

    #[test]
    fn crate_with_no_outcomes_is_unobserved_not_green() {
        // `b` ran no tests → None, so the caller must NOT record it green.
        let crates = vec!["a".to_string(), "b".to_string()];
        let outs = vec![outcome("a::x", true)];
        let r = attribute_crates(&outs, &crates);
        assert_eq!(r["a"], Some(true));
        assert_eq!(r["b"], None);
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

    #[test]
    fn hyphen_underscore_normalized_attribution() {
        // Package `my-crate` surfaces as suite `my_crate::…`.
        let crates = vec!["my-crate".to_string()];
        assert_eq!(
            owning_crate("my_crate::tests::ok", &crates).as_deref(),
            Some("my-crate")
        );
        let outs = vec![outcome("my_crate::tests::bad", false)];
        assert_eq!(attribute_crates(&outs, &crates)["my-crate"], Some(false));
    }
}

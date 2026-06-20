//! Function-level (coverage-based) test selection within a crate.
//!
//! Soundness: coverage records the *full* set of lines a test executes,
//! including callees across files, so a change to any function body a test runs
//! (transitively) is caught by intersecting impacted lines with the test's
//! covered lines. Changes *outside* a function body (a `const`, `static`,
//! `type`, `use`, item-position macro) are not captured by execution traces and
//! can affect a test through non-execution dependencies, so any such change
//! ([`FileImpact::Whole`](crate::fnspans::FileImpact)) conservatively reruns
//! every test in the crate. A test with no coverage record always reruns.

use crate::fnspans::{intersects, FileImpact};
use std::collections::{BTreeMap, BTreeSet};

/// Per-test covered lines: test id → (file → executed lines).
pub type TestCoverage = BTreeMap<String, BTreeMap<String, BTreeSet<usize>>>;

/// Result of function-level selection over one crate's tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Tests that must run.
    pub run: Vec<String>,
    /// Tests safely skipped.
    pub skip: Vec<String>,
}

/// Select which of `tests` must run, given the per-file `impacts` of the change
/// set and the recorded `coverage`.
#[must_use]
pub fn select(
    impacts: &BTreeMap<String, FileImpact>,
    coverage: &TestCoverage,
    tests: &[String],
) -> Selection {
    // A non-function change anywhere in the crate's changed files forces a full
    // crate run (it can affect tests via const/type/use dependencies).
    let any_whole = impacts.values().any(|i| *i == FileImpact::Whole);

    let mut run = Vec::new();
    let mut skip = Vec::new();
    for t in tests {
        let must = any_whole
            || match coverage.get(t) {
                None => true, // no coverage data — be safe
                Some(cov) => impacts
                    .iter()
                    .any(|(file, imp)| cov.get(file).is_some_and(|lines| intersects(imp, lines))),
            };
        if must {
            run.push(t.clone());
        } else {
            skip.push(t.clone());
        }
    }
    Selection { run, skip }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cov(pairs: &[(&str, &[usize])]) -> BTreeMap<String, BTreeSet<usize>> {
        pairs
            .iter()
            .map(|(f, l)| (f.to_string(), l.iter().copied().collect()))
            .collect()
    }

    #[test]
    fn localized_change_runs_only_intersecting_tests() {
        let impacts = BTreeMap::from([(
            "src/lib.rs".to_string(),
            FileImpact::Lines(BTreeSet::from([5, 6, 7, 8])),
        )]);
        let coverage: TestCoverage = BTreeMap::from([
            ("t_b".to_string(), cov(&[("src/lib.rs", &[6])])),
            ("t_a".to_string(), cov(&[("src/lib.rs", &[2])])),
            ("t_x".to_string(), cov(&[("src/other.rs", &[1])])),
        ]);
        let tests = vec![
            "t_a".to_string(),
            "t_b".to_string(),
            "t_x".to_string(),
            "t_new".to_string(),
        ];
        let sel = select(&impacts, &coverage, &tests);
        assert_eq!(sel.run, vec!["t_b", "t_new"]); // t_b intersects, t_new uncovered
        assert_eq!(sel.skip, vec!["t_a", "t_x"]);
    }

    #[test]
    fn whole_file_change_runs_everything() {
        let impacts = BTreeMap::from([("src/lib.rs".to_string(), FileImpact::Whole)]);
        let coverage: TestCoverage =
            BTreeMap::from([("t".to_string(), cov(&[("src/other.rs", &[1])]))]);
        let tests = vec!["t".to_string(), "u".to_string()];
        let sel = select(&impacts, &coverage, &tests);
        assert_eq!(sel.run, tests);
        assert!(sel.skip.is_empty());
    }

    #[test]
    fn no_impacts_skips_all_covered() {
        let impacts = BTreeMap::new();
        let coverage: TestCoverage =
            BTreeMap::from([("t".to_string(), cov(&[("src/lib.rs", &[1])]))]);
        let sel = select(&impacts, &coverage, &["t".to_string()]);
        assert_eq!(sel.skip, vec!["t"]);
        assert!(sel.run.is_empty());
    }
}

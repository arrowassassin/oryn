//! Statistically-rigorous flaky-test scoring.
//!
//! Production runners (cargo-nextest `--retries`, pytest-rerunfailures, Maven
//! Surefire) label a test "flaky" from a naive *fail-then-pass within 2–3
//! reruns* rule. That is statistically unjustified: a test that fails 1% of the
//! time needs ~300 reruns to be seen failing once with 95% confidence.
//!
//! Oryn instead treats reruns as Bernoulli trials and reports a **flake-rate
//! estimate with a Wilson confidence interval** plus the **rerun budget** the
//! statistics actually demand. (Gruber et al., ICST 2021; the rerun-budget
//! identity `n ≥ ln(1−γ)/ln(1−p)`.)

use crate::stats::{wilson_interval, ConfidenceInterval};
use serde::{Deserialize, Serialize};

/// Observed pass/fail counts for one test across repeated runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRuns {
    /// Test identifier.
    pub id: String,
    /// Number of passing runs.
    pub passes: u64,
    /// Number of failing runs.
    pub fails: u64,
}

impl TestRuns {
    /// Convenience constructor.
    pub fn new(id: impl Into<String>, passes: u64, fails: u64) -> Self {
        Self {
            id: id.into(),
            passes,
            fails,
        }
    }

    fn total(&self) -> u64 {
        self.passes + self.fails
    }
}

/// Classification of a test's stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlakyVerdict {
    /// Every observed run passed.
    StablePass,
    /// Every observed run failed.
    StableFail,
    /// Both passes and failures observed — genuinely flaky.
    Flaky,
    /// No runs recorded.
    Unknown,
}

/// Per-test flake score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeScore {
    /// Test id.
    pub id: String,
    /// Total runs observed.
    pub runs: u64,
    /// Failing runs.
    pub fails: u64,
    /// Point estimate of the failure probability.
    pub flake_rate: f64,
    /// Wilson confidence interval on the failure probability.
    pub ci: ConfidenceInterval,
    /// Classification.
    pub verdict: FlakyVerdict,
    /// For flaky tests: reruns needed to reproduce a failure at 95% confidence.
    /// For stable-pass tests: `None` (use `proven_below`).
    pub reruns_to_reproduce_95: Option<u64>,
    /// Upper bound of the CI — "we've only proven the flake rate is below this".
    pub proven_below: f64,
    /// Bayesian (Jeffreys-prior) posterior on the flake rate.
    pub posterior: crate::bayes::Posterior,
}

/// Reruns needed to observe at least one failure with confidence `gamma`, given
/// a per-run failure probability `fail_prob`: `n ≥ ln(1−γ)/ln(1−p)`.
///
/// Returns `None` when `fail_prob <= 0` (a failure can never be surfaced).
#[must_use]
pub fn required_reruns(fail_prob: f64, gamma: f64) -> Option<u64> {
    if fail_prob <= 0.0 {
        return None;
    }
    if fail_prob >= 1.0 {
        return Some(1);
    }
    let gamma = gamma.clamp(0.0, 0.999_999);
    let n = (1.0 - gamma).ln() / (1.0 - fail_prob).ln();
    Some((n.ceil() as u64).max(1))
}

/// Score one test's run history.
#[must_use]
pub fn score(runs: &TestRuns, level: f64) -> FlakeScore {
    let total = runs.total();
    let ci = wilson_interval(runs.fails, total, level);
    let rate = if total == 0 {
        0.0
    } else {
        runs.fails as f64 / total as f64
    };
    let verdict = match (runs.passes, runs.fails) {
        (0, 0) => FlakyVerdict::Unknown,
        (_, 0) => FlakyVerdict::StablePass,
        (0, _) => FlakyVerdict::StableFail,
        (_, _) => FlakyVerdict::Flaky,
    };
    let reruns = if verdict == FlakyVerdict::Flaky {
        required_reruns(rate, 0.95)
    } else {
        None
    };
    FlakeScore {
        id: runs.id.clone(),
        runs: total,
        fails: runs.fails,
        flake_rate: rate,
        ci,
        verdict,
        reruns_to_reproduce_95: reruns,
        proven_below: ci.high,
        posterior: crate::bayes::jeffreys(runs.fails, runs.passes, level),
    }
}

/// Aggregate flaky report over many tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakyReport {
    /// Confidence level used for the intervals.
    pub level: f64,
    /// Per-test scores (input order preserved).
    pub tests: Vec<FlakeScore>,
    /// Number classified flaky.
    pub flaky_count: usize,
    /// Number that always failed.
    pub always_fail_count: usize,
}

/// Score every test and summarize.
#[must_use]
pub fn analyze(tests: &[TestRuns], level: f64) -> FlakyReport {
    let scores: Vec<FlakeScore> = tests.iter().map(|t| score(t, level)).collect();
    let flaky_count = scores
        .iter()
        .filter(|s| s.verdict == FlakyVerdict::Flaky)
        .count();
    let always_fail_count = scores
        .iter()
        .filter(|s| s.verdict == FlakyVerdict::StableFail)
        .count();
    FlakyReport {
        level,
        tests: scores,
        flaky_count,
        always_fail_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerun_budget_matches_closed_form() {
        // 1% failure rate, 95% confidence -> ~299 reruns.
        let n = required_reruns(0.01, 0.95).unwrap();
        assert!((298..=300).contains(&n), "got {n}");
    }

    #[test]
    fn certain_failure_needs_one_rerun() {
        assert_eq!(required_reruns(1.0, 0.95), Some(1));
    }

    #[test]
    fn zero_rate_can_never_surface() {
        assert_eq!(required_reruns(0.0, 0.95), None);
    }

    #[test]
    fn flaky_test_scored_with_ci() {
        let s = score(&TestRuns::new("t", 95, 5), 0.95);
        assert_eq!(s.verdict, FlakyVerdict::Flaky);
        assert!((s.flake_rate - 0.05).abs() < 1e-9);
        assert!(s.ci.low >= 0.0 && s.ci.high <= 1.0 && s.ci.low < 0.05 && s.ci.high > 0.05);
        assert!(s.reruns_to_reproduce_95.is_some());
    }

    #[test]
    fn stable_pass_reports_proven_upper_bound() {
        // 20 passes, 0 fails: not proven perfect — Wilson upper bound > 0.
        let s = score(&TestRuns::new("t", 20, 0), 0.95);
        assert_eq!(s.verdict, FlakyVerdict::StablePass);
        assert_eq!(s.flake_rate, 0.0);
        assert!(s.proven_below > 0.0, "20 clean runs do not prove 0% flake");
        assert!(s.reruns_to_reproduce_95.is_none());
    }

    #[test]
    fn analyze_counts() {
        let tests = vec![
            TestRuns::new("a", 100, 0),
            TestRuns::new("b", 90, 10),
            TestRuns::new("c", 0, 100),
        ];
        let r = analyze(&tests, 0.95);
        assert_eq!(r.flaky_count, 1);
        assert_eq!(r.always_fail_count, 1);
    }
}

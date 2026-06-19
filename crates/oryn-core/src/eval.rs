//! Evaluation with error bars and a regression gate.
//!
//! The point: an eval number without an interval is noise theatre. This module
//! turns raw per-item scores into a report carrying confidence intervals,
//! statistical power, and a *required sample size* — and compares two runs with
//! a paired test so "model B beats model A" is a claim you can defend.

use crate::stats::{
    bootstrap_mean_ci, mean, paired_compare, power_analysis, sample_variance, standard_error,
    wilson_interval, ConfidenceInterval, PairedComparison, PowerAnalysis,
};
use crate::{OrynError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One scored eval item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalItem {
    /// Stable item id (used to pair runs).
    pub id: String,
    /// Score — typically 0/1 for accuracy, but any real value works.
    pub score: f64,
}

impl EvalItem {
    /// Convenience constructor.
    pub fn new(id: impl Into<String>, score: f64) -> Self {
        Self {
            id: id.into(),
            score,
        }
    }
}

/// A named collection of scored items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    /// Human label, e.g. "gpt-x on GPQA".
    pub name: String,
    /// Scored items.
    pub items: Vec<EvalItem>,
}

impl EvalRun {
    /// Construct a run.
    pub fn new(name: impl Into<String>, items: Vec<EvalItem>) -> Self {
        Self {
            name: name.into(),
            items,
        }
    }

    fn scores(&self) -> Vec<f64> {
        self.items.iter().map(|i| i.score).collect()
    }

    fn is_binary(&self) -> bool {
        self.items.iter().all(|i| i.score == 0.0 || i.score == 1.0)
    }
}

/// A statistically-described eval result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Run name.
    pub name: String,
    /// Number of items.
    pub n: usize,
    /// Mean score (the headline metric).
    pub mean: f64,
    /// Sample standard deviation.
    pub std: f64,
    /// Standard error of the mean.
    pub stderr: f64,
    /// Primary CI: Wilson if scores are 0/1, else normal-approx mean CI.
    pub ci: ConfidenceInterval,
    /// Seeded bootstrap CI (robust, distribution-free).
    pub bootstrap_ci: ConfidenceInterval,
    /// Whether scores were detected as binary (accuracy-style).
    pub binary: bool,
    /// Power planning at the configured effect size.
    pub power: PowerAnalysis,
}

/// Default bootstrap seed — a fixed constant so every report is reproducible.
pub const DEFAULT_BOOTSTRAP_SEED: u64 = 0x4F52_594E_5345_4544;

/// Knobs for report generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalConfig {
    /// Confidence level (e.g. 0.95).
    pub level: f64,
    /// Bootstrap resamples.
    pub bootstrap_resamples: usize,
    /// Bootstrap seed (reproducibility).
    pub bootstrap_seed: u64,
    /// Significance level for power planning.
    pub alpha: f64,
    /// Target power for planning.
    pub target_power: f64,
    /// Effect size (Cohen's d) to plan the required-N for.
    pub planned_effect: f64,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            level: 0.95,
            bootstrap_resamples: 2000,
            bootstrap_seed: DEFAULT_BOOTSTRAP_SEED,
            alpha: 0.05,
            target_power: 0.8,
            planned_effect: 0.2,
        }
    }
}

/// Build a statistically-complete report from a run.
///
/// # Errors
/// Returns an error if the run has no items.
pub fn analyze(run: &EvalRun, cfg: &EvalConfig) -> Result<EvalReport> {
    if run.items.is_empty() {
        return Err(OrynError::EmptyInput(format!("eval run '{}'", run.name)));
    }
    let scores = run.scores();
    let n = scores.len();
    let m = mean(&scores);
    let var = sample_variance(&scores);
    let std = var.sqrt();
    let se = standard_error(&scores);
    let binary = run.is_binary();

    let ci = if binary {
        let successes = scores.iter().filter(|&&s| s == 1.0).count() as u64;
        wilson_interval(successes, n as u64, cfg.level)
    } else {
        crate::stats::mean_ci(&scores, cfg.level)
    };
    let bootstrap_ci = bootstrap_mean_ci(
        &scores,
        cfg.level,
        cfg.bootstrap_resamples,
        cfg.bootstrap_seed,
    );
    let power = power_analysis(
        cfg.planned_effect,
        cfg.alpha,
        cfg.target_power,
        n as u64,
        std,
    );

    Ok(EvalReport {
        name: run.name.clone(),
        n,
        mean: m,
        std,
        stderr: se,
        ci,
        bootstrap_ci,
        binary,
        power,
    })
}

/// Verdict from comparing a candidate run against a baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    /// Candidate is significantly better.
    Improved,
    /// No significant difference within the confidence interval.
    NoChange,
    /// Candidate is significantly worse — the gate fails.
    Regressed,
}

/// Result of a regression-gate comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionGate {
    /// Baseline run name.
    pub baseline: String,
    /// Candidate run name.
    pub candidate: String,
    /// Number of items paired by id.
    pub paired_n: usize,
    /// Underlying paired comparison (candidate − baseline).
    pub comparison: PairedComparison,
    /// Verdict.
    pub verdict: GateVerdict,
    /// True if the gate should block (verdict == Regressed).
    pub blocked: bool,
}

/// Pair two runs by item id and decide whether the candidate regressed.
///
/// Only items present in *both* runs are compared. The verdict is significance-
/// based: a regression requires the candidate to be significantly *worse* than
/// the baseline at the configured confidence level.
///
/// # Errors
/// Returns an error if fewer than two items can be paired.
pub fn regression_gate(
    baseline: &EvalRun,
    candidate: &EvalRun,
    level: f64,
) -> Result<RegressionGate> {
    let base: BTreeMap<&str, f64> = baseline
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.score))
        .collect();
    let cand: BTreeMap<&str, f64> = candidate
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.score))
        .collect();

    let mut a = Vec::new(); // candidate
    let mut b = Vec::new(); // baseline
    for (id, &cs) in &cand {
        if let Some(&bs) = base.get(id) {
            a.push(cs);
            b.push(bs);
        }
    }
    if a.len() < 2 {
        return Err(OrynError::LengthMismatch(format!(
            "regression_gate: only {} paired items",
            a.len()
        )));
    }
    let comparison = paired_compare(&a, &b, level)?;
    let verdict = if !comparison.significant {
        GateVerdict::NoChange
    } else if comparison.mean_diff > 0.0 {
        GateVerdict::Improved
    } else {
        GateVerdict::Regressed
    };
    Ok(RegressionGate {
        baseline: baseline.name.clone(),
        candidate: candidate.name.clone(),
        paired_n: a.len(),
        comparison,
        verdict,
        blocked: verdict == GateVerdict::Regressed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_run(name: &str, ones: usize, zeros: usize) -> EvalRun {
        let mut items = Vec::new();
        for i in 0..ones {
            items.push(EvalItem::new(format!("{name}-1-{i}"), 1.0));
        }
        for i in 0..zeros {
            items.push(EvalItem::new(format!("{name}-0-{i}"), 0.0));
        }
        EvalRun::new(name, items)
    }

    #[test]
    fn analyze_binary_uses_wilson_and_reports_ci() {
        let run = binary_run("acc", 70, 30);
        let rep = analyze(&run, &EvalConfig::default()).unwrap();
        assert!(rep.binary);
        assert_eq!(rep.n, 100);
        assert!((rep.mean - 0.7).abs() < 1e-9);
        assert!(rep.ci.low < 0.7 && rep.ci.high > 0.7);
        assert!(rep.ci.margin() > 0.0);
        assert!(rep.power.required_n > 0);
    }

    #[test]
    fn analyze_empty_errors() {
        let run = EvalRun::new("x", vec![]);
        assert!(analyze(&run, &EvalConfig::default()).is_err());
    }

    #[test]
    fn analyze_is_reproducible() {
        let run = binary_run("acc", 55, 45);
        let a = analyze(&run, &EvalConfig::default()).unwrap();
        let b = analyze(&run, &EvalConfig::default()).unwrap();
        assert_eq!(a.bootstrap_ci.low, b.bootstrap_ci.low);
        assert_eq!(a.bootstrap_ci.high, b.bootstrap_ci.high);
    }

    #[test]
    fn gate_blocks_clear_regression() {
        // Same ids, candidate strictly worse.
        let ids: Vec<String> = (0..100).map(|i| format!("q{i}")).collect();
        let baseline = EvalRun::new(
            "base",
            ids.iter().map(|id| EvalItem::new(id, 1.0)).collect(),
        );
        let candidate = EvalRun::new(
            "cand",
            ids.iter().map(|id| EvalItem::new(id, 0.0)).collect(),
        );
        let gate = regression_gate(&baseline, &candidate, 0.95).unwrap();
        assert_eq!(gate.verdict, GateVerdict::Regressed);
        assert!(gate.blocked);
    }

    #[test]
    fn gate_passes_noise() {
        let ids: Vec<String> = (0..100).map(|i| format!("q{i}")).collect();
        let baseline = EvalRun::new(
            "base",
            ids.iter()
                .enumerate()
                .map(|(i, id)| EvalItem::new(id, (i % 2) as f64))
                .collect(),
        );
        let candidate = EvalRun::new(
            "cand",
            ids.iter()
                .enumerate()
                .map(|(i, id)| EvalItem::new(id, (i % 2) as f64))
                .collect(),
        );
        let gate = regression_gate(&baseline, &candidate, 0.95).unwrap();
        assert_eq!(gate.verdict, GateVerdict::NoChange);
        assert!(!gate.blocked);
    }

    #[test]
    fn gate_detects_improvement() {
        let ids: Vec<String> = (0..100).map(|i| format!("q{i}")).collect();
        let baseline = EvalRun::new(
            "base",
            ids.iter().map(|id| EvalItem::new(id, 0.0)).collect(),
        );
        let candidate = EvalRun::new(
            "cand",
            ids.iter().map(|id| EvalItem::new(id, 1.0)).collect(),
        );
        let gate = regression_gate(&baseline, &candidate, 0.95).unwrap();
        assert_eq!(gate.verdict, GateVerdict::Improved);
        assert!(!gate.blocked);
    }
}

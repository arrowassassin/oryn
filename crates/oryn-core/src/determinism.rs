//! Determinism analysis for repeated AI generations.
//!
//! Give it N completions produced from the *same* prompt at temperature 0 and it
//! tells you whether inference is actually reproducible: how many distinct
//! outputs appeared and, if they diverged, at which token. This is the
//! observable symptom of the batch-invariance problem (Thinking Machines Lab,
//! 2025) — the fix for which lives in `oryn-cuda`.

use crate::text::content_hash;
use ahash::AHashMap;
use serde::{Deserialize, Serialize};

/// Result of analyzing a set of repeated generations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeterminismReport {
    /// Total generations analyzed.
    pub total_runs: usize,
    /// Number of distinct outputs (1 == perfectly deterministic).
    pub unique_outputs: usize,
    /// True iff every run produced byte-identical output.
    pub deterministic: bool,
    /// Token index of the first position where outputs disagree, if any
    /// (whitespace tokenization).
    pub divergence_token: Option<usize>,
    /// Fraction of runs equal to the most common output (1.0 == all agree).
    pub majority_fraction: f64,
    /// BLAKE3 fingerprint of each distinct output, sorted, with its count.
    pub distinct: Vec<DistinctOutput>,
}

/// A distinct output value and how often it occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistinctOutput {
    /// BLAKE3 hex fingerprint.
    pub fingerprint: String,
    /// Number of runs that produced this output.
    pub count: usize,
}

/// Analyze repeated outputs for reproducibility.
#[must_use]
pub fn analyze_outputs(outputs: &[String]) -> DeterminismReport {
    let total = outputs.len();
    if total == 0 {
        return DeterminismReport {
            total_runs: 0,
            unique_outputs: 0,
            deterministic: true,
            divergence_token: None,
            majority_fraction: 1.0,
            distinct: Vec::new(),
        };
    }

    let mut counts: AHashMap<String, usize> = AHashMap::new();
    for o in outputs {
        *counts.entry(content_hash(o.as_bytes())).or_default() += 1;
    }
    let mut distinct: Vec<DistinctOutput> = counts
        .into_iter()
        .map(|(fingerprint, count)| DistinctOutput { fingerprint, count })
        .collect();
    // Deterministic ordering: by count desc, then fingerprint asc.
    distinct.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(a.fingerprint.cmp(&b.fingerprint))
    });

    let unique = distinct.len();
    let majority = distinct.first().map(|d| d.count).unwrap_or(0);
    let divergence_token = if unique <= 1 {
        None
    } else {
        first_divergence(outputs)
    };

    DeterminismReport {
        total_runs: total,
        unique_outputs: unique,
        deterministic: unique <= 1,
        divergence_token,
        majority_fraction: majority as f64 / total as f64,
        distinct,
    }
}

/// First whitespace-token index at which the outputs disagree.
fn first_divergence(outputs: &[String]) -> Option<usize> {
    let tokenized: Vec<Vec<&str>> = outputs
        .iter()
        .map(|o| o.split_whitespace().collect())
        .collect();
    let max_len = tokenized.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..max_len {
        let first = tokenized[0].get(i);
        for t in &tokenized[1..] {
            if t.get(i) != first {
                return Some(i);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_identical_is_deterministic() {
        let outs = vec!["the answer is 42".to_string(); 16];
        let r = analyze_outputs(&outs);
        assert!(r.deterministic);
        assert_eq!(r.unique_outputs, 1);
        assert_eq!(r.divergence_token, None);
        assert_eq!(r.majority_fraction, 1.0);
    }

    #[test]
    fn divergence_token_located() {
        let outs = vec![
            "the answer is 42".to_string(),
            "the answer is 43".to_string(),
            "the answer is 42".to_string(),
        ];
        let r = analyze_outputs(&outs);
        assert!(!r.deterministic);
        assert_eq!(r.unique_outputs, 2);
        assert_eq!(r.divergence_token, Some(3));
        assert!((r.majority_fraction - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_is_trivially_deterministic() {
        let r = analyze_outputs(&[]);
        assert!(r.deterministic);
        assert_eq!(r.total_runs, 0);
    }

    #[test]
    fn analysis_is_reproducible() {
        let outs = vec![
            "a b c".to_string(),
            "a b d".to_string(),
            "a b c".to_string(),
            "a b e".to_string(),
        ];
        let a = analyze_outputs(&outs);
        let b = analyze_outputs(&outs);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}

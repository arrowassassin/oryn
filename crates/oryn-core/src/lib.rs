//! # Oryn core — the reproducibility & evaluation-integrity engine
//!
//! Oryn is a **non-AI** toolchain that makes AI results you can reproduce,
//! trust, and audit. There is no model in the loop anywhere in this crate — only
//! classical computer science:
//!
//! * [`contam`] — benchmark/training-data **contamination scanning** (exact
//!   n-gram overlap + MinHash/LSH near-duplicate detection).
//! * [`eval`] — **statistically rigorous evaluation**: confidence intervals,
//!   power/required-N, and a paired **regression gate**.
//! * [`determinism`] — analysis of repeated generations to detect
//!   nondeterministic inference (the batch-invariance symptom).
//! * [`attest`] — tamper-evident, **Ed25519-signed hash chains** sealing reports
//!   for audit.
//! * [`stats`] / [`text`] — the deterministic numerics and tokenization beneath
//!   it all.
//!
//! Every public routine is deterministic: same input, same bytes out, on any
//! machine. Companion crate `oryn-cuda` provides the batch-invariant GPU kernels
//! that make *inference itself* reproducible.

#![forbid(unsafe_code)]

pub mod attest;
pub mod contam;
pub mod determinism;
pub mod error;
pub mod eval;
pub mod report;
pub mod stats;
pub mod text;

pub use error::{OrynError, Result};

/// Crate version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Serialize any report to canonical (stable-key) pretty JSON.
///
/// `serde_json` already emits map keys in struct-declaration order and our types
/// avoid `HashMap`, so this is reproducible byte-for-byte — suitable for hashing
/// into an attestation.
///
/// # Errors
/// Propagates serialization failures.
pub fn to_canonical_json<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

/// Convenience re-exports for the common path.
pub mod prelude {
    pub use crate::attest::{Attestation, AttestationChain, Signer};
    pub use crate::contam::{
        self_duplicates, ContaminationReport, CorpusIndex, Document, DuplicatePair,
        ItemContamination, ScanConfig,
    };
    pub use crate::determinism::{analyze_outputs, DeterminismReport};
    pub use crate::eval::{
        analyze, regression_gate, EvalConfig, EvalItem, EvalReport, EvalRun, GateVerdict,
        RegressionGate,
    };
    pub use crate::report::{IntegrityReport, IntegrityVerdict};
    pub use crate::text::Normalization;
    pub use crate::{OrynError, Result};
}

#[cfg(test)]
mod integration {
    use crate::prelude::*;

    #[test]
    fn end_to_end_scan_eval_attest() {
        // Contamination scan.
        let corpus = vec![Document::new(
            "c1",
            "the mitochondria is the powerhouse of the cell",
        )];
        let idx = CorpusIndex::build(
            &corpus,
            ScanConfig {
                ngram_n: 3,
                ..Default::default()
            },
        );
        let eval_docs = vec![
            Document::new("leaked", "the mitochondria is the powerhouse of the cell"),
            Document::new(
                "clean",
                "photosynthesis converts light into chemical energy",
            ),
        ];
        let scan = idx.scan(&eval_docs);
        assert_eq!(scan.contaminated_items, 1);
        assert_eq!(scan.clean_holdout, vec!["clean".to_string()]);

        // Eval with error bars.
        let run = EvalRun::new(
            "demo",
            (0..40)
                .map(|i| EvalItem::new(format!("q{i}"), (i % 4 != 0) as u8 as f64))
                .collect(),
        );
        let report = analyze(&run, &EvalConfig::default()).unwrap();
        assert!(report.ci.margin() > 0.0);

        // Seal both into a signed chain and verify.
        let signer = Signer::from_seed(&[3u8; 32]);
        let mut chain = AttestationChain::new();
        let scan_json = crate::to_canonical_json(&scan).unwrap();
        let eval_json = crate::to_canonical_json(&report).unwrap();
        chain
            .append(&signer, "contamination", scan_json.as_bytes())
            .unwrap();
        chain.append(&signer, "eval", eval_json.as_bytes()).unwrap();
        assert!(chain.verify().is_ok());
        assert!(chain.verify_payload(0, scan_json.as_bytes()).is_ok());
    }
}

//! Aggregate integrity report — one bundle a UI or auditor can consume.
//!
//! Combines any subset of the engine's outputs (contamination, eval, regression
//! gate, determinism) into a single deterministic document, plus a one-glance
//! verdict and the ability to **seal** it into a signed attestation chain.

use crate::attest::{AttestationChain, Signer};
use crate::contam::{ContaminationReport, DuplicatePair};
use crate::determinism::DeterminismReport;
use crate::eval::{EvalReport, RegressionGate};
use crate::Result;
use serde::{Deserialize, Serialize};

/// Overall pass/warn/fail verdict derived from the included signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityVerdict {
    /// No issues detected in the included signals.
    Pass,
    /// Non-blocking concerns (e.g. some contamination, nondeterminism).
    Warn,
    /// A blocking failure (e.g. regression gate blocked, heavy contamination).
    Fail,
}

/// A composite, deterministic integrity report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntegrityReport {
    /// Engine version that produced it.
    pub engine_version: String,
    /// Free-form label for the subject under review.
    pub subject: String,
    /// Contamination scan, if run.
    pub contamination: Option<ContaminationReport>,
    /// Intra-set duplicate pairs, if computed.
    #[serde(default)]
    pub duplicates: Vec<DuplicatePair>,
    /// Eval report, if run.
    pub eval: Option<EvalReport>,
    /// Regression gate, if run.
    pub gate: Option<RegressionGate>,
    /// Determinism report, if run.
    pub determinism: Option<DeterminismReport>,
}

impl IntegrityReport {
    /// Start a new report for `subject`.
    #[must_use]
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            engine_version: crate::VERSION.to_string(),
            subject: subject.into(),
            ..Default::default()
        }
    }

    /// Derive the overall verdict from whatever signals are present.
    ///
    /// Rules: a blocked gate or >25% contamination → Fail; any contamination,
    /// duplicates, or nondeterminism → Warn; otherwise Pass.
    #[must_use]
    pub fn verdict(&self) -> IntegrityVerdict {
        if self.gate.as_ref().is_some_and(|g| g.blocked) {
            return IntegrityVerdict::Fail;
        }
        if self
            .contamination
            .as_ref()
            .is_some_and(|c| c.contamination_rate > 0.25)
        {
            return IntegrityVerdict::Fail;
        }
        let warn = self
            .contamination
            .as_ref()
            .is_some_and(|c| c.contaminated_items > 0)
            || !self.duplicates.is_empty()
            || self.determinism.as_ref().is_some_and(|d| !d.deterministic);
        if warn {
            IntegrityVerdict::Warn
        } else {
            IntegrityVerdict::Pass
        }
    }

    /// Seal this report into a fresh single-entry signed attestation chain.
    ///
    /// # Errors
    /// Propagates serialization/crypto errors.
    pub fn seal(&self, signer: &Signer) -> Result<AttestationChain> {
        let json = crate::to_canonical_json(self)?;
        let mut chain = AttestationChain::new();
        chain.append(signer, "integrity-report", json.as_bytes())?;
        Ok(chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn empty_report_passes() {
        let r = IntegrityReport::new("nothing");
        assert_eq!(r.verdict(), IntegrityVerdict::Pass);
    }

    #[test]
    fn blocked_gate_fails() {
        let ids: Vec<String> = (0..50).map(|i| format!("q{i}")).collect();
        let base = EvalRun::new("b", ids.iter().map(|i| EvalItem::new(i, 1.0)).collect());
        let cand = EvalRun::new("c", ids.iter().map(|i| EvalItem::new(i, 0.0)).collect());
        let gate = regression_gate(&base, &cand, 0.95).unwrap();
        let mut r = IntegrityReport::new("subject");
        r.gate = Some(gate);
        assert_eq!(r.verdict(), IntegrityVerdict::Fail);
    }

    #[test]
    fn nondeterminism_warns() {
        let mut r = IntegrityReport::new("subject");
        r.determinism = Some(analyze_outputs(&["a b c".to_string(), "a b d".to_string()]));
        assert_eq!(r.verdict(), IntegrityVerdict::Warn);
    }

    #[test]
    fn seal_then_verify_roundtrips() {
        let r = IntegrityReport::new("subject");
        let signer = Signer::from_seed(&[9u8; 32]);
        let chain = r.seal(&signer).unwrap();
        assert!(chain.verify().is_ok());
        let json = crate::to_canonical_json(&r).unwrap();
        assert!(chain.verify_payload(0, json.as_bytes()).is_ok());
    }
}

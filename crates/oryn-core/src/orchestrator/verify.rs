//! Verify-by-execution — the real "did it actually pass?" gate.
//!
//! [`ExecutionVerifier`] runs the project's test command inside the target's
//! worktree and gates on the **process exit code**: zero passes, non-zero fails.
//! This is the verification the whole "route, don't race" thesis rests on — a
//! cheaper tier only wins if its change genuinely builds and passes, not if a
//! model merely claims success.
//!
//! It wraps an inner [`Verifier`] (typically the advisor) used as a fallback when
//! there is no test command configured, no worktree for the target, or the test
//! runner itself can't be launched — so a missing test setup degrades to a
//! judgement rather than a hard failure.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::orchestrator::harness::HarnessInvocation;
use crate::orchestrator::provider::{CompletionResponse, ExecutionTarget};
use crate::orchestrator::runner::ProcessRunner;
use crate::orchestrator::scheduler::{Verdict, Verifier};
use crate::orchestrator::task::Subtask;

/// A [`Verifier`] that runs a test command in the target's worktree and passes
/// only on a zero exit code.
pub struct ExecutionVerifier<V: Verifier> {
    runner: Arc<dyn ProcessRunner>,
    /// Per-target worktree directory to run the tests in.
    workdirs: BTreeMap<ExecutionTarget, PathBuf>,
    /// The test command (program + args). Empty means "no execution gate".
    command: Vec<String>,
    /// Fallback verifier used when execution can't decide.
    inner: V,
}

impl<V: Verifier> ExecutionVerifier<V> {
    /// Build an execution verifier. An empty `command` makes it a pure pass-through
    /// to `inner` (the app supplies a command only when it detects a test runner).
    pub fn new(
        runner: Arc<dyn ProcessRunner>,
        workdirs: BTreeMap<ExecutionTarget, PathBuf>,
        command: Vec<String>,
        inner: V,
    ) -> Self {
        Self {
            runner,
            workdirs,
            command,
            inner,
        }
    }
}

impl<V: Verifier> Verifier for ExecutionVerifier<V> {
    fn verify(
        &self,
        target: &ExecutionTarget,
        subtask: &Subtask,
        response: &CompletionResponse,
    ) -> Verdict {
        if self.command.is_empty() {
            return self.inner.verify(target, subtask, response);
        }
        let Some(cwd) = self.workdirs.get(target) else {
            return self.inner.verify(target, subtask, response);
        };
        let inv = HarnessInvocation {
            program: self.command[0].clone(),
            args: self.command[1..].to_vec(),
            env: vec![],
            stdin: None,
            cwd: cwd.clone(),
        };
        match self.runner.run(&inv) {
            // Tests ran: the exit code is the verdict. A passing run is fully
            // trusted (1.0); a failing run is a hard fail (0.0).
            Ok(output) => Verdict {
                passed: output.exit_code == 0,
                score: if output.exit_code == 0 { 1.0 } else { 0.0 },
            },
            // Couldn't launch the test runner — fall back to the inner verifier.
            Err(_) => self.inner.verify(target, subtask, response),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TokenUsage;
    use crate::orchestrator::provider::{AgentFramework, ModelId};
    use crate::orchestrator::runner::{ProcessOutput, RunError};
    use crate::orchestrator::task::{SubtaskId, SubtaskKind};
    use std::sync::Mutex;

    /// A runner that returns a fixed exit code, or errors when `fail_to_spawn`.
    struct ExitRunner {
        exit_code: i32,
        fail_to_spawn: bool,
        invoked: Mutex<Vec<String>>,
    }
    impl ExitRunner {
        fn new(exit_code: i32) -> Self {
            Self {
                exit_code,
                fail_to_spawn: false,
                invoked: Mutex::new(vec![]),
            }
        }
        fn unspawnable() -> Self {
            Self {
                exit_code: 0,
                fail_to_spawn: true,
                invoked: Mutex::new(vec![]),
            }
        }
    }
    impl ProcessRunner for ExitRunner {
        fn run(&self, inv: &HarnessInvocation) -> Result<ProcessOutput, RunError> {
            self.invoked.lock().unwrap().push(inv.program.clone());
            if self.fail_to_spawn {
                return Err(RunError::Spawn {
                    program: inv.program.clone(),
                    reason: "missing".into(),
                });
            }
            Ok(ProcessOutput {
                stdout_lines: vec![],
                exit_code: self.exit_code,
            })
        }
    }

    /// A fixed-verdict inner verifier so we can observe fallbacks.
    struct InnerStub(Verdict);
    impl Verifier for InnerStub {
        fn verify(&self, _t: &ExecutionTarget, _s: &Subtask, _r: &CompletionResponse) -> Verdict {
            self.0
        }
    }

    fn target() -> ExecutionTarget {
        ExecutionTarget::new(AgentFramework::Local, ModelId::new("m"))
    }
    fn subtask() -> Subtask {
        Subtask {
            id: SubtaskId::new("s"),
            kind: SubtaskKind::Debugging,
            summary: "x".into(),
            deps: vec![],
        }
    }
    fn response() -> CompletionResponse {
        CompletionResponse {
            text: "done".into(),
            usage: TokenUsage::default(),
        }
    }
    fn workdirs() -> BTreeMap<ExecutionTarget, PathBuf> {
        BTreeMap::from([(target(), PathBuf::from("/wt"))])
    }
    const DENY: Verdict = Verdict {
        passed: false,
        score: 0.0,
    };

    #[test]
    fn zero_exit_passes_with_full_score() {
        let v = ExecutionVerifier::new(
            Arc::new(ExitRunner::new(0)),
            workdirs(),
            vec!["cargo".into(), "test".into()],
            InnerStub(DENY),
        );
        let out = v.verify(&target(), &subtask(), &response());
        assert!(out.passed);
        assert_eq!(out.score, 1.0);
    }

    #[test]
    fn nonzero_exit_fails() {
        let v = ExecutionVerifier::new(
            Arc::new(ExitRunner::new(101)),
            workdirs(),
            vec!["cargo".into(), "test".into()],
            InnerStub(Verdict {
                passed: true,
                score: 0.9,
            }),
        );
        // Exit code wins over what the advisor would have said.
        let out = v.verify(&target(), &subtask(), &response());
        assert!(!out.passed);
        assert_eq!(out.score, 0.0);
    }

    #[test]
    fn empty_command_delegates_to_inner() {
        let runner = Arc::new(ExitRunner::new(0));
        let v = ExecutionVerifier::new(
            runner.clone(),
            workdirs(),
            vec![],
            InnerStub(Verdict {
                passed: true,
                score: 0.7,
            }),
        );
        let out = v.verify(&target(), &subtask(), &response());
        assert!(out.passed && (out.score - 0.7).abs() < 1e-9);
        assert!(
            runner.invoked.lock().unwrap().is_empty(),
            "no test process spawned"
        );
    }

    #[test]
    fn unspawnable_runner_falls_back_to_inner() {
        let v = ExecutionVerifier::new(
            Arc::new(ExitRunner::unspawnable()),
            workdirs(),
            vec!["pytest".into()],
            InnerStub(Verdict {
                passed: true,
                score: 0.5,
            }),
        );
        let out = v.verify(&target(), &subtask(), &response());
        assert!(out.passed && (out.score - 0.5).abs() < 1e-9);
    }

    #[test]
    fn unknown_target_falls_back_to_inner() {
        let v = ExecutionVerifier::new(
            Arc::new(ExitRunner::new(1)),
            BTreeMap::new(), // no workdir for the target
            vec!["cargo".into(), "test".into()],
            InnerStub(Verdict {
                passed: true,
                score: 0.6,
            }),
        );
        let out = v.verify(&target(), &subtask(), &response());
        assert!(out.passed && (out.score - 0.6).abs() < 1e-9);
    }
}

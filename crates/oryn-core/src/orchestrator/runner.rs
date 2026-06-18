//! Process execution + output normalization for headless harness runs.
//!
//! [`build_invocation`](super::harness::build_invocation) produces a pure command;
//! this module *runs* it (behind the [`ProcessRunner`] trait so it is faked in
//! tests), normalizes the vendor's stdout into `(final_text, usage)`
//! ([`parse_run`]), and exposes [`HarnessProvider`] — a [`ModelProvider`] that ties
//! the two together so the deterministic scheduler can drive real agents.
//!
//! Output shapes are heterogeneous: Claude/Codex stream NDJSON, Gemini/Cursor emit
//! a single JSON object, aider/local emit plain text. [`parse_run`] handles all of
//! them, falling back to raw text when nothing parses — never panicking on drift,
//! the same robustness contract as the event adapters.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use crate::event::TokenUsage;
use crate::orchestrator::harness::{AuthMode, HarnessInvocation, build_invocation};
use crate::orchestrator::provider::{
    AgentFramework, CompletionRequest, CompletionResponse, ModelProvider, ModelSpec, ProviderError,
};

// ── ProcessRunner ───────────────────────────────────────────────────────────────

/// Errors a [`ProcessRunner`] can return.
#[derive(Debug, Error)]
pub enum RunError {
    /// The program could not be launched (not installed, not on `PATH`, …).
    #[error("failed to spawn `{program}`: {reason}")]
    Spawn { program: String, reason: String },
    /// Spawned but I/O failed while communicating with the child.
    #[error("i/o error running `{program}`: {reason}")]
    Io { program: String, reason: String },
}

/// The captured result of running a harness to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// Standard-output split into lines (NDJSON frames or plain text).
    pub stdout_lines: Vec<String>,
    /// Process exit code (`-1` if terminated without one).
    pub exit_code: i32,
}

/// Runs a [`HarnessInvocation`] to completion. Object-safe so the registry can
/// hold `Arc<dyn ProcessRunner>`; faked in tests.
pub trait ProcessRunner: Send + Sync {
    /// Spawn `inv`, feed its stdin, and capture stdout.
    ///
    /// # Errors
    ///
    /// [`RunError::Spawn`] if the program cannot be launched, [`RunError::Io`] on a
    /// communication failure.
    fn run(&self, inv: &HarnessInvocation) -> Result<ProcessOutput, RunError>;
}

/// The real runner: spawns a child process via [`std::process`].
///
/// Blocks until the child exits, capturing all of stdout. Incremental streaming
/// and budget-driven kill are a later enhancement; the trait seam keeps that
/// change isolated.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, inv: &HarnessInvocation) -> Result<ProcessOutput, RunError> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut cmd = Command::new(&inv.program);
        cmd.args(&inv.args).current_dir(&inv.cwd);
        for (k, v) in &inv.env {
            cmd.env(k, v);
        }
        cmd.stdin(if inv.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| RunError::Spawn { program: inv.program.clone(), reason: e.to_string() })?;

        if let Some(input) = &inv.stdin
            && let Some(mut sink) = child.stdin.take()
        {
            // Write then drop `sink` to close stdin before waiting (avoids deadlock).
            sink.write_all(input.as_bytes())
                .map_err(|e| RunError::Io { program: inv.program.clone(), reason: e.to_string() })?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| RunError::Io { program: inv.program.clone(), reason: e.to_string() })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(ProcessOutput {
            stdout_lines: stdout.lines().map(str::to_string).collect(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

// ── output normalization ────────────────────────────────────────────────────────

/// The normalized result extracted from a harness's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// The agent's final textual answer/summary.
    pub final_text: String,
    /// Token usage, when the harness reported it (zero otherwise).
    pub usage: TokenUsage,
}

/// Normalize `lines` of a `framework`'s stdout into a [`RunOutcome`].
///
/// Text-only harnesses (aider, local) join their lines verbatim. JSON harnesses
/// are scanned frame-by-frame: the last recognized text field wins (the final
/// message), and the last usage object wins (the cumulative total). Anything that
/// fails to parse falls back to the raw joined text.
pub fn parse_run(framework: AgentFramework, lines: &[String]) -> RunOutcome {
    match framework {
        AgentFramework::Aider | AgentFramework::Local => {
            RunOutcome { final_text: lines.join("\n"), usage: TokenUsage::default() }
        }
        _ => parse_structured(lines),
    }
}

fn parse_structured(lines: &[String]) -> RunOutcome {
    let mut final_text: Option<String> = None;
    let mut usage = TokenUsage::default();
    let mut saw_json = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            saw_json = true;
            if let Some(u) = extract_usage(&v) {
                usage = u;
            }
            if let Some(text) = extract_text(&v) {
                final_text = Some(text);
            }
        }
    }

    let final_text = match final_text {
        Some(t) => t,
        None if saw_json => String::new(),
        None => lines.join("\n"),
    };
    RunOutcome { final_text, usage }
}

/// Extract usage from `v["usage"]` (or `v` itself), tolerating both Anthropic and
/// OpenAI key spellings. Returns `None` when no token field is present.
fn extract_usage(v: &Value) -> Option<TokenUsage> {
    let obj = v.get("usage").unwrap_or(v);
    let get = |keys: &[&str]| keys.iter().find_map(|k| obj.get(*k).and_then(Value::as_u64));
    let input = get(&["input_tokens", "prompt_tokens"]);
    let output = get(&["output_tokens", "completion_tokens"]);
    let cache_read = get(&["cache_read_input_tokens", "cached_tokens"]);
    let cache_write = get(&["cache_creation_input_tokens"]);
    input.or(output).or(cache_read).or(cache_write)?;
    Some(TokenUsage {
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        cache_read: cache_read.unwrap_or(0),
        cache_write: cache_write.unwrap_or(0),
    })
}

/// Extract a final-message string from common top-level keys.
fn extract_text(v: &Value) -> Option<String> {
    for key in ["result", "response", "last_agent_message", "text", "message"] {
        if let Some(s) = v.get(key).and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some(s.to_string());
        }
    }
    None
}

// ── HarnessProvider ───────────────────────────────────────────────────────────

/// A [`ModelProvider`] that executes one `(framework, model)` target by spawning
/// its headless CLI in an isolated worktree.
///
/// The deterministic scheduler treats this exactly like any other provider: it
/// calls [`complete`](ModelProvider::complete), which builds the invocation, runs
/// it through the injected [`ProcessRunner`], and normalizes the output.
pub struct HarnessProvider {
    spec: ModelSpec,
    workdir: PathBuf,
    auth: AuthMode,
    runner: Arc<dyn ProcessRunner>,
}

impl HarnessProvider {
    /// Build a provider for `spec`, running in `workdir`, authenticated per `auth`,
    /// executed by `runner`.
    pub fn new(spec: ModelSpec, workdir: PathBuf, auth: AuthMode, runner: Arc<dyn ProcessRunner>) -> Self {
        Self { spec, workdir, auth, runner }
    }
}

impl ModelProvider for HarnessProvider {
    fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let inv = build_invocation(&self.spec.target(), &req.prefix, &req.suffix, &self.workdir, &self.auth);
        let output = self.runner.run(&inv).map_err(|e| match e {
            // A target whose CLI isn't installed/launchable is treated as
            // unavailable so the cascade skips it — never a panic.
            RunError::Spawn { .. } => ProviderError::Unavailable,
            RunError::Io { reason, .. } => ProviderError::Refused(reason),
        })?;
        let outcome = parse_run(self.spec.framework, &output.stdout_lines);
        Ok(CompletionResponse { text: outcome.final_text, usage: outcome.usage })
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::provider::{ExecutionTarget, ModelId, ModelKind, Pricing};
    use std::sync::Mutex;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    // ── parse_run: Claude stream-json ─────────────────────────────────────────

    #[test]
    fn parse_claude_stream_json_extracts_result_and_usage() {
        let out = parse_run(
            AgentFramework::ClaudeCode,
            &lines(&[
                r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}"#,
                r#"{"type":"result","subtype":"success","result":"Fixed the race.","usage":{"input_tokens":1200,"output_tokens":340,"cache_read_input_tokens":9000,"cache_creation_input_tokens":200},"total_cost_usd":0.02}"#,
            ]),
        );
        assert_eq!(out.final_text, "Fixed the race.");
        assert_eq!(out.usage, TokenUsage { input: 1200, output: 340, cache_read: 9000, cache_write: 200 });
    }

    // ── parse_run: Gemini single JSON object ──────────────────────────────────

    #[test]
    fn parse_gemini_single_object_extracts_response_and_usage() {
        let out = parse_run(
            AgentFramework::GeminiCli,
            &lines(&[r#"{"response":"done","usage":{"prompt_tokens":500,"completion_tokens":120}}"#]),
        );
        assert_eq!(out.final_text, "done");
        assert_eq!(out.usage, TokenUsage { input: 500, output: 120, cache_read: 0, cache_write: 0 });
    }

    // ── parse_run: Codex NDJSON (OpenAI key spelling, last frame wins) ─────────

    #[test]
    fn parse_codex_ndjson_takes_final_message_and_usage() {
        let out = parse_run(
            AgentFramework::Codex,
            &lines(&[
                r#"{"type":"item.started","text":"thinking"}"#,
                r#"{"type":"item.completed","last_agent_message":"All tests pass.","usage":{"prompt_tokens":2000,"completion_tokens":600,"cached_tokens":1500}}"#,
            ]),
        );
        assert_eq!(out.final_text, "All tests pass.");
        assert_eq!(out.usage, TokenUsage { input: 2000, output: 600, cache_read: 1500, cache_write: 0 });
    }

    // ── parse_run: text harnesses ──────────────────────────────────────────────

    #[test]
    fn parse_aider_joins_text_with_zero_usage() {
        let out = parse_run(AgentFramework::Aider, &lines(&["Applied edit to auth.ts", "Commit a1b2c3"]));
        assert_eq!(out.final_text, "Applied edit to auth.ts\nCommit a1b2c3");
        assert_eq!(out.usage, TokenUsage::default());
    }

    #[test]
    fn parse_local_joins_text() {
        let out = parse_run(AgentFramework::Local, &lines(&["line one", "line two"]));
        assert_eq!(out.final_text, "line one\nline two");
    }

    #[test]
    fn parse_structured_falls_back_to_raw_text_on_non_json() {
        let out = parse_run(AgentFramework::ClaudeCode, &lines(&["not json at all", "still not"]));
        assert_eq!(out.final_text, "not json at all\nstill not");
        assert_eq!(out.usage, TokenUsage::default());
    }

    #[test]
    fn parse_run_is_deterministic() {
        let frames = lines(&[r#"{"result":"x","usage":{"input_tokens":1}}"#]);
        assert_eq!(parse_run(AgentFramework::ClaudeCode, &frames), parse_run(AgentFramework::ClaudeCode, &frames));
    }

    // ── HarnessProvider with a fake runner ─────────────────────────────────────

    /// Records the invocation it was given and replays a canned result.
    struct FakeRunner {
        result: Result<ProcessOutput, ()>,
        seen: Mutex<Option<HarnessInvocation>>,
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, inv: &HarnessInvocation) -> Result<ProcessOutput, RunError> {
            *self.seen.lock().unwrap() = Some(inv.clone());
            self.result
                .clone()
                .map_err(|()| RunError::Spawn { program: inv.program.clone(), reason: "missing".into() })
        }
    }

    fn spec(framework: AgentFramework, id: &str) -> ModelSpec {
        ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "test".into() },
            pricing: Pricing::ZERO,
            context_window: 200_000,
            framework,
        }
    }

    fn request() -> CompletionRequest {
        CompletionRequest { prefix: "CTX".into(), suffix: "do it".into(), temperature: 0.0, seed: Some(1) }
    }

    #[test]
    fn provider_runs_builds_and_parses() {
        let runner = Arc::new(FakeRunner {
            result: Ok(ProcessOutput {
                stdout_lines: lines(&[
                    r#"{"type":"result","result":"done","usage":{"input_tokens":10,"output_tokens":5}}"#,
                ]),
                exit_code: 0,
            }),
            seen: Mutex::new(None),
        });
        let provider = HarnessProvider::new(
            spec(AgentFramework::ClaudeCode, "opus"),
            PathBuf::from("/work/oryn-claude"),
            AuthMode::Subscription,
            runner.clone(),
        );
        let resp = provider.complete(&request()).unwrap();
        assert_eq!(resp.text, "done");
        assert_eq!(resp.usage, TokenUsage { input: 10, output: 5, cache_read: 0, cache_write: 0 });
        // The provider built the right command for the target.
        let inv = runner.seen.lock().unwrap().clone().unwrap();
        assert_eq!(inv.program, "claude");
        assert_eq!(inv.stdin.as_deref(), Some("CTX\n\ndo it"));
        assert_eq!(inv.cwd, PathBuf::from("/work/oryn-claude"));
    }

    #[test]
    fn provider_maps_spawn_failure_to_unavailable() {
        let runner = Arc::new(FakeRunner { result: Err(()), seen: Mutex::new(None) });
        let provider = HarnessProvider::new(
            spec(AgentFramework::Cursor, "ghost"),
            PathBuf::from("/work"),
            AuthMode::Subscription,
            runner,
        );
        match provider.complete(&request()) {
            Err(ProviderError::Unavailable) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn provider_spec_roundtrips_target() {
        let provider = HarnessProvider::new(
            spec(AgentFramework::Codex, "gpt-5.2"),
            PathBuf::from("/w"),
            AuthMode::Subscription,
            Arc::new(FakeRunner { result: Err(()), seen: Mutex::new(None) }),
        );
        assert_eq!(provider.spec().target(), ExecutionTarget::new(AgentFramework::Codex, ModelId::new("gpt-5.2")));
    }

    // ── SystemProcessRunner against a real, ubiquitous process ─────────────────

    #[cfg(unix)]
    #[test]
    fn system_runner_pipes_stdin_to_stdout_via_cat() {
        let inv = HarnessInvocation {
            program: "cat".into(),
            args: vec![],
            env: vec![],
            stdin: Some("hello\nworld".into()),
            cwd: std::env::temp_dir(),
        };
        let out = SystemProcessRunner.run(&inv).unwrap();
        assert_eq!(out.stdout_lines, vec!["hello".to_string(), "world".to_string()]);
        assert_eq!(out.exit_code, 0);
    }

    #[cfg(unix)]
    #[test]
    fn system_runner_reports_spawn_failure() {
        let inv = HarnessInvocation {
            program: "oryn-definitely-not-a-real-binary".into(),
            args: vec![],
            env: vec![],
            stdin: None,
            cwd: std::env::temp_dir(),
        };
        match SystemProcessRunner.run(&inv) {
            Err(RunError::Spawn { .. }) => {}
            other => panic!("expected Spawn error, got {other:?}"),
        }
    }
}

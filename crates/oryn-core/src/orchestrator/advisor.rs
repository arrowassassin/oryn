//! Local advisor model — the cheap, private, on-system decision-maker.
//!
//! A small local model (served via Ollama's OpenAI-compatible endpoint) provides
//! **bounded, reproducible** judgement that complements the deterministic core: it
//! semantically verifies a harness's result so the cascade can gate escalation
//! without burning the mission budget. It **never reorders the deterministic
//! route** — the capability cascade stays a pure function of the pinned matrix.
//!
//! Determinism: every call is `temperature = 0`, a [`ADVISOR_SEED`], strict-JSON
//! output, against a pinned model. Network sits behind the [`Http`] trait (faked in
//! tests); prompt construction and response parsing are pure and tested. If the
//! model is unreachable the caller degrades to the deterministic-only path rather
//! than failing — the same fallback philosophy as the bundled seed catalog.

use std::sync::Arc;

use serde_json::{Value, json};
use thiserror::Error;

use crate::orchestrator::provider::{CompletionResponse, ExecutionTarget};
use crate::orchestrator::scheduler::{Verdict, Verifier};
use crate::orchestrator::task::{Subtask, SubtaskKind};

/// Fixed sampling seed for advisor calls — reproducible judgement across runs.
pub const ADVISOR_SEED: u64 = 0x4F52_594E; // "ORYN"

// ── Http seam ─────────────────────────────────────────────────────────────────

/// Errors a [`Http`] backend can return.
#[derive(Debug, Error)]
pub enum HttpError {
    /// The endpoint could not be reached.
    #[error("http endpoint unreachable")]
    Unreachable,
    /// The endpoint returned a non-success status.
    #[error("http status {0}")]
    Status(u16),
}

/// Minimal blocking HTTP-POST seam, object-safe so it can be faked in tests. A
/// real implementation (reqwest/ureq) lands in the app layer; the core stays
/// network-free.
pub trait Http: Send + Sync {
    /// POST `body` as `application/json` to `url`, returning the response body.
    ///
    /// # Errors
    ///
    /// [`HttpError`] when the request cannot be completed.
    fn post_json(&self, url: &str, body: &str) -> Result<String, HttpError>;
}

// ── advisor ───────────────────────────────────────────────────────────────────

/// Errors from a [`LocalAdvisor`].
#[derive(Debug, Error)]
pub enum AdvisorError {
    /// The transport failed.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// The model's response could not be parsed into a verdict.
    #[error("malformed advisor response: {0}")]
    Malformed(String),
}

/// A local model that provides bounded orchestration judgement.
pub trait LocalAdvisor: Send + Sync {
    /// Judge whether `response_text` satisfies `subtask`'s intent.
    ///
    /// # Errors
    ///
    /// [`AdvisorError`] if the model is unreachable or its reply is malformed.
    fn verify(&self, subtask: &Subtask, response_text: &str) -> Result<Verdict, AdvisorError>;
}

/// Stable identifier for a [`SubtaskKind`], used in the advisor prompt.
fn kind_str(kind: SubtaskKind) -> &'static str {
    match kind {
        SubtaskKind::MechanicalEdit => "mechanical_edit",
        SubtaskKind::TestGen => "test_gen",
        SubtaskKind::DiffEdit => "diff_edit",
        SubtaskKind::LargeContext => "large_context",
        SubtaskKind::Debugging => "debugging",
        SubtaskKind::Refactor => "refactor",
    }
}

/// Build the deterministic request body for a verification call against an
/// Ollama OpenAI-compatible `/v1/chat/completions` endpoint.
///
/// `serde_json` serializes object keys in sorted order by default, so identical
/// inputs produce a byte-identical body.
pub fn verify_request_body(model: &str, subtask: &Subtask, response_text: &str) -> String {
    let system = "You are a strict code-review verifier. Given a sub-task and an \
                  agent's result, decide if the result satisfies the sub-task. \
                  Reply with ONLY a JSON object: {\"passed\": bool, \"score\": number \
                  in 0..1, \"reason\": string}.";
    let user = format!(
        "sub-task kind: {}\nsub-task: {}\n\nagent result:\n{}",
        kind_str(subtask.kind),
        subtask.summary,
        response_text
    );
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.0,
        "seed": ADVISOR_SEED,
        "response_format": {"type": "json_object"},
        "stream": false,
    })
    .to_string()
}

/// Parse a [`Verdict`] from an OpenAI-compatible chat-completions response body.
///
/// Reads `choices[0].message.content`, strips any markdown code fence, and parses
/// the inner JSON `{"passed", "score"}`.
pub fn parse_verdict(response_body: &str) -> Result<Verdict, AdvisorError> {
    let outer: Value = serde_json::from_str(response_body)
        .map_err(|e| AdvisorError::Malformed(format!("envelope: {e}")))?;
    let content = outer
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| AdvisorError::Malformed("missing choices[0].message.content".into()))?;

    let inner_str = strip_code_fence(content.trim());
    let inner: Value = serde_json::from_str(inner_str)
        .map_err(|e| AdvisorError::Malformed(format!("content: {e}")))?;

    let passed = inner
        .get("passed")
        .and_then(Value::as_bool)
        .ok_or_else(|| AdvisorError::Malformed("missing `passed`".into()))?;
    let score = inner
        .get("score")
        .and_then(Value::as_f64)
        .ok_or_else(|| AdvisorError::Malformed("missing `score`".into()))?
        .clamp(0.0, 1.0);
    Ok(Verdict { passed, score })
}

/// Strip a leading ```` ```json ```` / ```` ``` ```` fence and trailing ```` ``` ````
/// that small models sometimes wrap JSON in.
fn strip_code_fence(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    // Drop an optional language tag on the first line.
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.trim_start_matches('\n');
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

/// A [`LocalAdvisor`] backed by an Ollama OpenAI-compatible endpoint.
pub struct OllamaAdvisor {
    base_url: String,
    model: String,
    http: Arc<dyn Http>,
}

impl OllamaAdvisor {
    /// Construct an advisor talking to `base_url` (e.g. `http://localhost:11434`)
    /// using `model` (e.g. `qwen2.5-coder`).
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, http: Arc<dyn Http>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            http,
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }
}

impl LocalAdvisor for OllamaAdvisor {
    fn verify(&self, subtask: &Subtask, response_text: &str) -> Result<Verdict, AdvisorError> {
        let body = verify_request_body(&self.model, subtask, response_text);
        let reply = self.http.post_json(&self.endpoint(), &body)?;
        parse_verdict(&reply)
    }
}

// ── AdvisorVerifier ─────────────────────────────────────────────────────────────

/// Adapts a [`LocalAdvisor`] into the scheduler's [`Verifier`], degrading to a
/// fixed `fallback` verdict if the advisor errors — so an unavailable local model
/// never aborts a mission.
pub struct AdvisorVerifier<A: LocalAdvisor> {
    advisor: A,
    fallback: Verdict,
}

impl<A: LocalAdvisor> AdvisorVerifier<A> {
    /// Wrap `advisor`, using `fallback` when it is unreachable or malformed.
    pub fn new(advisor: A, fallback: Verdict) -> Self {
        Self { advisor, fallback }
    }
}

impl<A: LocalAdvisor> Verifier for AdvisorVerifier<A> {
    fn verify(
        &self,
        _target: &ExecutionTarget,
        subtask: &Subtask,
        response: &CompletionResponse,
    ) -> Verdict {
        self.advisor
            .verify(subtask, &response.text)
            .unwrap_or(self.fallback)
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TokenUsage;
    use crate::orchestrator::task::SubtaskId;
    use std::sync::Mutex;

    fn subtask() -> Subtask {
        Subtask {
            id: SubtaskId::new("s1"),
            kind: SubtaskKind::Debugging,
            summary: "fix the refresh race".into(),
            deps: vec![],
        }
    }

    fn tgt() -> ExecutionTarget {
        use crate::orchestrator::provider::{AgentFramework, ModelId};
        ExecutionTarget::new(AgentFramework::Local, ModelId::new("m"))
    }

    fn ok_body(content: &str) -> String {
        json!({"choices": [{"message": {"role": "assistant", "content": content}}]}).to_string()
    }

    // ── request construction ──────────────────────────────────────────────────

    #[test]
    fn verify_request_is_deterministic_and_complete() {
        let a = verify_request_body("qwen2.5-coder", &subtask(), "patched refreshQueue");
        let b = verify_request_body("qwen2.5-coder", &subtask(), "patched refreshQueue");
        assert_eq!(a, b, "identical inputs → byte-identical body");
        assert!(a.contains("qwen2.5-coder"));
        assert!(a.contains("debugging"));
        assert!(a.contains("fix the refresh race"));
        assert!(a.contains("patched refreshQueue"));
        // temperature 0 + fixed seed for reproducibility
        assert!(a.contains("\"temperature\":0.0"));
        assert!(a.contains(&format!("\"seed\":{ADVISOR_SEED}")));
    }

    // ── verdict parsing ────────────────────────────────────────────────────────

    #[test]
    fn parse_plain_json_verdict() {
        let v = parse_verdict(&ok_body(
            r#"{"passed":true,"score":0.92,"reason":"tests pass"}"#,
        ))
        .unwrap();
        assert!(v.passed);
        assert!((v.score - 0.92).abs() < 1e-9);
    }

    #[test]
    fn parse_verdict_strips_code_fence() {
        let v = parse_verdict(&ok_body("```json\n{\"passed\":false,\"score\":0.2}\n```")).unwrap();
        assert!(!v.passed);
        assert!((v.score - 0.2).abs() < 1e-9);
    }

    #[test]
    fn parse_verdict_clamps_score() {
        let v = parse_verdict(&ok_body(r#"{"passed":true,"score":1.5}"#)).unwrap();
        assert_eq!(v.score, 1.0);
    }

    #[test]
    fn parse_verdict_rejects_missing_fields() {
        assert!(matches!(
            parse_verdict(&ok_body(r#"{"score":0.5}"#)),
            Err(AdvisorError::Malformed(_))
        ));
        assert!(matches!(
            parse_verdict(&ok_body("not json")),
            Err(AdvisorError::Malformed(_))
        ));
        assert!(matches!(
            parse_verdict("garbage"),
            Err(AdvisorError::Malformed(_))
        ));
    }

    // ── fakes ────────────────────────────────────────────────────────────────

    struct FakeHttp {
        reply: Result<String, ()>,
        seen: Mutex<Option<(String, String)>>,
    }

    impl Http for FakeHttp {
        fn post_json(&self, url: &str, body: &str) -> Result<String, HttpError> {
            *self.seen.lock().unwrap() = Some((url.to_string(), body.to_string()));
            self.reply.clone().map_err(|()| HttpError::Unreachable)
        }
    }

    #[test]
    fn ollama_advisor_posts_to_chat_endpoint_and_parses() {
        let http = Arc::new(FakeHttp {
            reply: Ok(ok_body(r#"{"passed":true,"score":0.8}"#)),
            seen: Mutex::new(None),
        });
        let advisor = OllamaAdvisor::new("http://localhost:11434", "qwen2.5-coder", http.clone());
        let verdict = advisor.verify(&subtask(), "done").unwrap();
        assert!(verdict.passed);
        let (url, body) = http.seen.lock().unwrap().clone().unwrap();
        assert_eq!(url, "http://localhost:11434/v1/chat/completions");
        assert!(body.contains("qwen2.5-coder"));
    }

    #[test]
    fn ollama_advisor_surfaces_transport_error() {
        let http = Arc::new(FakeHttp {
            reply: Err(()),
            seen: Mutex::new(None),
        });
        let advisor = OllamaAdvisor::new("http://localhost:11434/", "m", http);
        assert!(matches!(
            advisor.verify(&subtask(), "x"),
            Err(AdvisorError::Http(_))
        ));
    }

    // ── AdvisorVerifier ─────────────────────────────────────────────────────────

    struct StubAdvisor(Result<Verdict, ()>);
    impl LocalAdvisor for StubAdvisor {
        fn verify(&self, _s: &Subtask, _t: &str) -> Result<Verdict, AdvisorError> {
            self.0.map_err(|()| AdvisorError::Malformed("stub".into()))
        }
    }

    fn response(text: &str) -> CompletionResponse {
        CompletionResponse {
            text: text.into(),
            usage: TokenUsage::default(),
        }
    }

    #[test]
    fn advisor_verifier_uses_advisor_verdict_on_success() {
        let v = AdvisorVerifier::new(
            StubAdvisor(Ok(Verdict {
                passed: true,
                score: 0.9,
            })),
            Verdict {
                passed: false,
                score: 0.0,
            },
        );
        let out = v.verify(&tgt(), &subtask(), &response("done"));
        assert!(out.passed);
        assert!((out.score - 0.9).abs() < 1e-9);
    }

    #[test]
    fn advisor_verifier_falls_back_on_error() {
        let fallback = Verdict {
            passed: false,
            score: 0.1,
        };
        let v = AdvisorVerifier::new(StubAdvisor(Err(())), fallback);
        let out = v.verify(&tgt(), &subtask(), &response("done"));
        assert!(!out.passed);
        assert!((out.score - 0.1).abs() < 1e-9);
    }

    #[test]
    fn http_error_displays() {
        assert_eq!(
            HttpError::Unreachable.to_string(),
            "http endpoint unreachable"
        );
        assert_eq!(HttpError::Status(503).to_string(), "http status 503");
    }
}

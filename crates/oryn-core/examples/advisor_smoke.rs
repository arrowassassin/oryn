//! Live smoke test for the local advisor against a *real* OpenAI-compatible
//! endpoint (Ollama, llamafile, llama.cpp server, …). No fakes: a real `ureq`
//! HTTP client over a real socket.
//!
//! Run it against a local model:
//!
//! ```sh
//! # Ollama (recommended): a deterministic, reasoning-capable local model
//! ollama serve &
//! ollama pull qwen2.5-coder:7b           # or: deepseek-r1:7b / qwq
//! ORYN_ADVISOR_MODEL=qwen2.5-coder:7b \
//!   cargo run -p oryn-core --example advisor_smoke
//!
//! # Point at any OpenAI-compatible server:
//! OLLAMA_HOST=http://localhost:8080 cargo run -p oryn-core --example advisor_smoke
//! ```
//!
//! It asks the model to verify a "good" and a "bad" result for the same sub-task
//! and prints the real [`Verdict`] each time. Exits non-zero if the endpoint is
//! unreachable, so it doubles as a connectivity check.

use std::sync::Arc;

use oryn_core::orchestrator::advisor::{Http, HttpError, LocalAdvisor, OllamaAdvisor};
use oryn_core::orchestrator::task::{Subtask, SubtaskId, SubtaskKind};

/// Real blocking HTTP client. This is the production transport — the same impl
/// belongs in the app layer; it lives here so the core crate stays network-free
/// (ureq is a dev-dependency, pulled in only for examples/tests).
struct UreqHttp;

impl Http for UreqHttp {
    fn post_json(&self, url: &str, body: &str) -> Result<String, HttpError> {
        match ureq::post(url).set("Content-Type", "application/json").send_string(body) {
            Ok(resp) => resp.into_string().map_err(|_| HttpError::Unreachable),
            Err(ureq::Error::Status(code, _)) => Err(HttpError::Status(code)),
            Err(_) => Err(HttpError::Unreachable),
        }
    }
}

fn main() {
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("ORYN_ADVISOR_MODEL").unwrap_or_else(|_| "qwen2.5-coder".into());
    eprintln!("advisor_smoke → endpoint {host}/v1/chat/completions · model {model}\n");

    let advisor = OllamaAdvisor::new(host, model, Arc::new(UreqHttp));

    let subtask = Subtask {
        id: SubtaskId::new("auth-1"),
        kind: SubtaskKind::Debugging,
        summary: "The token refresh fires twice under concurrent 401s. Add a single-flight \
                  guard so concurrent refreshes coalesce, and make auth/refresh.test.ts pass."
            .into(),
        deps: vec![],
    };

    let cases = [
        ("GOOD result", "Added a single-flight promise cache in refreshQueue.ts; concurrent 401s now await one refresh. Ran `pnpm vitest run auth/refresh` → 14/14 passing."),
        ("BAD result", "Renamed a variable and added a comment. Did not touch the refresh logic; tests still fail 3/14."),
    ];

    let mut failures = 0;
    for (label, result_text) in cases {
        match advisor.verify(&subtask, result_text) {
            Ok(v) => println!("{label:<12} → passed={} score={:.2}", v.passed, v.score),
            Err(e) => {
                eprintln!("{label:<12} → ERROR: {e}");
                failures += 1;
            }
        }
    }

    if failures > 0 {
        eprintln!(
            "\nCould not reach a model. Start one first, e.g.:\n  ollama serve & ollama pull qwen2.5-coder:7b"
        );
        std::process::exit(1);
    }
    println!("\nadvisor transport OK — real HTTP round-trip to the model succeeded.");
}

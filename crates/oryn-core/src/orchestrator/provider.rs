//! Model provider abstraction — one interface over API and local models.
//!
//! [`ModelProvider`] is the single interface the router/scheduler uses when it
//! needs a completion. Concrete implementations either call a remote API
//! (Anthropic, OpenAI, Google) or POST to a local OpenAI-compatible endpoint.
//!
//! [`ProviderRegistry`] holds a `Vec` of boxed providers so that insertion-order
//! iteration is always deterministic.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::event::TokenUsage;

// ── ModelId ──────────────────────────────────────────────────────────────────

/// Stable identifier for a model, e.g. `"claude-opus-4-5"` or
/// `"llama-3-8b-instruct"`.
///
/// Wraps a `String` the same way [`crate::ids::EventId`] does so handles cannot
/// be confused with arbitrary strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    /// Wrap an existing model-id string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── ModelKind ────────────────────────────────────────────────────────────────

/// Whether a model is reached via a hosted API or a local OpenAI-compatible
/// endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ModelKind {
    /// Hosted API model. `provider` is a short canonical name such as
    /// `"anthropic"`, `"openai"`, or `"google"`.
    Api { provider: String },
    /// Local model exposed via an OpenAI-compatible base URL (e.g. Ollama,
    /// llama.cpp, LM Studio).
    Local { endpoint: String },
}

// ── Pricing ───────────────────────────────────────────────────────────────────

/// USD cost per **million** tokens for each billing class.
///
/// Local models should use all-zero pricing. `Eq` is not derived because `f64`
/// does not implement `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pricing {
    /// Input (prompt) tokens — standard rate.
    pub input: f64,
    /// Output (completion) tokens.
    pub output: f64,
    /// Tokens served from the prompt cache (reduced rate).
    pub cache_read: f64,
    /// Tokens written to the prompt cache (premium rate).
    pub cache_write: f64,
}

impl Pricing {
    /// All-zero pricing, appropriate for local models.
    pub const ZERO: Self = Self {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    };
}

// ── ModelSpec ────────────────────────────────────────────────────────────────

/// Full specification for a model: what it is, where it lives, what it costs,
/// and how much context it accepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// The model's stable identifier.
    pub id: ModelId,
    /// Where the model is and how to reach it.
    pub kind: ModelKind,
    /// USD pricing per million tokens.
    pub pricing: Pricing,
    /// Maximum number of tokens the model accepts in a single context window.
    pub context_window: u32,
}

// ── CompletionRequest / CompletionResponse ────────────────────────────────────

/// A single completion request sent to a [`ModelProvider`].
///
/// The split between `prefix` and `suffix` lets adapters keep the
/// cache-stable portion separate from the volatile per-subtask instruction,
/// which is the primitive the future context broker needs to maximise prompt
/// cache hit rates.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    /// The cache-stable prefix (system prompt, shared context, …).
    pub prefix: String,
    /// The volatile per-subtask instruction appended after the prefix.
    pub suffix: String,
    /// Sampling temperature in `[0.0, 1.0]`.
    pub temperature: f32,
    /// Optional seed for deterministic sampling (when the provider supports it).
    pub seed: Option<u64>,
}

/// The result of a successful completion.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionResponse {
    /// The model-generated text.
    pub text: String,
    /// Token usage for this completion, in the same breakdown used by the event
    /// model (reuses [`TokenUsage`] from [`crate::event`]).
    pub usage: TokenUsage,
}

// ── ProviderError ─────────────────────────────────────────────────────────────

/// Errors that a [`ModelProvider`] can return.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The provider is unreachable (network failure, local server not running,
    /// …).
    #[error("provider unavailable")]
    Unavailable,
    /// The provider refused the request (content policy, auth failure, quota
    /// exceeded, …). The inner string carries the reason from the provider.
    #[error("provider refused: {0}")]
    Refused(String),
}

// ── ModelProvider trait ───────────────────────────────────────────────────────

/// One interface over any model backend.
///
/// The trait is object-safe (`spec` returns a reference; `complete` takes `&self`
/// and a reference) so the registry can hold `Box<dyn ModelProvider>` without
/// knowing the concrete type.
pub trait ModelProvider: Send + Sync {
    /// Metadata describing this model.
    fn spec(&self) -> &ModelSpec;

    /// Send `req` to the model and return its response.
    fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError>;
}

// ── ProviderRegistry ──────────────────────────────────────────────────────────

/// A registry of model providers.
///
/// Providers are stored in a `Vec` so that `get` and `specs` always iterate in
/// insertion order — deterministic regardless of the model ids involved.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: Vec<Box<dyn ModelProvider>>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { providers: Vec::new() }
    }

    /// Register `provider`. The provider is appended; subsequent `get` calls
    /// for its id will find it.
    pub fn register(&mut self, provider: Box<dyn ModelProvider>) {
        self.providers.push(provider);
    }

    /// Look up a provider by model id.
    ///
    /// Returns the first registered provider whose [`ModelSpec::id`] matches
    /// `id`, preserving insertion-order determinism.
    pub fn get(&self, id: &ModelId) -> Option<&dyn ModelProvider> {
        self.providers
            .iter()
            .find(|p| p.spec().id == *id)
            .map(|p| p.as_ref())
    }

    /// Ordered list of all registered model specs (insertion order).
    pub fn specs(&self) -> Vec<&ModelSpec> {
        self.providers.iter().map(|p| p.spec()).collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── FakeProvider ─────────────────────────────────────────────────────────

    /// A test-only provider that returns a fixed [`CompletionResponse`].
    struct FakeProvider {
        spec: ModelSpec,
        response_text: String,
        usage: TokenUsage,
    }

    impl FakeProvider {
        fn new(spec: ModelSpec, response_text: impl Into<String>, usage: TokenUsage) -> Self {
            Self { spec, response_text: response_text.into(), usage }
        }
    }

    impl ModelProvider for FakeProvider {
        fn spec(&self) -> &ModelSpec {
            &self.spec
        }

        fn complete(&self, _req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
            Ok(CompletionResponse {
                text: self.response_text.clone(),
                usage: self.usage,
            })
        }
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn api_spec(id: &str) -> ModelSpec {
        ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "anthropic".into() },
            pricing: Pricing {
                input: 3.0,
                output: 15.0,
                cache_read: 0.30,
                cache_write: 3.75,
            },
            context_window: 200_000,
        }
    }

    fn local_spec(id: &str, endpoint: &str) -> ModelSpec {
        ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Local { endpoint: endpoint.into() },
            pricing: Pricing::ZERO,
            context_window: 8_192,
        }
    }

    fn simple_req() -> CompletionRequest {
        CompletionRequest {
            prefix: "You are a coding assistant.".into(),
            suffix: "Write a hello world in Rust.".into(),
            temperature: 0.0,
            seed: Some(42),
        }
    }

    // ── ModelId tests ─────────────────────────────────────────────────────────

    #[test]
    fn model_id_new_as_str_display() {
        let id = ModelId::new("claude-opus-4-5");
        assert_eq!(id.as_str(), "claude-opus-4-5");
        assert_eq!(id.to_string(), "claude-opus-4-5");
    }

    #[test]
    fn model_id_eq_and_hash() {
        use std::collections::HashSet;
        let a = ModelId::new("m1");
        let b = ModelId::new("m1");
        let c = ModelId::new("m2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut set = HashSet::new();
        set.insert(a.clone());
        set.insert(b);
        assert_eq!(set.len(), 1);
        set.insert(c);
        assert_eq!(set.len(), 2);
    }

    // ── ModelKind serde ───────────────────────────────────────────────────────

    #[test]
    fn model_kind_api_roundtrips_json() {
        let kind = ModelKind::Api { provider: "anthropic".into() };
        let json = serde_json::to_string(&kind).unwrap();
        let back: ModelKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn model_kind_local_roundtrips_json() {
        let kind = ModelKind::Local { endpoint: "http://localhost:11434".into() };
        let json = serde_json::to_string(&kind).unwrap();
        let back: ModelKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    // ── Pricing serde ─────────────────────────────────────────────────────────

    #[test]
    fn pricing_zero_const_is_all_zero() {
        let p = Pricing::ZERO;
        assert_eq!(p.input, 0.0);
        assert_eq!(p.output, 0.0);
        assert_eq!(p.cache_read, 0.0);
        assert_eq!(p.cache_write, 0.0);
    }

    #[test]
    fn pricing_roundtrips_json() {
        let p = Pricing { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pricing = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    // ── ModelSpec serde ───────────────────────────────────────────────────────

    #[test]
    fn model_spec_roundtrips_json() {
        let spec = api_spec("claude-sonnet-4-5");
        let json = serde_json::to_string(&spec).unwrap();
        let back: ModelSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
    }

    // ── FakeProvider / completion ─────────────────────────────────────────────

    #[test]
    fn fake_provider_returns_canned_response() {
        let usage = TokenUsage { input: 100, output: 50, cache_read: 20, cache_write: 5 };
        let provider = FakeProvider::new(api_spec("m1"), "fn main() {}", usage);
        let resp = provider.complete(&simple_req()).unwrap();
        assert_eq!(resp.text, "fn main() {}");
        assert_eq!(resp.usage, usage);
    }

    #[test]
    fn fake_provider_spec_matches_constructed_spec() {
        let spec = local_spec("llama3", "http://localhost:11434");
        let provider = FakeProvider::new(spec.clone(), "", TokenUsage::default());
        assert_eq!(provider.spec(), &spec);
    }

    // ── ProviderRegistry ──────────────────────────────────────────────────────

    #[test]
    fn registry_get_returns_registered_provider() {
        let mut reg = ProviderRegistry::new();
        let id = ModelId::new("m1");
        let usage = TokenUsage { input: 10, output: 5, ..Default::default() };
        reg.register(Box::new(FakeProvider::new(api_spec("m1"), "hi", usage)));

        let found = reg.get(&id).expect("provider should be registered");
        assert_eq!(found.spec().id, id);
    }

    #[test]
    fn registry_get_returns_none_for_missing_id() {
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::new(api_spec("m1"), "", TokenUsage::default())));
        assert!(reg.get(&ModelId::new("not-here")).is_none());
    }

    #[test]
    fn registry_specs_returns_insertion_order() {
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::new(api_spec("first"), "", TokenUsage::default())));
        reg.register(Box::new(FakeProvider::new(
            local_spec("second", "http://localhost:11434"),
            "",
            TokenUsage::default(),
        )));
        reg.register(Box::new(FakeProvider::new(api_spec("third"), "", TokenUsage::default())));

        let ids: Vec<&str> = reg.specs().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["first", "second", "third"]);
    }

    #[test]
    fn registry_get_first_match_when_duplicate_ids() {
        // Edge case: if two providers share an id, get returns the first one.
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::new(api_spec("dup"), "first", TokenUsage::default())));
        reg.register(Box::new(FakeProvider::new(api_spec("dup"), "second", TokenUsage::default())));

        let p = reg.get(&ModelId::new("dup")).unwrap();
        let resp = p.complete(&simple_req()).unwrap();
        assert_eq!(resp.text, "first");
    }

    #[test]
    fn registry_complete_via_get() {
        let mut reg = ProviderRegistry::new();
        let usage = TokenUsage { input: 200, output: 80, cache_read: 50, cache_write: 10 };
        reg.register(Box::new(FakeProvider::new(api_spec("opus"), "some code", usage)));

        let p = reg.get(&ModelId::new("opus")).unwrap();
        let resp = p.complete(&simple_req()).unwrap();
        assert_eq!(resp.text, "some code");
        assert_eq!(resp.usage.total(), 340);
    }

    #[test]
    fn provider_error_unavailable_displays() {
        let e = ProviderError::Unavailable;
        assert_eq!(e.to_string(), "provider unavailable");
    }

    #[test]
    fn provider_error_refused_displays_reason() {
        let e = ProviderError::Refused("content policy violation".into());
        assert_eq!(e.to_string(), "provider refused: content policy violation");
    }

    #[test]
    fn registry_default_is_empty() {
        let reg = ProviderRegistry::default();
        assert!(reg.specs().is_empty());
        assert!(reg.get(&ModelId::new("any")).is_none());
    }
}

//! Dynamic model discovery — ask each CLI which models it can access, *now*.
//!
//! CLIs change their available models constantly, so Oryn never hardcodes model
//! names. Instead it runs each framework's "list models" command (via the same
//! [`ProcessRunner`](crate::orchestrator::runner::ProcessRunner) seam) and parses
//! whatever the CLI reports. Discovered `(framework, model)` targets are then
//! **enriched** from the pinned catalog: pricing (OpenRouter) and capability
//! (benchmarks), with a routable baseline for models we have no data on yet.
//!
//! This module is pure: it builds the list command ([`default_list_command`]),
//! parses the output ([`parse_model_list`]), and enriches discovered targets
//! ([`build_targets`]). Running the command is the app's job.

use std::collections::{BTreeMap, BTreeSet};

use crate::orchestrator::capability::{CapabilityProfile, MIN_CAPABILITY};
use crate::orchestrator::pricing::PricingTable;
use crate::orchestrator::provider::{
    AgentFramework, ExecutionTarget, ModelId, ModelKind, ModelSpec, Pricing,
};
use crate::orchestrator::task::SubtaskKind;

/// A command that enumerates the models a CLI can currently access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListCommand {
    /// Executable name.
    pub program: String,
    /// Arguments that produce a model listing.
    pub args: Vec<String>,
}

impl ListCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// The best-known default command to list a framework's models, or `None` when the
/// CLI exposes no listing (the app may supply one via configuration).
///
/// Only commands that are actually documented are encoded; we never invent flags.
pub fn default_list_command(framework: AgentFramework) -> Option<ListCommand> {
    match framework {
        // `ollama list` prints a table of locally-available models.
        AgentFramework::Local => Some(ListCommand::new("ollama", &["list"])),
        // `aider --list-models <substr>` lists litellm models matching the substring;
        // "/" matches provider-prefixed ids (the vast majority).
        AgentFramework::Aider => Some(ListCommand::new("aider", &["--list-models", "/"])),
        // `cursor-agent models` prints the models the logged-in account can use.
        AgentFramework::Cursor => Some(ListCommand::new("cursor-agent", &["models"])),
        // Claude Code / Codex / Gemini have no stable list subcommand today; the app
        // can configure one per framework via `ORYN_LIST_<CLI>`. No guessed flags here.
        _ => None,
    }
}

/// Parse a framework's model-listing stdout into model ids, tolerant of format
/// drift and never panicking. Order-preserving and de-duplicated.
pub fn parse_model_list(framework: AgentFramework, lines: &[String]) -> Vec<ModelId> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |id: String| {
        if !id.is_empty() && seen.insert(id.clone()) {
            out.push(ModelId::new(id));
        }
    };

    match framework {
        AgentFramework::Local => {
            // `ollama list` table: first whitespace token per row; skip the header.
            for line in lines {
                let Some(first) = line.split_whitespace().next() else {
                    continue;
                };
                if first == "NAME" {
                    continue;
                }
                push(first.to_string());
            }
        }
        _ => {
            // Bulleted or plain lines; one id per line, no embedded spaces.
            for line in lines {
                let s = line.trim().trim_start_matches(['-', '*', '•']).trim();
                if !s.is_empty() && !s.contains(char::is_whitespace) {
                    push(s.to_string());
                }
            }
        }
    }
    out
}

/// A capability profile assigning `score` to every sub-task kind — the routable
/// baseline for a discovered model we have no benchmark data on yet.
pub fn baseline_profile(score: f64) -> CapabilityProfile {
    let mut p = CapabilityProfile::new();
    for kind in SubtaskKind::ALL {
        p = p.with(kind, score);
    }
    p
}

/// Base capability from the model name's tier (flagship / strong / light), used
/// when there's no benchmark for the model. Deterministic and explainable — a
/// rough prior, not a benchmark, so real leaderboard data always wins when present.
fn name_tier(name: &str) -> f64 {
    const FLAGSHIP: &[&str] = &[
        "opus",
        "gpt-5",
        "o3",
        "sonnet-4",
        "2.5-pro",
        "gemini-2.5-pro",
        "grok-4",
        "deepseek-v3",
        "deepseek-r1",
        "-r1",
    ];
    const STRONG: &[&str] = &[
        "sonnet",
        "gpt-4.1",
        "gpt-4o",
        "gemini-1.5-pro",
        "gemini-pro",
        "qwen",
        "coder",
        "deepseek",
        "mistral-large",
        "llama-3.1-70",
        "llama-3.3",
        "command-r",
        "qwq",
    ];
    const LIGHT: &[&str] = &[
        "mini", "flash", "haiku", "small", "nano", "lite", "1.5b", "3b", "7b", "8b",
    ];
    let has = |set: &[&str]| set.iter().any(|k| name.contains(k));
    if has(FLAGSHIP) {
        0.86
    } else if has(STRONG) {
        0.72
    } else if has(LIGHT) {
        0.55
    } else {
        0.62
    }
}

/// Small adjustment from price: pricier models tend to be more capable; very cheap
/// API models tend to be lighter. Local (zero-priced) models lean on the name only.
fn price_adjust(price: &Pricing) -> f64 {
    if *price == Pricing::ZERO {
        0.0
    } else if price.output >= 30.0 {
        0.04
    } else if price.output > 0.0 && price.output <= 2.0 {
        -0.06
    } else {
        0.0
    }
}

/// Per-kind shaping: capable models pull further ahead on reasoning-heavy kinds
/// (debugging, large-context, refactor) than on mechanical edits.
fn kind_bump(kind: SubtaskKind) -> f64 {
    match kind {
        SubtaskKind::Debugging => 0.03,
        SubtaskKind::LargeContext => 0.02,
        SubtaskKind::Refactor => 0.01,
        SubtaskKind::MechanicalEdit => -0.02,
        SubtaskKind::TestGen | SubtaskKind::DiffEdit => 0.0,
    }
}

/// Estimate a per-kind capability profile for a model from its name tier and
/// price, clamped to at least `floor` (and the routable minimum). Deterministic.
pub fn estimated_profile(model: &ModelId, price: &Pricing, floor: f64) -> CapabilityProfile {
    let name = model.as_str().to_lowercase();
    let base = (name_tier(&name) + price_adjust(price)).clamp(floor.max(MIN_CAPABILITY), 0.95);
    let mut p = CapabilityProfile::new();
    for kind in SubtaskKind::ALL {
        let score = (base + kind_bump(kind)).clamp(MIN_CAPABILITY, 0.97);
        p = p.with(kind, score);
    }
    p
}

/// Enrich discovered `(framework, model)` targets into routable [`ModelSpec`]s plus
/// the capability profiles keyed by the discovered ids.
///
/// - Pricing: local models are free; others are fuzzy-matched against the live
///   `pricing` table (falling back to zero so an unpriced model still routes).
/// - Capability: fuzzy-matched against `profiles`, else a [`baseline_profile`] of
///   `baseline` so a freshly-released model is still a candidate.
///
/// Targets are de-duplicated and returned in deterministic [`ExecutionTarget`]
/// order.
pub fn build_targets(
    discovered: &[(AgentFramework, ModelId)],
    pricing: &PricingTable,
    profiles: &BTreeMap<ModelId, CapabilityProfile>,
    baseline: f64,
) -> (Vec<ModelSpec>, BTreeMap<ModelId, CapabilityProfile>) {
    // Dedup by target, deterministic order.
    let targets: BTreeSet<ExecutionTarget> = discovered
        .iter()
        .map(|(fw, m)| ExecutionTarget::new(*fw, m.clone()))
        .collect();

    let mut specs = Vec::with_capacity(targets.len());
    let mut out_profiles = BTreeMap::new();

    for target in targets {
        let model = &target.model;
        let (kind, default_price) = if target.framework == AgentFramework::Local {
            (
                ModelKind::Local {
                    endpoint: "http://localhost:11434".into(),
                },
                Pricing::ZERO,
            )
        } else {
            (
                ModelKind::Api {
                    provider: target.framework.to_string(),
                },
                Pricing::ZERO,
            )
        };
        let price = if target.framework == AgentFramework::Local {
            Pricing::ZERO
        } else {
            pricing.price_fuzzy(model.as_str()).unwrap_or(default_price)
        };
        let profile = lookup_profile_fuzzy(profiles, model)
            .unwrap_or_else(|| estimated_profile(model, &price, baseline));

        specs.push(ModelSpec {
            id: model.clone(),
            kind,
            pricing: price,
            context_window: 200_000,
            framework: target.framework,
        });
        out_profiles.insert(model.clone(), profile);
    }

    (specs, out_profiles)
}

/// Look up a capability profile by exact id, else by trailing path segment.
fn lookup_profile_fuzzy(
    profiles: &BTreeMap<ModelId, CapabilityProfile>,
    model: &ModelId,
) -> Option<CapabilityProfile> {
    if let Some(p) = profiles.get(model) {
        return Some(p.clone());
    }
    let tail = model.as_str().rsplit('/').next();
    profiles
        .iter()
        .find(|(id, _)| id.as_str().rsplit('/').next() == tail)
        .map(|(_, p)| p.clone())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::capability::MIN_CAPABILITY;
    use crate::orchestrator::catalog::CatalogProvenance;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    // ── list commands ────────────────────────────────────────────────────────

    #[test]
    fn default_list_commands_only_for_documented_clis() {
        assert_eq!(
            default_list_command(AgentFramework::Local).unwrap().program,
            "ollama"
        );
        assert_eq!(
            default_list_command(AgentFramework::Aider).unwrap().program,
            "aider"
        );
        let cursor = default_list_command(AgentFramework::Cursor).unwrap();
        assert_eq!(cursor.program, "cursor-agent");
        assert_eq!(cursor.args, vec!["models".to_string()]);
        assert!(default_list_command(AgentFramework::ClaudeCode).is_none());
        assert!(default_list_command(AgentFramework::Codex).is_none());
    }

    // ── parsing ───────────────────────────────────────────────────────────────

    #[test]
    fn parse_ollama_table_skips_header_takes_name() {
        let out = parse_model_list(
            AgentFramework::Local,
            &lines(&[
                "NAME                ID              SIZE      MODIFIED",
                "qwen2.5-coder:7b    abc123          4.7 GB    2 days ago",
                "deepseek-r1:7b      def456          4.7 GB    1 week ago",
            ]),
        );
        assert_eq!(
            out,
            vec![
                ModelId::new("qwen2.5-coder:7b"),
                ModelId::new("deepseek-r1:7b")
            ]
        );
    }

    #[test]
    fn parse_bulleted_model_list_dedups() {
        let out = parse_model_list(
            AgentFramework::Aider,
            &lines(&[
                "Models which match \"/\":",
                "- anthropic/claude-3-7-sonnet",
                "- openai/gpt-5",
                "- anthropic/claude-3-7-sonnet",
            ]),
        );
        assert_eq!(
            out,
            vec![
                ModelId::new("anthropic/claude-3-7-sonnet"),
                ModelId::new("openai/gpt-5")
            ]
        );
    }

    #[test]
    fn parse_ignores_noise_and_spaced_lines() {
        let out = parse_model_list(
            AgentFramework::Codex,
            &lines(&["", "some prose with spaces", "gpt-5.2"]),
        );
        assert_eq!(out, vec![ModelId::new("gpt-5.2")]);
    }

    // ── baseline ───────────────────────────────────────────────────────────────

    #[test]
    fn baseline_profile_is_routable() {
        let p = baseline_profile(0.6);
        for kind in SubtaskKind::ALL {
            assert!(p.score(kind) >= MIN_CAPABILITY);
        }
    }

    #[test]
    fn estimated_profile_ranks_flagship_above_light_and_respects_floor() {
        let flagship = estimated_profile(&ModelId::new("claude-opus-4-6"), &Pricing::ZERO, 0.5);
        let light = estimated_profile(&ModelId::new("gpt-4o-mini"), &Pricing::ZERO, 0.5);
        // A flagship out-scores a light model on the reasoning-heavy kind.
        assert!(flagship.score(SubtaskKind::Debugging) > light.score(SubtaskKind::Debugging));
        // Every kind stays routable and within bounds.
        for kind in SubtaskKind::ALL {
            assert!((MIN_CAPABILITY..=0.97).contains(&flagship.score(kind)));
            assert!(light.score(kind) >= 0.5_f64.max(MIN_CAPABILITY) - 0.06 - 1e-9);
        }
        // Deterministic.
        assert_eq!(
            estimated_profile(&ModelId::new("claude-opus-4-6"), &Pricing::ZERO, 0.5).scores,
            flagship.scores
        );
    }

    #[test]
    fn estimated_profile_price_signal_moves_score() {
        let pricey = Pricing {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75,
        };
        let cheap = Pricing {
            input: 0.1,
            output: 0.4,
            cache_read: 0.0,
            cache_write: 0.0,
        };
        let model = ModelId::new("some-unknown-model");
        let hi = estimated_profile(&model, &pricey, 0.0).score(SubtaskKind::DiffEdit);
        let lo = estimated_profile(&model, &cheap, 0.0).score(SubtaskKind::DiffEdit);
        assert!(hi > lo, "pricier model estimates higher ({hi} vs {lo})");
    }

    // ── enrichment ───────────────────────────────────────────────────────────

    fn pricing_table() -> PricingTable {
        let body = r#"{"data":[{"id":"anthropic/claude-3-7-sonnet","pricing":{"prompt":"0.000003","completion":"0.000015"}}]}"#;
        PricingTable {
            prices: crate::orchestrator::pricing::parse_openrouter_models(body).unwrap(),
            provenance: CatalogProvenance {
                source: "openrouter".into(),
                fetched_at_unix: 1,
                version: "v1".into(),
            },
        }
    }

    #[test]
    fn build_targets_prices_and_profiles_dynamically() {
        let discovered = vec![
            (
                AgentFramework::ClaudeCode,
                ModelId::new("anthropic/claude-3-7-sonnet"),
            ),
            (AgentFramework::Local, ModelId::new("qwen2.5-coder:7b")),
        ];
        let (specs, profiles) = build_targets(&discovered, &pricing_table(), &BTreeMap::new(), 0.6);
        assert_eq!(specs.len(), 2);

        let sonnet = specs
            .iter()
            .find(|s| s.framework == AgentFramework::ClaudeCode)
            .unwrap();
        assert!(
            (sonnet.pricing.input - 3.0).abs() < 1e-6,
            "priced from the live table"
        );
        let local = specs
            .iter()
            .find(|s| s.framework == AgentFramework::Local)
            .unwrap();
        assert_eq!(local.pricing, Pricing::ZERO, "local is free");

        // Every discovered model gets a routable baseline profile.
        assert!(
            profiles[&ModelId::new("qwen2.5-coder:7b")].score(SubtaskKind::Debugging)
                >= MIN_CAPABILITY
        );
    }

    #[test]
    fn build_targets_dedups_and_is_deterministic() {
        let discovered = vec![
            (AgentFramework::Local, ModelId::new("m")),
            (AgentFramework::Local, ModelId::new("m")),
        ];
        let a = build_targets(&discovered, &PricingTable::seed(), &BTreeMap::new(), 0.6);
        let b = build_targets(&discovered, &PricingTable::seed(), &BTreeMap::new(), 0.6);
        assert_eq!(a.0.len(), 1);
        assert_eq!(a, b);
    }

    #[test]
    fn build_targets_uses_known_profile_when_id_matches() {
        let mut profiles = BTreeMap::new();
        profiles.insert(ModelId::new("opus"), baseline_profile(0.95));
        // Discovered id ends with the known id's trailing segment.
        let discovered = vec![(AgentFramework::ClaudeCode, ModelId::new("anthropic/opus"))];
        let (_, out) = build_targets(&discovered, &PricingTable::seed(), &profiles, 0.5);
        assert!(
            (out[&ModelId::new("anthropic/opus")].score(SubtaskKind::Debugging) - 0.95).abs()
                < 1e-9
        );
    }
}

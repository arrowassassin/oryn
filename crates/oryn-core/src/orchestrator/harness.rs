//! Headless CLI invocation construction — how Oryn "triggers" a real agent.
//!
//! Each [`ExecutionTarget`] `(framework, model)` is executed by spawning that
//! vendor's **headless CLI** with the chosen model, fed the universal
//! cache-stable context. This module builds the *exact* command — program, args,
//! env, stdin, working directory — as a **pure, deterministic** value
//! ([`HarnessInvocation`]); spawning lives behind the `ProcessRunner` trait in
//! [`super::runner`].
//!
//! Riding the vendor CLI (rather than the raw model API) means Oryn inherits the
//! harness's agentic loop, tool use, and — crucially — the user's **existing
//! subscription/OAuth login**; an API key is only a fallback (see [`AuthMode`]).
//!
//! The headless surfaces (confirmed against current vendor docs):
//!
//! | Framework | program | model flag | output | prompt via |
//! |---|---|---|---|---|
//! | Claude Code | `claude -p` | `--model` | `--output-format stream-json --verbose` | stdin |
//! | Cursor | `cursor-agent -p` | `--model` | `--output-format json` | positional |
//! | Codex | `codex exec` | `--model` | `--json` | positional |
//! | Gemini CLI | `gemini` | `--model` | `--output-format json` | `-p` flag |
//! | aider | `aider` | `--model` | text (`--no-stream`) | `--message` flag |
//! | Local | `ollama run <model>` | (positional) | text | stdin |
//!
//! Auto-approve/sandbox flags are version-sensitive and intentionally
//! conservative; they are the one place to adjust when a vendor renames a flag.

use std::path::{Path, PathBuf};

use crate::orchestrator::provider::{AgentFramework, ExecutionTarget};

/// How a headless run authenticates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    /// Use the harness's on-disk subscription/OAuth login. No extra env injected —
    /// the CLI already holds the user's credentials (the preferred path).
    Subscription,
    /// Inject an API key into the child's environment under `var`. Used only when
    /// a framework has no logged-in session (the fallback path).
    ApiKey { var: String, value: String },
}

/// A fully-resolved, deterministic command to launch one framework's headless
/// harness for one model against the universal context.
///
/// Equality is structural so tests can pin the exact command. `env` is kept
/// sorted by key so the value is order-independent and reproducible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessInvocation {
    /// Executable name (resolved against `PATH` by the runner).
    pub program: String,
    /// Arguments in launch order.
    pub args: Vec<String>,
    /// Environment overrides, sorted by key.
    pub env: Vec<(String, String)>,
    /// Prompt delivered on stdin, when the framework reads the prompt that way.
    pub stdin: Option<String>,
    /// Working directory — the target's isolated worktree.
    pub cwd: PathBuf,
}

/// Join the cache-stable `prefix` and the volatile per-subtask `suffix` into the
/// prompt the harness receives.
///
/// The prefix is emitted **verbatim and first** so each vendor's prompt cache hits
/// on the identical leading bytes; the suffix follows after a blank line. An empty
/// suffix yields just the prefix (no trailing separator) to stay byte-stable.
pub fn combined_prompt(prefix: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}\n\n{suffix}")
    }
}

/// Build the headless invocation for `target`, feeding `prefix` + `suffix` as the
/// universal context, running in `workdir`, authenticated per `auth`.
///
/// Pure and deterministic: identical inputs always yield an identical
/// [`HarnessInvocation`].
pub fn build_invocation(
    target: &ExecutionTarget,
    prefix: &str,
    suffix: &str,
    workdir: &Path,
    auth: &AuthMode,
) -> HarnessInvocation {
    let model = target.model.as_str();
    let prompt = combined_prompt(prefix, suffix);

    let (program, args, stdin): (&str, Vec<String>, Option<String>) = match target.framework {
        AgentFramework::ClaudeCode => (
            "claude",
            args(&["-p", "--model", model, "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"]),
            Some(prompt),
        ),
        AgentFramework::Cursor => (
            "cursor-agent",
            args(&["-p", "--model", model, "--output-format", "json", &prompt]),
            None,
        ),
        AgentFramework::Codex => (
            "codex",
            args(&["exec", "--model", model, "--json", "--ask-for-approval", "never", "--sandbox", "workspace-write", &prompt]),
            None,
        ),
        AgentFramework::GeminiCli => (
            "gemini",
            args(&["--model", model, "--output-format", "json", "--yolo", "-p", &prompt]),
            None,
        ),
        AgentFramework::Aider => (
            "aider",
            args(&["--model", model, "--yes", "--no-stream", "--message", &prompt]),
            None,
        ),
        AgentFramework::Local => (
            // Ollama's one-shot CLI: `ollama run <model>` reads the prompt on stdin.
            "ollama",
            args(&["run", model]),
            Some(prompt),
        ),
    };

    HarnessInvocation {
        program: program.to_string(),
        args,
        env: auth_env(auth),
        stdin,
        cwd: workdir.to_path_buf(),
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Environment overrides for `auth`, sorted by key for determinism.
fn auth_env(auth: &AuthMode) -> Vec<(String, String)> {
    match auth {
        AuthMode::Subscription => Vec::new(),
        AuthMode::ApiKey { var, value } => vec![(var.clone(), value.clone())],
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::provider::ModelId;
    use std::path::PathBuf;

    fn target(framework: AgentFramework, model: &str) -> ExecutionTarget {
        ExecutionTarget::new(framework, ModelId::new(model))
    }

    fn wd() -> PathBuf {
        PathBuf::from("/work/oryn-claude")
    }

    fn build(fw: AgentFramework, model: &str, prefix: &str, suffix: &str) -> HarnessInvocation {
        build_invocation(&target(fw, model), prefix, suffix, &wd(), &AuthMode::Subscription)
    }

    // ── combined_prompt ──────────────────────────────────────────────────────

    #[test]
    fn combined_prompt_joins_with_blank_line() {
        assert_eq!(combined_prompt("PREFIX", "do it"), "PREFIX\n\ndo it");
    }

    #[test]
    fn combined_prompt_empty_suffix_is_prefix_only() {
        assert_eq!(combined_prompt("PREFIX", ""), "PREFIX");
    }

    // ── per-framework command shape ──────────────────────────────────────────

    #[test]
    fn claude_uses_stream_json_model_and_stdin_prompt() {
        let inv = build(AgentFramework::ClaudeCode, "claude-opus-4-6", "CTX", "fix it");
        assert_eq!(inv.program, "claude");
        assert!(inv.args.contains(&"-p".to_string()));
        // model is selected explicitly
        let m = inv.args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(inv.args[m + 1], "claude-opus-4-6");
        // structured streaming output
        let o = inv.args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(inv.args[o + 1], "stream-json");
        assert!(inv.args.contains(&"--verbose".to_string()));
        // prompt on stdin, verbatim prefix first
        assert_eq!(inv.stdin.as_deref(), Some("CTX\n\nfix it"));
        assert_eq!(inv.cwd, wd());
    }

    #[test]
    fn cursor_passes_prompt_positionally_with_json() {
        let inv = build(AgentFramework::Cursor, "sonnet-4", "CTX", "fix it");
        assert_eq!(inv.program, "cursor-agent");
        assert_eq!(inv.args.last().unwrap(), "CTX\n\nfix it");
        let o = inv.args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(inv.args[o + 1], "json");
        assert!(inv.stdin.is_none());
    }

    #[test]
    fn codex_uses_exec_subcommand_and_json() {
        let inv = build(AgentFramework::Codex, "gpt-5.2", "CTX", "fix it");
        assert_eq!(inv.program, "codex");
        assert_eq!(inv.args.first().unwrap(), "exec");
        assert!(inv.args.contains(&"--json".to_string()));
        assert_eq!(inv.args.last().unwrap(), "CTX\n\nfix it");
    }

    #[test]
    fn gemini_uses_prompt_flag_and_json() {
        let inv = build(AgentFramework::GeminiCli, "gemini-3-pro", "CTX", "fix it");
        assert_eq!(inv.program, "gemini");
        let p = inv.args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(inv.args[p + 1], "CTX\n\nfix it");
        assert!(inv.args.contains(&"--yolo".to_string()));
    }

    #[test]
    fn aider_uses_message_flag_and_yes() {
        let inv = build(AgentFramework::Aider, "gpt-5.2", "CTX", "fix it");
        assert_eq!(inv.program, "aider");
        assert!(inv.args.contains(&"--yes".to_string()));
        let mfl = inv.args.iter().position(|a| a == "--message").unwrap();
        assert_eq!(inv.args[mfl + 1], "CTX\n\nfix it");
    }

    #[test]
    fn local_uses_ollama_run_with_stdin() {
        let inv = build(AgentFramework::Local, "qwen2.5-coder", "CTX", "fix it");
        assert_eq!(inv.program, "ollama");
        assert_eq!(inv.args, vec!["run".to_string(), "qwen2.5-coder".to_string()]);
        assert_eq!(inv.stdin.as_deref(), Some("CTX\n\nfix it"));
    }

    // ── auth ──────────────────────────────────────────────────────────────────

    #[test]
    fn subscription_injects_no_env() {
        let inv = build(AgentFramework::ClaudeCode, "opus", "C", "s");
        assert!(inv.env.is_empty(), "subscription rides the on-disk login");
    }

    #[test]
    fn api_key_is_injected_as_env() {
        let inv = build_invocation(
            &target(AgentFramework::ClaudeCode, "opus"),
            "C",
            "s",
            &wd(),
            &AuthMode::ApiKey { var: "ANTHROPIC_API_KEY".into(), value: "sk-xyz".into() },
        );
        assert_eq!(inv.env, vec![("ANTHROPIC_API_KEY".to_string(), "sk-xyz".to_string())]);
    }

    // ── determinism ────────────────────────────────────────────────────────────

    #[test]
    fn build_is_deterministic() {
        let a = build(AgentFramework::ClaudeCode, "opus", "CTX", "fix it");
        let b = build(AgentFramework::ClaudeCode, "opus", "CTX", "fix it");
        assert_eq!(a, b);
    }

    #[test]
    fn prefix_appears_verbatim_and_first_for_cache_stability() {
        // The cache-stable region must be the leading bytes of the prompt, byte
        // for byte, or the vendor prompt cache misses.
        let prefix = "SYSTEM\n\nrepo-map\n\nTASK";
        let inv = build(AgentFramework::ClaudeCode, "opus", prefix, "the subtask");
        let prompt = inv.stdin.unwrap();
        assert!(prompt.starts_with(prefix));
        assert_eq!(&prompt[..prefix.len()], prefix);
    }

    #[test]
    fn every_framework_selects_its_model() {
        for fw in [
            AgentFramework::ClaudeCode,
            AgentFramework::Cursor,
            AgentFramework::Codex,
            AgentFramework::GeminiCli,
            AgentFramework::Aider,
            AgentFramework::Local,
        ] {
            let inv = build(fw, "the-model", "C", "s");
            assert!(
                inv.args.iter().any(|a| a == "the-model"),
                "{fw} must pass the model to the CLI"
            );
        }
    }
}

//! `oryn` — compile less, test less, trust the results.

mod cov;
mod render;
mod runner;
mod tui;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "oryn",
    version,
    about = "Compile less, test less, trust the results — safe test-impact selection, \
             cached test results, and statistical flaky scoring for Cargo."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show which crates a change can affect (safe, crate-level selection).
    Affected {
        /// Compare against this git ref instead of the working tree.
        #[arg(long)]
        since: Option<String>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run tests for only the affected crates, skipping ones cached green.
    Test {
        /// Compare against this git ref instead of the working tree.
        #[arg(long)]
        since: Option<String>,
        /// Test the whole workspace regardless of the diff.
        #[arg(long)]
        all: bool,
        /// Ignore the green-result cache and re-run everything selected.
        #[arg(long)]
        no_cache: bool,
        /// Use sccache as the compile cache (RUSTC_WRAPPER) if available.
        #[arg(long)]
        cache: bool,
        /// Function-level selection: run only tests whose recorded coverage
        /// intersects the change (needs `oryn cover` first).
        #[arg(long = "fn")]
        function: bool,
        /// Extra args passed through to the test runner (after `--`).
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Record per-test coverage for function-level selection (`oryn cover`).
    Cover {
        /// Limit to crates affected since this git ref (default: all).
        #[arg(long)]
        since: Option<String>,
    },
    /// Build only the affected crates (a faster `cargo build`).
    Build {
        /// Compare against this git ref instead of the working tree.
        #[arg(long)]
        since: Option<String>,
        /// Build the whole workspace regardless of the diff.
        #[arg(long)]
        all: bool,
        /// Compile the affected crates' **test** binaries (what `oryn test`
        /// runs) instead of their lib/bin targets — front-loads compilation so
        /// a later `oryn test` is run-only and doesn't recompile.
        #[arg(long)]
        tests: bool,
        /// Use sccache as the compile cache (RUSTC_WRAPPER) if available.
        #[arg(long)]
        cache: bool,
        /// Extra args passed through to cargo (after `--`).
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Score flaky tests (from accumulated history, or a rerun-history file).
    Flaky {
        /// Optional `{"id","passes","fails"}` JSONL/array file; defaults to the
        /// history Oryn recorded from past `oryn test` runs.
        #[arg(long)]
        input: Option<PathBuf>,
        /// Confidence/credible level (0..1).
        #[arg(long, default_value_t = 0.95, value_parser = unit_interval)]
        level: f64,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reruns needed to confirm a flake of a given rate at a confidence level.
    Budget {
        /// Per-run failure probability (0..1).
        #[arg(long, value_parser = unit_interval)]
        fail_rate: f64,
        /// Target confidence (0..1).
        #[arg(long, default_value_t = 0.95, value_parser = unit_interval)]
        confidence: f64,
    },
    /// Open the terminal dashboard (selection, cache, crates, flaky stats).
    Tui {
        /// Compare against this git ref instead of the working tree.
        #[arg(long)]
        since: Option<String>,
        /// Confidence/credible level for the flaky view.
        #[arg(long, default_value_t = 0.95, value_parser = unit_interval)]
        level: f64,
    },
    /// Enable rich per-test collection (writes a nextest JUnit profile).
    Setup,
    /// Detect proven compile-time speedups that aren't enabled here.
    Tune {
        /// Write the recommended sound config to `.cargo/config.toml` (only if
        /// none exists — never clobbers an existing file).
        #[arg(long)]
        apply: bool,
    },
    /// Show the sccache compile-cache statistics (hits/misses).
    Cache,
    /// Show version and detected tooling.
    Info,
}

/// Parse a probability/confidence argument, rejecting values outside `0..=1`
/// (and NaN) before they reach the statistics.
fn unit_interval(s: &str) -> std::result::Result<f64, String> {
    let v: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("must be between 0 and 1, got {v}"))
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Affected { since, json } => runner::affected(since.as_deref(), json),
        Cmd::Test {
            since,
            all,
            no_cache,
            cache,
            function,
            extra,
        } => {
            if function {
                cov::test_fn(&extra)
            } else {
                runner::test(since.as_deref(), all, no_cache, cache, &extra)
            }
        }
        Cmd::Cover { since } => cov::cover(since.as_deref()),
        Cmd::Build {
            since,
            all,
            tests,
            cache,
            extra,
        } => runner::build(since.as_deref(), all, tests, cache, &extra),
        Cmd::Flaky { input, level, json } => runner::flaky(input.as_deref(), level, json),
        Cmd::Budget {
            fail_rate,
            confidence,
        } => {
            match oryn_core::flaky::required_reruns(fail_rate, confidence) {
                Some(n) => println!(
                    "To catch a {:.2}% flake with {:.0}% confidence, run each test {} time(s).",
                    fail_rate * 100.0,
                    confidence * 100.0,
                    n
                ),
                None => println!("A 0% failure rate can never be surfaced by reruns."),
            }
            Ok(())
        }
        Cmd::Tui { since, level } => tui::run(since.as_deref(), level),
        Cmd::Setup => runner::setup(),
        Cmd::Tune { apply } => runner::tune(apply),
        Cmd::Cache => runner::cache_stats(),
        Cmd::Info => {
            println!("oryn      {}", oryn_core::VERSION);
            println!(
                "nextest   {}",
                if runner::has("cargo", &["nextest", "--version"]) {
                    "found"
                } else {
                    "not found"
                }
            );
            println!(
                "mold      {}",
                if runner::has("mold", &["--version"]) {
                    "found"
                } else {
                    "not found"
                }
            );
            println!(
                "sccache   {}",
                if runner::has("sccache", &["--version"]) {
                    "found"
                } else {
                    "not found"
                }
            );
            Ok(())
        }
    }
    .context("oryn failed")
}

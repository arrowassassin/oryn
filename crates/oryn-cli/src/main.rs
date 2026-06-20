//! `oryn` — compile less, test less, trust the results.
//!
//! * `oryn affected` — show which crates a change can affect (safe selection)
//! * `oryn test`     — run tests for only the affected crates
//! * `oryn flaky`    — statistically score flaky tests from rerun history
//! * `oryn budget`   — how many reruns to confirm a flake at a confidence level
//! * `oryn tune`     — detect proven compile-time speedups you haven't enabled
//! * `oryn info`     — versions

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oryn_core::flaky::{self, TestRuns};
use oryn_core::{git, metadata, select};

#[derive(Parser)]
#[command(
    name = "oryn",
    version,
    about = "Compile less, test less, trust the results — safe test-impact selection + statistical flaky scoring for Cargo."
)]
struct Cli {
    #[command(subcommand)]
    command: Command_,
}

#[derive(Subcommand)]
enum Command_ {
    /// Show which crates a change can affect (safe, crate-level selection).
    Affected {
        /// Compare against this git ref instead of the working tree.
        #[arg(long)]
        since: Option<String>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run `cargo test` for only the affected crates.
    Test {
        /// Compare against this git ref instead of the working tree.
        #[arg(long)]
        since: Option<String>,
        /// Test the whole workspace regardless of the diff.
        #[arg(long)]
        all: bool,
        /// Use `cargo nextest run` instead of `cargo test`.
        #[arg(long)]
        nextest: bool,
        /// Extra args passed through to cargo (after `--`).
        #[arg(last = true)]
        cargo_args: Vec<String>,
    },
    /// Score flaky tests from rerun history ({"id","passes","fails"} JSONL/array).
    Flaky {
        /// Path to the rerun-history file.
        #[arg(long)]
        input: PathBuf,
        /// Confidence level for the intervals.
        #[arg(long, default_value_t = 0.95)]
        level: f64,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reruns needed to confirm a flake of a given rate at a confidence level.
    Budget {
        /// Per-run failure probability (0..1).
        #[arg(long)]
        fail_rate: f64,
        /// Target confidence (0..1).
        #[arg(long, default_value_t = 0.95)]
        confidence: f64,
    },
    /// Detect proven compile-time speedups that aren't enabled here.
    Tune,
    /// Show version.
    Info,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command_::Affected { since, json } => affected(since.as_deref(), json),
        Command_::Test {
            since,
            all,
            nextest,
            cargo_args,
        } => test(since.as_deref(), all, nextest, &cargo_args),
        Command_::Flaky { input, level, json } => run_flaky(input, level, json),
        Command_::Budget {
            fail_rate,
            confidence,
        } => budget(fail_rate, confidence),
        Command_::Tune => tune(),
        Command_::Info => {
            println!("oryn {}", oryn_core::VERSION);
            Ok(())
        }
    }
}

fn build_plan(since: Option<&str>) -> Result<(PathBuf, select::SelectionPlan)> {
    let cwd = std::env::current_dir()?;
    let graph = metadata::load(&cwd).context("loading cargo metadata")?;
    let root = git::repo_root(&cwd).context("finding git repo root")?;
    let changed = git::changed_files(&root, since).context("listing changed files")?;
    let plan = select::plan(&graph, &root, &changed);
    Ok((root, plan))
}

fn affected(since: Option<&str>, json: bool) -> Result<()> {
    let (_root, plan) = build_plan(since)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    println!("{}", plan.reason);
    if plan.ignored_files > 0 {
        println!(
            "  {} changed file(s) belong to no crate (docs/CI) — ignored",
            plan.ignored_files
        );
    }
    if plan.affected_crates.is_empty() {
        println!("  → nothing to test");
    } else {
        println!(
            "  affected ({}): {}",
            plan.affected_crates.len(),
            plan.affected_crates.join(", ")
        );
        if !plan.skipped_crates.is_empty() {
            println!(
                "  skipped  ({}): {}",
                plan.skipped_crates.len(),
                plan.skipped_crates.join(", ")
            );
        }
    }
    Ok(())
}

fn test(since: Option<&str>, all: bool, nextest: bool, extra: &[String]) -> Result<()> {
    let (_root, plan) = build_plan(since)?;
    let mut cmd = Command::new("cargo");
    if nextest {
        cmd.args(["nextest", "run"]);
    } else {
        cmd.arg("test");
    }

    if all || plan.select_all {
        eprintln!(
            "oryn: {}",
            if all {
                "--all set"
            } else {
                plan.reason.as_str()
            }
        );
        cmd.arg("--workspace");
    } else if plan.affected_crates.is_empty() {
        println!(
            "oryn: nothing affected — skipped all {} crate(s) ✓",
            plan.skipped_crates.len()
        );
        return Ok(());
    } else {
        eprintln!(
            "oryn: testing {} affected crate(s), skipping {} ✓",
            plan.affected_crates.len(),
            plan.skipped_crates.len()
        );
        for c in &plan.affected_crates {
            cmd.args(["-p", c]);
        }
    }
    cmd.args(extra);

    let status = cmd.status().context("running cargo")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn read_runs(path: &PathBuf) -> Result<Vec<TestRuns>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let trimmed = raw.trim_start();
    if trimmed.starts_with('[') {
        Ok(serde_json::from_str(&raw)?)
    } else {
        let mut out = Vec::new();
        for (n, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            out.push(
                serde_json::from_str(line)
                    .with_context(|| format!("{}:{}: invalid JSON", path.display(), n + 1))?,
            );
        }
        Ok(out)
    }
}

fn run_flaky(input: PathBuf, level: f64, json: bool) -> Result<()> {
    let runs = read_runs(&input)?;
    let report = flaky::analyze(&runs, level);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{} test(s): {} flaky, {} always-failing",
        report.tests.len(),
        report.flaky_count,
        report.always_fail_count
    );
    for t in &report.tests {
        match t.verdict {
            flaky::FlakyVerdict::Flaky => println!(
                "  FLAKY  {}  rate={:.1}% (95% CI {:.1}%–{:.1}%), ~{} reruns to reproduce",
                t.id,
                t.flake_rate * 100.0,
                t.ci.low * 100.0,
                t.ci.high * 100.0,
                t.reruns_to_reproduce_95.unwrap_or(0)
            ),
            flaky::FlakyVerdict::StablePass => println!(
                "  pass   {}  ({} runs; flake rate only proven < {:.1}%)",
                t.id,
                t.runs,
                t.proven_below * 100.0
            ),
            flaky::FlakyVerdict::StableFail => {
                println!("  FAIL   {}  ({} runs, all failed)", t.id, t.runs)
            }
            flaky::FlakyVerdict::Unknown => println!("  ?      {}  (no runs)", t.id),
        }
    }
    Ok(())
}

fn budget(fail_rate: f64, confidence: f64) -> Result<()> {
    match flaky::required_reruns(fail_rate, confidence) {
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

fn tune() -> Result<()> {
    fn has(bin: &str) -> bool {
        Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    println!("Compile-time tuning (proven, low-risk wins):");
    let mold = has("mold");
    let lld = has("ld.lld") || has("lld");
    if mold {
        println!("  ✓ mold found — set a faster linker in .cargo/config.toml:");
        println!("      [target.x86_64-unknown-linux-gnu]");
        println!("      linker = \"clang\"");
        println!("      rustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]");
    } else if lld {
        println!("  ✓ lld found — use it as the linker (rustflags: -C link-arg=-fuse-ld=lld)");
    } else {
        println!("  • no fast linker found — installing `mold` is the single biggest win for incremental rebuilds");
    }
    if has("sccache") {
        println!("  ✓ sccache found — set RUSTC_WRAPPER=sccache in CI to cache crate builds");
    } else {
        println!(
            "  • sccache not found — `cargo install sccache` to cache compilation across CI runs"
        );
    }
    println!("  • for local debug iteration: try the Cranelift backend (~20% faster codegen)");
    println!(
        "  • split large crates: a crate is the unit of caching *and* of `oryn` test selection"
    );
    Ok(())
}

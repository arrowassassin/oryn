//! Subprocess orchestration for the `oryn` subcommands.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use oryn_core::flaky::TestRuns;
use oryn_core::graph::WorkspaceGraph;
use oryn_core::runner::attribute_crates;
use oryn_core::select::SelectionPlan;
use oryn_core::store::{self, Store};
use oryn_core::{fingerprint, flaky, git, junit, metadata, select};

use crate::render;

/// Does `bin args…` run successfully?
#[must_use]
pub fn has(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rustc_version() -> String {
    Command::new("rustc")
        .arg("-vV")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_else(|| "rustc-unknown".to_string())
}

/// Load the workspace graph, repo root, and a selection plan for `since`.
fn context(since: Option<&str>) -> Result<(WorkspaceGraph, PathBuf, SelectionPlan)> {
    let cwd = std::env::current_dir()?;
    let graph = metadata::load(&cwd).context("loading cargo metadata")?;
    let root = git::repo_root(&cwd).context("finding git repo root")?;
    let changed = git::changed_files(&root, since).context("listing changed files")?;
    let plan = select::plan(&graph, &root, &changed);
    Ok((graph, root, plan))
}

/// The crates we should consider testing/building.
fn candidates(graph: &WorkspaceGraph, plan: &SelectionPlan, all: bool) -> Vec<String> {
    if all {
        graph.names(&graph.all_indices())
    } else {
        plan.affected_crates.clone()
    }
}

/// `oryn affected`
pub fn affected(since: Option<&str>, json: bool) -> Result<()> {
    let (_g, _r, plan) = context(since)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        render::plan(&plan);
    }
    Ok(())
}

/// `oryn test`
pub fn test(since: Option<&str>, all: bool, no_cache: bool, extra: &[String]) -> Result<()> {
    let (graph, _root, plan) = context(since)?;
    let cands = candidates(&graph, &plan, all);
    if cands.is_empty() {
        println!("oryn: {} — nothing to test ✓", plan.reason);
        return Ok(());
    }

    let ver = rustc_version();
    let fps = fingerprint::compute(&graph, &ver).context("fingerprinting crates")?;
    let store_dir = Store::dir_for(&graph.root);
    let mut st = Store::load(&store_dir).context("loading oryn store")?;

    let to_run: Vec<String> = cands
        .iter()
        .filter(|c| no_cache || !is_green(&st, c, &fps))
        .cloned()
        .collect();
    let cached = cands.len() - to_run.len();

    if to_run.is_empty() {
        println!(
            "oryn: all {} affected crate(s) are cached green — nothing to run ✓",
            cached
        );
        return Ok(());
    }
    eprintln!(
        "oryn: {} affected, {} cached green, running {} ▶ {}",
        cands.len(),
        cached,
        to_run.len(),
        to_run.join(", ")
    );

    let now = store::now_unix();
    let use_nextest = has("cargo", &["nextest", "--version"]) && nextest_profile(&graph.root);

    let status = if use_nextest {
        run_tests_nextest(&graph, &to_run, &fps, &mut st, now, extra)?
    } else {
        run_tests_cargo(&to_run, &fps, &mut st, now, extra)?
    };

    st.save(&store_dir).context("saving oryn store")?;

    match status {
        0 => println!(
            "oryn: {} crate(s) green ✓ ({} skipped via cache)",
            to_run.len(),
            cached
        ),
        code => {
            println!("oryn: tests failed (exit {code})");
            std::process::exit(code);
        }
    }
    Ok(())
}

fn is_green(
    st: &Store,
    crate_name: &str,
    fps: &std::collections::BTreeMap<String, String>,
) -> bool {
    fps.get(crate_name)
        .is_some_and(|fp| st.is_green(crate_name, fp))
}

fn record_green(
    st: &mut Store,
    crate_name: &str,
    fps: &std::collections::BTreeMap<String, String>,
    now: u64,
    passed: bool,
) {
    match (passed, fps.get(crate_name)) {
        (true, Some(fp)) => st.record_green(crate_name, fp, now),
        _ => st.clear_green(crate_name),
    }
}

fn run_tests_nextest(
    graph: &WorkspaceGraph,
    to_run: &[String],
    fps: &std::collections::BTreeMap<String, String>,
    st: &mut Store,
    now: u64,
    extra: &[String],
) -> Result<i32> {
    let mut cmd = Command::new("cargo");
    cmd.args(["nextest", "run", "--profile", "oryn"]);
    for c in to_run {
        cmd.args(["-p", c]);
    }
    cmd.args(extra);
    let status = cmd.status().context("running cargo nextest")?;

    let junit = graph.root.join("target/nextest/oryn/junit.xml");
    if let Ok(bytes) = std::fs::read(&junit) {
        let outcomes = junit::parse(&bytes)?;
        for o in &outcomes {
            st.observe_test(&o.id, o.passed, now, o.duration_ms);
        }
        for (c, passed) in attribute_crates(&outcomes, to_run) {
            record_green(st, &c, fps, now, passed);
        }
    } else {
        // No JUnit (profile not writing?) — fall back to aggregate status.
        let ok = status.success();
        for c in to_run {
            record_green(st, c, fps, now, ok);
        }
    }
    Ok(status.code().unwrap_or(1))
}

fn run_tests_cargo(
    to_run: &[String],
    fps: &std::collections::BTreeMap<String, String>,
    st: &mut Store,
    now: u64,
    extra: &[String],
) -> Result<i32> {
    let mut cmd = Command::new("cargo");
    cmd.arg("test");
    for c in to_run {
        cmd.args(["-p", c]);
    }
    cmd.args(extra);
    let status = cmd.status().context("running cargo test")?;
    // Aggregate result: we cannot attribute per crate, so be conservative —
    // mark green only if the whole run passed; otherwise forget green.
    let ok = status.success();
    for c in to_run {
        record_green(st, c, fps, now, ok);
    }
    Ok(status.code().unwrap_or(1))
}

/// `oryn build`
pub fn build(since: Option<&str>, all: bool, extra: &[String]) -> Result<()> {
    let (graph, _root, plan) = context(since)?;
    let cands = candidates(&graph, &plan, all);
    if cands.is_empty() {
        println!("oryn: {} — nothing to build ✓", plan.reason);
        return Ok(());
    }
    eprintln!(
        "oryn: building {} affected crate(s) ▶ {}",
        cands.len(),
        cands.join(", ")
    );
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    for c in &cands {
        cmd.args(["-p", c]);
    }
    cmd.args(extra);
    let status = cmd.status().context("running cargo build")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `oryn flaky`
pub fn flaky(input: Option<&Path>, level: f64, json: bool) -> Result<()> {
    let runs: Vec<TestRuns> = match input {
        Some(path) => read_runs(path)?,
        None => {
            let cwd = std::env::current_dir()?;
            let graph = metadata::load(&cwd)?;
            let st = Store::load(&Store::dir_for(&graph.root))?;
            st.tests
                .iter()
                .map(|(id, r)| TestRuns::new(id.clone(), r.passes, r.fails))
                .collect()
        }
    };
    if runs.is_empty() {
        println!(
            "No test history yet. Run `oryn setup` then `oryn test` to collect per-test data."
        );
        return Ok(());
    }
    let report = flaky::analyze(&runs, level);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render::flaky(&report);
    }
    Ok(())
}

fn read_runs(path: &Path) -> Result<Vec<TestRuns>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if raw.trim_start().starts_with('[') {
        Ok(serde_json::from_str(&raw)?)
    } else {
        let mut out = Vec::new();
        for (n, line) in raw.lines().enumerate() {
            let line = line.trim();
            if !line.is_empty() {
                out.push(
                    serde_json::from_str(line)
                        .with_context(|| format!("{}:{}: invalid JSON", path.display(), n + 1))?,
                );
            }
        }
        Ok(out)
    }
}

fn nextest_profile(root: &Path) -> bool {
    std::fs::read_to_string(root.join(".config/nextest.toml"))
        .map(|s| s.contains("profile.oryn"))
        .unwrap_or(false)
}

/// `oryn setup`
pub fn setup() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = git::repo_root(&cwd).unwrap_or(cwd);
    let path = root.join(".config/nextest.toml");
    const SNIPPET: &str = "\n[profile.oryn]\n# Written by `oryn setup` — enables per-test result collection.\n[profile.oryn.junit]\npath = \"junit.xml\"\n";
    if path.exists() {
        let existing = std::fs::read_to_string(&path)?;
        if existing.contains("profile.oryn") {
            println!(
                "✓ nextest 'oryn' profile already present in {}",
                path.display()
            );
        } else {
            std::fs::write(&path, format!("{existing}{SNIPPET}"))?;
            println!("✓ added 'oryn' profile to {}", path.display());
        }
    } else {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, SNIPPET.trim_start())?;
        println!("✓ created {} with the 'oryn' profile", path.display());
    }
    println!("Now `oryn test` will collect per-test history (needs cargo-nextest installed).");
    Ok(())
}

/// `oryn tune`
pub fn tune() -> Result<()> {
    println!("Compile-time tuning (proven, low-risk wins):\n");

    println!("Linker:");
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        println!("  • rust-lld is the DEFAULT linker here since Rust 1.90 — already fast.");
        if has("mold", &["--version"]) {
            println!("  ✓ mold found — for an extra edge, .cargo/config.toml:");
            println!("      [target.x86_64-unknown-linux-gnu]");
            println!("      linker = \"clang\"");
            println!("      rustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]");
        } else {
            println!("  • optional: install `mold` for the fastest large-link times.");
        }
    } else if cfg!(target_os = "macos") {
        println!(
            "  • macOS: the default `ld-prime` (Xcode 15+) is already the fast path — no change."
        );
    }

    println!("\nCaching:");
    if has("sccache", &["--version"]) {
        println!("  ✓ sccache found — in CI set RUSTC_WRAPPER=sccache to cache crate builds across runs.");
    } else {
        println!("  • `cargo install sccache`, then RUSTC_WRAPPER=sccache — the correct shared compile cache.");
    }

    println!("\nDev profile (Cargo.toml) — optimize deps once, keep your crates fast to build:");
    println!("  [profile.dev.package.\"*\"]");
    println!("  opt-level = 3");
    println!("  [profile.dev]");
    println!("  split-debuginfo = \"unpacked\"   # faster linking on Linux");

    println!("\nDo less work:");
    println!("  • `oryn build` / `oryn test` build & test only the crates your change affects.");
    println!("  • split large crates — a crate is the unit of caching AND of oryn's selection.");
    Ok(())
}

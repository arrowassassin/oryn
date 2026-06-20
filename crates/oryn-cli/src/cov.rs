//! Function-level (coverage-based) selection: `oryn cover` and `oryn test --fn`.
//!
//! `oryn cover` runs each test in isolation under `-C instrument-coverage`,
//! exports its executed lines via the toolchain's `llvm-cov`, and stores a
//! per-test coverage map keyed at the current commit. `oryn test --fn` then
//! diffs against that base, maps each changed hunk to its enclosing function
//! (sound under insertions), and runs only the tests whose coverage intersects
//! the impacted lines — falling back to a full crate run for any non-function
//! change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use oryn_core::fnselect::{self, TestCoverage};
use oryn_core::fnspans::{self, FileImpact};
use oryn_core::store::{self, Store};
use oryn_core::{coverage, difflines, git, metadata, select};

fn rustc_print(arg: &str) -> Result<String> {
    let out = Command::new("rustc").args(["--print", arg]).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn host() -> Result<String> {
    let out = Command::new("rustc").arg("-vV").output()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .context("could not read rustc host triple")
}

fn llvm_tool(name: &str) -> Result<PathBuf> {
    let p = PathBuf::from(rustc_print("sysroot")?)
        .join("lib/rustlib")
        .join(host()?)
        .join("bin")
        .join(name);
    if !p.exists() {
        bail!(
            "{name} not found at {} — run `rustup component add llvm-tools-preview`",
            p.display()
        );
    }
    Ok(p)
}

fn head_commit(dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Build (optionally instrumented) test binaries for `crate_name`, returning the
/// executable paths.
fn test_binaries(dir: &Path, crate_name: &str, instrument: bool) -> Result<Vec<PathBuf>> {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "test",
        "-p",
        crate_name,
        "--no-run",
        "--message-format=json",
    ])
    .current_dir(dir);
    if instrument {
        cmd.env("RUSTFLAGS", "-C instrument-coverage");
    }
    let out = cmd.output().context("cargo test --no-run")?;
    if !out.status.success() {
        bail!(
            "building test binaries for {crate_name} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut bins = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] == "compiler-artifact"
            && v["profile"]["test"] == true
            && v["executable"].is_string()
        {
            bins.push(PathBuf::from(v["executable"].as_str().unwrap()));
        }
    }
    Ok(bins)
}

/// List the test names a binary contains.
fn list_tests(bin: &Path) -> Result<Vec<String>> {
    let out = Command::new(bin)
        .args(["--list", "--format", "terse"])
        .output()
        .with_context(|| format!("listing tests in {}", bin.display()))?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_suffix(": test").map(str::to_string))
        .collect())
}

/// Make an absolute coverage path repo-relative, dropping non-repo files (std).
fn relativize(abs: &str, root: &Path) -> Option<String> {
    Path::new(abs)
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// `oryn cover`
pub fn cover(since: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = metadata::load(&cwd)?;
    let root = git::repo_root(&cwd)?;
    let changed = git::changed_files(&root, since)?;
    let plan = select::plan(&graph, &root, &changed);
    let crates = if plan.affected_crates.is_empty() {
        graph.names(&graph.all_indices())
    } else {
        plan.affected_crates.clone()
    };

    let profdata_tool = llvm_tool("llvm-profdata")?;
    let cov_tool = llvm_tool("llvm-cov")?;
    let tmp = root.join("target/oryn/cov");
    std::fs::create_dir_all(&tmp)?;

    let store_dir = Store::dir_for(&graph.root);
    let mut st = Store::load(&store_dir)?;

    let mut total = 0usize;
    for crate_name in &crates {
        eprintln!("oryn cover: instrumenting {crate_name}…");
        let bins = test_binaries(&cwd, crate_name, true)?;
        for bin in &bins {
            for test in list_tests(bin)? {
                let safe = test.replace([':', '/', ' '], "_");
                let profraw = tmp.join(format!("{crate_name}-{safe}.profraw"));
                let profdata = tmp.join(format!("{crate_name}-{safe}.profdata"));
                let _ = std::fs::remove_file(&profraw);

                let status = Command::new(bin)
                    .args(["--exact", &test])
                    .env("LLVM_PROFILE_FILE", &profraw)
                    .current_dir(&cwd)
                    .status()?;
                if !status.success() || !profraw.exists() {
                    continue; // failing/aborting test: leave it uncovered (safe)
                }
                if !Command::new(&profdata_tool)
                    .args(["merge", "-sparse"])
                    .arg(&profraw)
                    .arg("-o")
                    .arg(&profdata)
                    .status()?
                    .success()
                {
                    continue;
                }
                let export = Command::new(&cov_tool)
                    .arg("export")
                    .arg(format!("--instr-profile={}", profdata.display()))
                    .args(["--format=text"])
                    .arg(bin)
                    .output()?;
                if !export.status.success() {
                    continue;
                }
                let executed = coverage::parse_export(&export.stdout)?;
                let mut files: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
                for (abs, lines) in executed {
                    if let Some(rel) = relativize(&abs, &root) {
                        files.insert(rel, lines);
                    }
                }
                st.set_coverage(&format!("{crate_name}::{test}"), files);
                total += 1;
            }
        }
    }

    st.coverage_base = Some(head_commit(&cwd)?);
    st.save(&store_dir)?;
    println!(
        "oryn cover: recorded coverage for {total} test(s) at {}",
        st.coverage_base.as_deref().unwrap_or("HEAD")
    );
    Ok(())
}

/// `oryn test --fn`
pub fn test_fn(extra: &[String]) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = metadata::load(&cwd)?;
    let root = git::repo_root(&cwd)?;
    let store_dir = Store::dir_for(&graph.root);
    let mut st = Store::load(&store_dir)?;
    let Some(base) = st.coverage_base.clone() else {
        bail!("no coverage recorded — run `oryn cover` first");
    };

    // Changes since the coverage base.
    let hunks = difflines::changed_hunks(&root, &base)?;
    let changed_files: Vec<PathBuf> = hunks.keys().map(PathBuf::from).collect();
    let plan = select::plan(&graph, &root, &changed_files);
    if plan.select_all {
        eprintln!("oryn: {} — running all affected crates", plan.reason);
    }

    let now = store::now_unix();
    let mut overall_fail = false;
    let mut ran_any = false;

    for crate_name in &plan.affected_crates {
        let idx = graph.index_of(crate_name).unwrap();
        let crate_dir = &graph.members[idx].manifest_dir;
        let crate_rel = relativize(&crate_dir.to_string_lossy(), &root).unwrap_or_default();

        // Per-file impact for this crate's changed files.
        let mut impacts: BTreeMap<String, FileImpact> = BTreeMap::new();
        for (file, hs) in &hunks {
            if !file.starts_with(&crate_rel) {
                continue;
            }
            let old_src = git_show(&root, &base, file);
            let imp = match old_src {
                Some(src) => fnspans::impact(&src, hs),
                None => FileImpact::Whole, // new/removed file — be safe
            };
            impacts.insert(file.clone(), imp);
        }

        // Universe of this crate's tests (current).
        let bins = test_binaries(&cwd, crate_name, false)?;
        let mut names = Vec::new();
        for bin in &bins {
            names.extend(list_tests(bin)?);
        }
        let ids: Vec<String> = names.iter().map(|n| format!("{crate_name}::{n}")).collect();

        // Coverage subset for this crate, keyed by id.
        let coverage: TestCoverage = ids
            .iter()
            .filter_map(|id| st.coverage.get(id).map(|c| (id.clone(), c.clone())))
            .collect();

        let sel = fnselect::select(&impacts, &coverage, &ids);
        let to_run_names: Vec<String> = sel
            .run
            .iter()
            .filter_map(|id| {
                id.strip_prefix(&format!("{crate_name}::"))
                    .map(str::to_string)
            })
            .collect();

        if to_run_names.is_empty() {
            eprintln!(
                "oryn fn: {crate_name} — 0/{} tests impacted, skipped ✓",
                ids.len()
            );
            continue;
        }
        eprintln!(
            "oryn fn: {crate_name} — running {}/{} impacted test(s)",
            to_run_names.len(),
            ids.len()
        );
        ran_any = true;

        let mut cmd = Command::new("cargo");
        cmd.args(["test", "-p", crate_name, "--", "--exact"]);
        cmd.args(&to_run_names);
        cmd.args(extra);
        let status = cmd.current_dir(&cwd).status()?;
        if !status.success() {
            overall_fail = true;
        } else {
            // The tests we just ran are green at the current line state; refresh
            // history so flaky stats accumulate.
            for n in &to_run_names {
                st.observe_test(&format!("{crate_name}::{n}"), true, now, None);
            }
        }
    }

    st.save(&store_dir)?;
    if !ran_any {
        println!("oryn fn: no test impacted by the change ✓");
    } else if overall_fail {
        std::process::exit(1);
    } else {
        println!("oryn fn: all impacted tests passed ✓");
    }
    Ok(())
}

fn git_show(dir: &Path, base: &str, file: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("show")
        .arg(format!("{base}:{file}"))
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

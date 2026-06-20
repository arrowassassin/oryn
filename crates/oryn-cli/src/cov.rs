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
use oryn_core::hybrid::{self, HybridImpact};
use oryn_core::store::{self, Store};
use oryn_core::{coverage, difflines, git, metadata, select};

/// Is `s` a non-empty git object id (hex)? Guards values that flow into
/// `git show <base>:<file>` / `git diff <base>` against option/ref injection.
fn is_hex_oid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn rustc_print(arg: &str) -> Result<String> {
    let out = Command::new("rustc").args(["--print", arg]).output()?;
    if !out.status.success() {
        bail!("rustc --print {arg} failed");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn host() -> Result<String> {
    let out = Command::new("rustc").arg("-vV").output()?;
    if !out.status.success() {
        bail!("rustc -vV failed");
    }
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
    if !out.status.success() {
        bail!("git rev-parse HEAD failed (no commits yet?)");
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !is_hex_oid(&sha) {
        bail!("git rev-parse HEAD returned an unexpected value");
    }
    Ok(sha)
}

/// Is the working tree clean (no staged/unstaged/untracked changes)? `oryn cover`
/// must run on a clean tree, else coverage line numbers reflect the dirty tree
/// but are labelled with HEAD's commit — decohering later `--fn` diffs.
fn tree_is_clean(dir: &Path) -> Result<bool> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        bail!("git status failed");
    }
    Ok(out.stdout.iter().all(u8::is_ascii_whitespace))
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

/// Per-file executed line sets for one test.
type FileLines = BTreeMap<String, BTreeSet<usize>>;

/// One unit of coverage work: run `test` from `bin` in isolation and export it.
struct CovTask {
    /// Store key (`crate::test`).
    id: String,
    /// Instrumented test binary.
    bin: PathBuf,
    /// Exact test name.
    test: String,
    /// Unique index, for collision-free temp filenames under parallelism.
    slot: usize,
}

/// Run one test under coverage and return its executed lines (repo-relative).
/// Returns `None` (uncovered → conservatively re-run later) on any failure.
fn cover_one_test(
    task: &CovTask,
    tmp: &Path,
    profdata_tool: &Path,
    cov_tool: &Path,
    cwd: &Path,
    root: &Path,
) -> Option<FileLines> {
    let profraw = tmp.join(format!("cov-{}.profraw", task.slot));
    let profdata = tmp.join(format!("cov-{}.profdata", task.slot));
    let _ = std::fs::remove_file(&profraw);

    let status = Command::new(&task.bin)
        .args(["--exact", &task.test])
        .env("LLVM_PROFILE_FILE", &profraw)
        .current_dir(cwd)
        .status()
        .ok()?;
    if !status.success() || !profraw.exists() {
        return None; // failing/aborting test: leave it uncovered (safe)
    }
    if !Command::new(profdata_tool)
        .args(["merge", "-sparse"])
        .arg(&profraw)
        .arg("-o")
        .arg(&profdata)
        .status()
        .ok()?
        .success()
    {
        return None;
    }
    let export = Command::new(cov_tool)
        .arg("export")
        .arg(format!("--instr-profile={}", profdata.display()))
        .args(["--format=text"])
        .arg(&task.bin)
        .output()
        .ok()?;
    if !export.status.success() {
        return None;
    }
    let executed = coverage::parse_export(&export.stdout).ok()?;
    let mut files: FileLines = BTreeMap::new();
    for (abs, lines) in executed {
        if let Some(rel) = relativize(&abs, root) {
            files.insert(rel, lines);
        }
    }
    Some(files)
}

/// `oryn cover`
pub fn cover(since: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = metadata::load(&cwd)?;
    let root = git::repo_root(&cwd)?;
    if !tree_is_clean(&cwd)? {
        bail!(
            "working tree has uncommitted changes — commit or stash them first.\n\
             `oryn cover` records coverage against HEAD, so a dirty tree would mislabel \
             line numbers and make `oryn test --fn` selection unsound."
        );
    }
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

    // Gather every (test → its instrumented binary) task. Building the
    // instrumented binaries is serial (a cargo compile); the expensive part is
    // the per-test run + llvm-profdata + llvm-cov export, which is independent
    // per test and run in parallel below.
    let mut tasks: Vec<CovTask> = Vec::new();
    for crate_name in &crates {
        eprintln!("oryn cover: instrumenting {crate_name}…");
        let bins = test_binaries(&cwd, crate_name, true)?;
        for bin in &bins {
            for test in list_tests(bin)? {
                let slot = tasks.len();
                tasks.push(CovTask {
                    id: format!("{crate_name}::{test}"),
                    bin: bin.clone(),
                    test,
                    slot,
                });
            }
        }
    }

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(tasks.len().max(1));
    eprintln!(
        "oryn cover: collecting coverage for {} test(s) on {workers} worker(s)…",
        tasks.len()
    );

    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<(String, FileLines)>> = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(task) = tasks.get(i) else { break };
                if let Some(files) =
                    cover_one_test(task, &tmp, &profdata_tool, &cov_tool, &cwd, &root)
                {
                    results.lock().unwrap().push((task.id.clone(), files));
                }
            });
        }
    });

    let results = results.into_inner().unwrap();
    let total = results.len();
    for (id, files) in results {
        st.set_coverage(&id, files);
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
    if !is_hex_oid(&base) {
        bail!("recorded coverage base is not a valid commit id — re-run `oryn cover`");
    }

    // Changes since the coverage base.
    let hunks = difflines::changed_hunks(&root, &base)?;
    let changed_files: Vec<PathBuf> = hunks.keys().map(PathBuf::from).collect();
    let plan = select::plan(&graph, &root, &changed_files);
    if plan.select_all {
        eprintln!("oryn: {} — running all affected crates", plan.reason);
    }

    let now = store::now_unix();

    // Coverage is a *cross-crate* execution trace, so impact must be computed
    // GLOBALLY: a dependent crate's test that executed changed code in another
    // crate has to be selected too. Analyze every hunk once against the repo's
    // base sources (one reference graph spanning the whole workspace).
    let base_files = repo_base_files(&cwd, &base)?;
    let impact = hybrid::analyze(&base_files, &hunks);
    if matches!(impact, HybridImpact::WholeCrate) {
        eprintln!("oryn fn: non-localizable change — selecting every affected crate's tests");
    }

    let mut overall_fail = false;
    let mut ran_any = false;

    for crate_name in &plan.affected_crates {
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

        // Select this crate's tests against the GLOBAL impact: a test runs if its
        // (cross-crate) coverage intersects an impacted line, it has no coverage
        // record, or the change couldn't be localized.
        let run_ids: Vec<String> = match &impact {
            HybridImpact::WholeCrate => ids.clone(),
            HybridImpact::PerFile(impacts) => fnselect::select(impacts, &coverage, &ids).run,
        };

        // Always rerun flaky tests — coverage is one execution and can be unsound
        // under nondeterminism; the flaky subsystem flags exactly these.
        let mut run_set: std::collections::BTreeSet<String> = run_ids.into_iter().collect();
        for id in &ids {
            if st
                .tests
                .get(id)
                .is_some_and(|r| r.passes > 0 && r.fails > 0)
            {
                run_set.insert(id.clone());
            }
        }
        let to_run_names: Vec<String> = run_set
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

/// Every tracked `.rs` file in the repo at the base revision, as `(rel, source)`.
/// One reference graph over the whole workspace is what makes cross-crate impact
/// (a dependent crate referencing a changed `const`, or executing changed code)
/// sound — analyzing per-crate would miss those edges.
fn repo_base_files(dir: &Path, base: &str) -> Result<Vec<(String, String)>> {
    let out = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", base])
        .current_dir(dir)
        .output()?;
    let mut files = Vec::new();
    if out.status.success() {
        for path in String::from_utf8_lossy(&out.stdout).lines() {
            if path.ends_with(".rs") {
                if let Some(src) = git_show(dir, base, path) {
                    files.push((path.to_string(), src));
                }
            }
        }
    }
    Ok(files)
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

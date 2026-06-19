//! `oryn` — the reproducibility & evaluation-integrity command line.
//!
//! Subcommands:
//! * `scan`        — contamination-scan an eval set against a corpus
//! * `eval`        — produce a statistically-rigorous eval report
//! * `gate`        — paired regression gate between two runs
//! * `determinism` — analyze repeated generations for nondeterminism
//! * `keygen`      — generate an Ed25519 attestation identity
//! * `attest`      — verify a signed attestation chain
//! * `info`        — show versions and the active compute backend

mod io;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oryn_core::attest::{AttestationChain, Signer};
use oryn_core::contam::{CorpusIndex, ScanConfig};
use oryn_core::eval::{analyze, regression_gate, EvalConfig};

#[derive(Parser)]
#[command(
    name = "oryn",
    version,
    about = "Deterministic reproducibility & evaluation-integrity for AI — no model in the loop."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Contamination-scan an eval set against a reference corpus.
    Scan {
        /// Reference corpus ({"id","text"} JSONL or JSON array).
        #[arg(long)]
        corpus: PathBuf,
        /// Eval set to check ({"id","text"} JSONL or JSON array).
        #[arg(long)]
        eval: PathBuf,
        /// n-gram width in tokens.
        #[arg(long, default_value_t = 13)]
        ngram: usize,
        /// Flag an item when this fraction of its n-grams appears in the corpus.
        #[arg(long, default_value_t = 0.5)]
        ngram_threshold: f64,
        /// Flag an item at this estimated Jaccard near-duplicate similarity.
        #[arg(long, default_value_t = 0.8)]
        jaccard_threshold: f64,
        /// Write the JSON report here (stdout if omitted).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Produce a statistically-rigorous eval report (CI, power, required-N).
    Eval {
        /// Eval run ({"id","score"} JSONL or JSON array).
        #[arg(long)]
        run: PathBuf,
        /// Optional run name (defaults to file stem).
        #[arg(long)]
        name: Option<String>,
        /// Confidence level.
        #[arg(long, default_value_t = 0.95)]
        level: f64,
        /// Write the JSON report here (stdout if omitted).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Paired regression gate between a baseline and a candidate run.
    Gate {
        /// Baseline run.
        #[arg(long)]
        baseline: PathBuf,
        /// Candidate run.
        #[arg(long)]
        candidate: PathBuf,
        /// Confidence level.
        #[arg(long, default_value_t = 0.95)]
        level: f64,
        /// Write the JSON report here (stdout if omitted).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Analyze repeated generations for nondeterminism.
    Determinism {
        /// Repeated outputs (JSON array of strings, or one per line).
        #[arg(long)]
        runs: PathBuf,
        /// Write the JSON report here (stdout if omitted).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Generate an Ed25519 attestation identity (prints secret + public hex).
    Keygen,
    /// Verify a signed attestation chain.
    Attest {
        #[command(subcommand)]
        cmd: AttestCmd,
    },
    /// Run the HTTP API server (UI-agnostic JSON surface).
    Serve {
        /// Address to bind.
        #[arg(long, default_value = "127.0.0.1:8787")]
        addr: String,
    },
    /// Show versions and the active compute backend.
    Info,
}

#[derive(Subcommand)]
enum AttestCmd {
    /// Verify chain linkage, hashes, and signatures.
    Verify {
        /// Attestation chain JSON.
        #[arg(long)]
        chain: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan {
            corpus,
            eval,
            ngram,
            ngram_threshold,
            jaccard_threshold,
            out,
        } => run_scan(corpus, eval, ngram, ngram_threshold, jaccard_threshold, out),
        Command::Eval {
            run,
            name,
            level,
            out,
        } => run_eval(run, name, level, out),
        Command::Gate {
            baseline,
            candidate,
            level,
            out,
        } => run_gate(baseline, candidate, level, out),
        Command::Determinism { runs, out } => run_determinism(runs, out),
        Command::Keygen => run_keygen(),
        Command::Attest { cmd } => match cmd {
            AttestCmd::Verify { chain } => run_attest_verify(chain),
        },
        Command::Serve { addr } => run_serve(addr),
        Command::Info => run_info(),
    }
}

fn run_serve(addr: String) -> Result<()> {
    let addr: std::net::SocketAddr = addr.parse().context("parsing --addr")?;
    let rt = tokio::runtime::Runtime::new().context("starting tokio runtime")?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr}"))?;
        eprintln!(
            "oryn listening on http://{addr}  (backend: {})",
            oryn_cuda::backend()
        );
        oryn_server::serve(listener).await.context("server error")
    })
}

fn run_scan(
    corpus: PathBuf,
    eval: PathBuf,
    ngram: usize,
    ngram_threshold: f64,
    jaccard_threshold: f64,
    out: Option<PathBuf>,
) -> Result<()> {
    let corpus_docs = io::load_documents(&corpus)?;
    let eval_docs = io::load_documents(&eval)?;
    let cfg = ScanConfig {
        ngram_n: ngram,
        ngram_threshold,
        jaccard_threshold,
        ..Default::default()
    };
    let index = CorpusIndex::build(&corpus_docs, cfg);
    let report = index.scan(&eval_docs);
    eprintln!(
        "scan: {} items, {} contaminated ({:.1}%), {} clean held out",
        report.total_items,
        report.contaminated_items,
        report.contamination_rate * 100.0,
        report.clean_holdout.len()
    );
    io::write_json(&report, out.as_deref())
}

fn run_eval(run: PathBuf, name: Option<String>, level: f64, out: Option<PathBuf>) -> Result<()> {
    let run = io::load_run(&run, name.as_deref())?;
    let cfg = EvalConfig {
        level,
        ..Default::default()
    };
    let report = analyze(&run, &cfg).context("analyzing run")?;
    eprintln!(
        "eval '{}': mean={:.4} ±{:.4} (n={}, {} CI [{:.4}, {:.4}]); required-N for d={:.2}: {}",
        report.name,
        report.mean,
        report.ci.margin(),
        report.n,
        level,
        report.ci.low,
        report.ci.high,
        report.power.effect_size,
        report.power.required_n
    );
    io::write_json(&report, out.as_deref())
}

fn run_gate(baseline: PathBuf, candidate: PathBuf, level: f64, out: Option<PathBuf>) -> Result<()> {
    let base = io::load_run(&baseline, None)?;
    let cand = io::load_run(&candidate, None)?;
    let gate = regression_gate(&base, &cand, level).context("running regression gate")?;
    eprintln!(
        "gate: {:?} (Δ={:+.4}, p={:.4}, n={}){}",
        gate.verdict,
        gate.comparison.mean_diff,
        gate.comparison.p_value,
        gate.paired_n,
        if gate.blocked { " — BLOCKED" } else { "" }
    );
    io::write_json(&gate, out.as_deref())?;
    if gate.blocked {
        std::process::exit(2);
    }
    Ok(())
}

fn run_determinism(runs: PathBuf, out: Option<PathBuf>) -> Result<()> {
    let outputs = io::load_strings(&runs)?;
    let report = oryn_core::determinism::analyze_outputs(&outputs);
    eprintln!(
        "determinism: {}/{} unique outputs ({}); {}",
        report.unique_outputs,
        report.total_runs,
        if report.deterministic {
            "DETERMINISTIC"
        } else {
            "NONDETERMINISTIC"
        },
        match report.divergence_token {
            Some(t) => format!("first divergence at token {t}"),
            None => "no divergence".to_string(),
        }
    );
    io::write_json(&report, out.as_deref())
}

fn run_keygen() -> Result<()> {
    let signer = Signer::generate();
    println!("{{");
    println!("  \"secret_hex\": \"{}\",", signer.secret_hex());
    println!("  \"public_hex\": \"{}\"", signer.public_hex());
    println!("}}");
    eprintln!("keep secret_hex private; pass it to attestation signing");
    Ok(())
}

fn run_attest_verify(chain: PathBuf) -> Result<()> {
    let raw =
        std::fs::read_to_string(&chain).with_context(|| format!("reading {}", chain.display()))?;
    let chain: AttestationChain = serde_json::from_str(&raw).context("parsing chain")?;
    match chain.verify() {
        Ok(()) => {
            eprintln!(
                "attestation OK: {} entries, chain intact",
                chain.entries.len()
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("attestation FAILED: {e}");
            std::process::exit(1);
        }
    }
}

fn run_info() -> Result<()> {
    println!("oryn-core   {}", oryn_core::VERSION);
    println!("compute     {}", oryn_cuda::backend());
    println!("cuda linked {}", oryn_cuda::cuda_available());
    Ok(())
}

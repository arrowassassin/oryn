//! Human-readable output.

use oryn_core::flaky::{FlakyReport, FlakyVerdict};
use oryn_core::select::SelectionPlan;

/// Print a selection plan.
pub fn plan(p: &SelectionPlan) {
    println!("{}", p.reason);
    if p.ignored_files > 0 {
        println!(
            "  {} changed file(s) belong to no crate (docs/CI) — ignored",
            p.ignored_files
        );
    }
    if p.affected_crates.is_empty() {
        println!("  → nothing to test");
    } else {
        println!(
            "  affected ({}): {}",
            p.affected_crates.len(),
            p.affected_crates.join(", ")
        );
        if !p.skipped_crates.is_empty() {
            println!(
                "  skipped  ({}): {}",
                p.skipped_crates.len(),
                p.skipped_crates.join(", ")
            );
        }
    }
}

/// Print a flaky report.
pub fn flaky(r: &FlakyReport) {
    println!(
        "{} test(s) with history: {} flaky, {} always-failing",
        r.tests.len(),
        r.flaky_count,
        r.always_fail_count
    );
    // Show flaky first (most actionable), then always-fail.
    let mut shown = 0;
    for t in &r.tests {
        if t.verdict == FlakyVerdict::Flaky {
            println!(
                "  FLAKY  {}  rate={:.1}% (Wilson {:.1}–{:.1}%, Bayes {:.1}–{:.1}%), ~{} reruns to reproduce",
                t.id,
                t.flake_rate * 100.0,
                t.ci.low * 100.0,
                t.ci.high * 100.0,
                t.posterior.low * 100.0,
                t.posterior.high * 100.0,
                t.reruns_to_reproduce_95.unwrap_or(0),
            );
            shown += 1;
        }
    }
    for t in &r.tests {
        if t.verdict == FlakyVerdict::StableFail {
            println!("  FAIL   {}  ({} runs, all failed)", t.id, t.runs);
            shown += 1;
        }
    }
    if shown == 0 {
        println!("  no flaky or failing tests in history ✓");
    }
}

//! # Oryn core — compile less, test less, trust the results
//!
//! A safe, Rust-native engine for two things every Cargo project needs and no
//! free tool provides together:
//!
//! * [`select`] — **safe test-impact selection**. From a git diff, work out which
//!   workspace crates can possibly be affected (the changed crates plus every
//!   crate that transitively depends on them) and skip the rest. Safe by
//!   construction at crate granularity (RustyRTS, ICST 2025).
//! * [`flaky`] — **statistically-rigorous flaky-test scoring**: a flake-rate
//!   estimate with a Wilson confidence interval and the rerun budget the
//!   statistics actually require — instead of the naive "fail-then-pass in 3
//!   reruns" rule every other runner uses.
//!
//! Supporting modules: [`graph`] (the workspace dependency graph), [`metadata`]
//! (loading it from `cargo metadata`), [`git`] (change detection), and [`stats`]
//! (the deterministic numerics — Wilson intervals, etc.).
//!
//! No model is in the loop; everything here is classical, deterministic CS.

#![forbid(unsafe_code)]

pub mod bayes;
pub mod dashboard;
pub mod error;
pub mod fingerprint;
pub mod flaky;
pub mod git;
pub mod graph;
pub mod junit;
pub mod metadata;
pub mod prioritize;
pub mod runner;
pub mod select;
pub mod stats;
pub mod store;

pub use error::{OrynError, Result};

/// Crate version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Convenience re-exports.
pub mod prelude {
    pub use crate::flaky::{
        analyze as analyze_flaky, required_reruns, FlakeScore, FlakyReport, FlakyVerdict, TestRuns,
    };
    pub use crate::graph::{Member, WorkspaceGraph};
    pub use crate::select::{plan as plan_selection, SelectionPlan};
    pub use crate::{OrynError, Result};
}

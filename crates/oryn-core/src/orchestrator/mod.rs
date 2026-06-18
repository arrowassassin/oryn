//! Orchestration engine — the "route, don't race" pipeline.
//!
//! # Design philosophy
//!
//! Oryn controls AI coding agents by decomposing a high-level [`task::Mission`]
//! into typed [`task::Subtask`]s with explicit dependency edges, then
//! scheduling them in deterministic topological order. The rule is
//! **route, don't race**: work items are dispatched to the best-fit agent slot
//! for each [`task::SubtaskKind`], never naively raced in parallel with the
//! hope that one wins. This gives reproducible behaviour, meaningful cost
//! attribution, and sensible retry semantics.
//!
//! Later modules in this crate extend the pipeline with scheduling, agent
//! selection, budget enforcement, and result aggregation.

pub mod capability;
pub mod catalog;
pub mod cost;
pub mod discovery;
pub mod prefix;
pub mod provider;
pub mod scheduler;
pub mod task;

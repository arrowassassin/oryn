//! Oryn core — the headless, UI-agnostic engine.
//!
//! This crate owns everything real about a session: the normalized event
//! model, the content-addressed store, agent adapters, budgets, worktrees,
//! and (in later segments) the orchestration engine. No UI types live here;
//! clients (GPUI app, future TUI) consume this crate's API.

pub mod adapter;
pub mod budget;
pub mod event;
pub mod ids;
pub mod orchestrator;
pub mod session;
pub mod store;
pub mod worktree;

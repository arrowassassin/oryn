//! Mission Control view — the live "race" board, fully interactive.
//!
//! Pure data model ([`AgentRun`]) plus the render methods on [`crate::Root`]:
//! a race strip (one clickable row per execution target) above a grid of agent
//! cards (current activity, metrics, token/budget gauges, actions). The board is
//! driven by the live simulation tick and the Play/Pause control; card actions
//! open the agent's timeline, jump to Review, or promote a winner.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px, relative};

use crate::Root;
use crate::Screen;
use crate::colors::{overlay, solid, tint};
use crate::state::Msg;
use crate::theme::{Rgb, Theme};

/// Hard token cap per agent.
pub const TOKEN_CAP: u32 = 300_000;
/// Hard USD budget cap per agent.
pub const COST_CAP: f64 = 4.00;

/// Lifecycle state of one agent's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Finished,
    Stopped,
}

/// One execution target racing on the mission, with its live telemetry.
#[derive(Debug, Clone)]
pub struct AgentRun {
    pub id: &'static str,
    pub cli: &'static str,
    pub model: &'static str,
    pub color: Rgb,
    pub leading: bool,
    pub tests: &'static str,
    pub test_ok: bool,
    pub status: RunStatus,
    pub elapsed_sec: u32,
    pub turns: u32,
    pub files: u32,
    pub add: u32,
    pub del: u32,
    pub tokens: u32,
    pub cost: f64,
    pub race: f32,
    pub cur_tool: &'static str,
    pub cur_text: &'static str,
}

impl AgentRun {
    /// The four demo runs from the design handoff (a race already in flight).
    pub fn sample() -> Vec<AgentRun> {
        vec![
            AgentRun { id: "claude", cli: "claude", model: "opus-4.6", color: 0xC08CFF, leading: true, tests: "14/14", test_ok: true, status: RunStatus::Running, elapsed_sec: 461, turns: 41, files: 9, add: 486, del: 213, tokens: 184_200, cost: 2.41, race: 0.82, cur_tool: "Edit", cur_text: "src/auth/refreshQueue.ts" },
            AgentRun { id: "codex", cli: "codex", model: "gpt-5.2", color: 0x4ED99A, leading: false, tests: "11/14", test_ok: false, status: RunStatus::Running, elapsed_sec: 459, turns: 33, files: 6, add: 312, del: 147, tokens: 251_000, cost: 3.31, race: 0.61, cur_tool: "Bash", cur_text: "pnpm vitest run auth/refresh" },
            AgentRun { id: "gemini", cli: "gemini", model: "2.5-pro", color: 0x7FA8FF, leading: false, tests: "14/14", test_ok: true, status: RunStatus::Finished, elapsed_sec: 302, turns: 22, files: 5, add: 201, del: 96, tokens: 96_400, cost: 0.71, race: 1.0, cur_tool: "done", cur_text: "completed · all tests pass" },
            AgentRun { id: "amp", cli: "amp", model: "sonnet-4.6", color: 0xFFB454, leading: false, tests: "9/14", test_ok: false, status: RunStatus::Stopped, elapsed_sec: 598, turns: 58, files: 11, add: 734, del: 402, tokens: 300_000, cost: 3.97, race: 0.55, cur_tool: "killed", cur_text: "token budget exceeded" },
        ]
    }

    /// A fresh run seeded from a selected adapter, at the start line.
    pub fn launching(ad: &crate::launcher::Adapter) -> AgentRun {
        AgentRun {
            id: ad.cli,
            cli: ad.cli,
            model: ad.tag,
            color: ad.color,
            leading: false,
            tests: "0/14",
            test_ok: false,
            status: RunStatus::Running,
            elapsed_sec: 0,
            turns: 0,
            files: 0,
            add: 0,
            del: 0,
            tokens: 0,
            cost: 0.0,
            race: 0.0,
            cur_tool: "init",
            cur_text: "starting worktree…",
        }
    }

    pub(crate) fn status_color(&self, theme: &Theme) -> Rgb {
        match self.status {
            RunStatus::Running => theme.status.green,
            RunStatus::Finished => theme.status.blue,
            RunStatus::Stopped => theme.status.red,
        }
    }

    pub(crate) fn status_label(&self) -> &'static str {
        match self.status {
            RunStatus::Running => "Running",
            RunStatus::Finished => "Finished",
            RunStatus::Stopped => "Stopped",
        }
    }

    pub(crate) fn fill_color(&self, theme: &Theme) -> Rgb {
        if self.status == RunStatus::Stopped {
            theme.status.red
        } else {
            self.color
        }
    }

    pub(crate) fn token_fraction(&self) -> f32 {
        (self.tokens as f32 / TOKEN_CAP as f32).min(1.0)
    }

    pub(crate) fn cost_fraction(&self) -> f32 {
        (self.cost as f32 / COST_CAP as f32).min(1.0)
    }
}

// ── formatting helpers ──────────────────────────────────────────────────────

pub(crate) fn fmt_k(n: u32) -> String {
    if n >= 100_000 {
        format!("{}k", (n as f32 / 1000.0).round() as u32)
    } else {
        let s = format!("{:.1}", n as f32 / 1000.0);
        format!("{}k", s.trim_end_matches(".0"))
    }
}

pub(crate) fn fmt_usd(n: f64) -> String {
    format!("${n:.2}")
}

pub(crate) fn fmt_elapsed(sec: u32) -> String {
    format!("{}m {:02}s", sec / 60, sec % 60)
}

// ── shared little elements ────────────────────────────────────────────────────

pub(crate) fn dot(size: gpui::Pixels, color: Rgb) -> gpui::Div {
    div().size(size).rounded_full().bg(solid(color))
}

pub(crate) fn pill_chip(t: &Theme, label: &'static str) -> gpui::Div {
    div()
        .px(px(5.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(tint(t.accent.base, 0.13))
        .border_1()
        .border_color(tint(t.accent.base, 0.3))
        .text_size(px(8.5))
        .font_weight(FontWeight::BOLD)
        .text_color(solid(t.accent.base))
        .child(label)
}

pub(crate) fn status_pill(t: &Theme, a: &AgentRun) -> gpui::Div {
    let c = a.status_color(t);
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(9.0))
        .py(px(3.0))
        .rounded(px(6.0))
        .bg(tint(c, 0.13))
        .border_1()
        .border_color(tint(c, 0.25))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(solid(c))
        .child(dot(px(6.0), c))
        .child(a.status_label())
}

// ── view (methods on Root) ────────────────────────────────────────────────────

impl Root {
    /// Render the full Mission Control view.
    pub(crate) fn mission_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(self.mission_header(cx, &t))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .px(px(24.0))
                    .pt(px(18.0))
                    .pb(px(28.0))
                    .gap(px(18.0))
                    .child(self.race_strip(cx, &t))
                    .child(self.card_grid(cx, &t)),
            )
            .into_any_element()
    }

    fn mission_header(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let play_label = if self.playing { "Pause" } else { "Resume" };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .px(px(24.0))
            .pt(px(18.0))
            .pb(px(14.0))
            .border_b_1()
            .border_color(overlay(t.overlays.w06))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(div().text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child("MISSION CONTROL"))
                            .child(div().text_size(px(21.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)).child("Fix flaky token-refresh race")),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("play")
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .h(px(32.0))
                            .px(px(13.0))
                            .rounded(px(8.0))
                            .bg(overlay(t.overlays.w04))
                            .border_1()
                            .border_color(overlay(t.overlays.w09))
                            .text_size(px(12.0))
                            .text_color(solid(t.text.t2))
                            .cursor_pointer()
                            .on_click(self.on(cx, Msg::TogglePlay))
                            .child(div().size(px(8.0)).rounded(px(2.0)).bg(solid(t.text.t3)))
                            .child(play_label),
                    )
                    .child(
                        div()
                            .id("compare")
                            .flex()
                            .items_center()
                            .h(px(32.0))
                            .px(px(13.0))
                            .rounded(px(8.0))
                            .bg(overlay(t.overlays.w04))
                            .border_1()
                            .border_color(overlay(t.overlays.w09))
                            .text_size(px(12.0))
                            .text_color(solid(t.text.t2))
                            .cursor_pointer()
                            .on_click(self.on(cx, Msg::Navigate(Screen::Review)))
                            .child("Compare diffs"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .text_size(px(11.5))
                    .child(div().text_color(solid(t.text.t5)).child("repo"))
                    .child(div().text_color(solid(t.text.t2)).child("acme/web-platform"))
                    .child(div().text_color(solid(t.surfaces.dot_faint)).child("·"))
                    .child(div().text_color(solid(t.text.t5)).child("base"))
                    .child(div().text_color(solid(t.text.t2)).child("main@4f2ab1c"))
                    .child(div().text_color(solid(t.surfaces.dot_faint)).child("·"))
                    .child(dot(px(6.0), t.status.green))
                    .child(div().text_color(solid(t.text.t2)).child(self.status_summary())),
            )
    }

    fn race_strip(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, a) in self.agents.iter().enumerate() {
            rows.push(self.race_row(cx, t, i, a).into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(overlay(t.overlays.w07))
            .rounded(px(13.0))
            .px(px(18.0))
            .pt(px(16.0))
            .pb(px(18.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .mb(px(15.0))
                    .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t3)).child("THE RACE"))
                    .child(dot(px(6.0), t.accent.base))
                    .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child(if self.playing { "live · isolated worktrees" } else { "paused" }))
                    .child(div().flex_1())
                    .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child("progress · cost")),
            )
            .children(rows)
    }

    fn race_row(&self, cx: &mut Context<Self>, t: &Theme, idx: usize, a: &AgentRun) -> impl IntoElement {
        div()
            .id(("race", idx))
            .flex()
            .items_center()
            .gap(px(14.0))
            .py(px(9.0))
            .border_t_1()
            .border_color(overlay(t.overlays.w04))
            .cursor_pointer()
            .on_click(self.on(cx, Msg::OpenTimeline(idx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .w(px(190.0))
                    .flex_none()
                    .child(dot(px(9.0), a.color))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(12.5)).text_color(solid(t.text.t1)).child(a.cli))
                    .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child(a.model))
                    .when(a.leading, |d| d.child(pill_chip(t, "LEAD"))),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(7.0))
                    .rounded(px(4.0))
                    .bg(overlay(t.overlays.w05))
                    .overflow_hidden()
                    .child(div().h_full().w(relative(a.race)).rounded(px(4.0)).bg(solid(a.fill_color(t)))),
            )
            .child(div().w(px(44.0)).flex_none().text_size(px(11.5)).text_color(solid(t.text.t2)).child(fmt_usd(a.cost)))
            .child(status_pill(t, a))
    }

    fn card_grid(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        // Two-column grid emulated with rows of two flex_1 cards. Built with
        // explicit loops so the `&mut Context` isn't captured by a `map` closure.
        let mut rows: Vec<AnyElement> = Vec::new();
        for (row, pair) in self.agents.chunks(2).enumerate() {
            let mut cards: Vec<AnyElement> = Vec::new();
            for (col, a) in pair.iter().enumerate() {
                cards.push(self.agent_card(cx, t, row * 2 + col, a));
            }
            rows.push(div().flex().gap(px(14.0)).children(cards).into_any_element());
        }
        div().flex().flex_col().gap(px(14.0)).children(rows)
    }

    fn agent_card(&self, cx: &mut Context<Self>, t: &Theme, idx: usize, a: &AgentRun) -> AnyElement {
        let border = if a.leading { tint(t.accent.base, 0.4) } else { overlay(t.overlays.w07) };
        div()
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(border)
            .rounded(px(13.0))
            .px(px(18.0))
            .py(px(16.0))
            .overflow_hidden()
            .child(div().absolute().left(px(0.0)).top(px(0.0)).bottom(px(0.0)).w(px(3.0)).bg(solid(a.color)))
            .child(card_head(t, a))
            .child(current_activity(t, a))
            .child(metrics_row(t, a))
            .child(gauges(t, a))
            .child(self.card_footer(cx, t, idx, a))
            .into_any_element()
    }

    fn card_footer(&self, cx: &mut Context<Self>, t: &Theme, idx: usize, a: &AgentRun) -> impl IntoElement {
        let btn = |id: &'static str, label: &'static str, handler| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(28.0))
                .px(px(11.0))
                .rounded(px(7.0))
                .bg(overlay(t.overlays.w04))
                .border_1()
                .border_color(overlay(t.overlays.w09))
                .text_size(px(11.5))
                .text_color(solid(t.text.t2))
                .cursor_pointer()
                .on_click(handler)
                .child(label)
        };
        let promote = div()
            .id(("promote", idx))
            .flex()
            .items_center()
            .h(px(28.0))
            .px(px(11.0))
            .rounded(px(7.0))
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .on_click(self.on(cx, Msg::Promote(idx)))
            .map(|d| {
                if a.test_ok {
                    d.bg(solid(t.accent.base)).text_color(solid(0x1A0F2E)).child("Promote")
                } else {
                    d.bg(overlay(t.overlays.w03)).border_1().border_color(overlay(t.overlays.w07)).text_color(solid(t.text.t4)).child("Promote")
                }
            });
        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .pt(px(13.0))
            .border_t_1()
            .border_color(overlay(t.overlays.w06))
            .child(div().text_size(px(11.5)).text_color(solid(t.status.green)).child(format!("+{}", a.add)))
            .child(div().text_size(px(11.5)).text_color(solid(t.status.red)).child(format!("−{}", a.del)))
            .child(div().flex_1())
            .child(btn("tl", "Timeline", self.on(cx, Msg::OpenTimeline(idx))))
            .child(btn("df", "Diff", self.on(cx, Msg::Navigate(Screen::Review))))
            .child(promote)
    }
}

// ── static card sub-parts (theme only) ─────────────────────────────────────────

fn card_head(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .mb(px(14.0))
        .child(dot(px(9.0), a.color))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(13.5)).text_color(solid(t.text.t1)).child(a.cli))
                        .when(a.leading, |d| d.child(pill_chip(t, "LEADING"))),
                )
                .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child(format!("{} · oryn-{}", a.model, a.id))),
        )
        .child(div().flex_1())
        .child(status_pill(t, a))
}

fn current_activity(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let live = a.status == RunStatus::Running;
    let cur_dot = if live { t.status.green } else { t.text.t6 };
    div()
        .flex()
        .items_center()
        .gap(px(9.0))
        .mb(px(14.0))
        .px(px(11.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(overlay(t.overlays.w025))
        .border_1()
        .border_color(overlay(t.overlays.w05))
        .child(dot(px(7.0), cur_dot))
        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(11.0)).text_color(solid(t.text.t2)).child(a.cur_tool))
        .child(div().flex_1().overflow_hidden().text_size(px(11.0)).text_color(solid(t.text.t4)).child(a.cur_text))
}

fn metrics_row(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let tests_color = if a.test_ok { t.status.green } else { t.status.amber };
    div()
        .flex()
        .gap(px(2.0))
        .mb(px(15.0))
        .child(metric_tile(t, "ELAPSED", fmt_elapsed(a.elapsed_sec), t.text.t1))
        .child(metric_tile(t, "TURNS", a.turns.to_string(), t.text.t1))
        .child(metric_tile(t, "FILES", a.files.to_string(), t.text.t1))
        .child(metric_tile(t, "TESTS", a.tests.to_string(), tests_color))
}

fn metric_tile(t: &Theme, label: &'static str, value: String, value_color: Rgb) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(div().text_size(px(9.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child(label))
        .child(div().text_size(px(13.0)).text_color(solid(value_color)).child(value))
}

fn gauges(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(11.0))
        .mb(px(15.0))
        .child(gauge(t, "Tokens", format!("{} / {}", fmt_k(a.tokens), fmt_k(TOKEN_CAP)), a.token_fraction(), a.fill_color(t)))
        .child(gauge(t, "Budget", format!("{} / {}", fmt_usd(a.cost), fmt_usd(COST_CAP)), a.cost_fraction(), a.fill_color(t)))
}

fn gauge(t: &Theme, label: &'static str, value: String, fraction: f32, fill: Rgb) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .flex()
                .justify_between()
                .child(div().text_size(px(10.0)).text_color(solid(t.text.t4)).child(label))
                .child(div().text_size(px(10.5)).text_color(solid(t.text.t3)).child(value)),
        )
        .child(
            div()
                .h(px(5.0))
                .rounded(px(3.0))
                .bg(overlay(t.overlays.w06))
                .overflow_hidden()
                .child(div().h_full().w(relative(fraction)).rounded(px(3.0)).bg(solid(fill))),
        )
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_k_compacts_thousands() {
        assert_eq!(fmt_k(184_200), "184k");
        assert_eq!(fmt_k(96_400), "96.4k");
        assert_eq!(fmt_k(300_000), "300k");
        assert_eq!(fmt_k(2_000), "2k");
    }

    #[test]
    fn fmt_usd_two_places() {
        assert_eq!(fmt_usd(2.4), "$2.40");
        assert_eq!(fmt_usd(0.71), "$0.71");
    }

    #[test]
    fn fmt_elapsed_minutes_seconds() {
        assert_eq!(fmt_elapsed(461), "7m 41s");
        assert_eq!(fmt_elapsed(302), "5m 02s");
        assert_eq!(fmt_elapsed(59), "0m 59s");
    }

    #[test]
    fn sample_has_four_distinct_runs_one_leader() {
        let runs = AgentRun::sample();
        assert_eq!(runs.len(), 4);
        let mut ids: Vec<&str> = runs.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        assert_eq!(runs.iter().filter(|r| r.leading).count(), 1);
    }

    #[test]
    fn launching_starts_at_zero_running() {
        let ad = crate::launcher::Adapter { name: "Claude Code", cli: "claude", color: 0xC08CFF, enabled: true, tag: "opus-4.6" };
        let a = AgentRun::launching(&ad);
        assert_eq!(a.status, RunStatus::Running);
        assert_eq!(a.tokens, 0);
        assert_eq!(a.race, 0.0);
        assert_eq!(a.model, "opus-4.6");
    }

    #[test]
    fn fractions_clamped_to_unit() {
        for r in AgentRun::sample() {
            assert!((0.0..=1.0).contains(&r.token_fraction()));
            assert!((0.0..=1.0).contains(&r.cost_fraction()));
        }
    }
}

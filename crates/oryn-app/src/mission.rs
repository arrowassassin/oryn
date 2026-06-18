//! Mission Control view — the live "race" board.
//!
//! Renders the running execution targets for a mission: a compact race strip
//! (one row per agent, progress + cost) above a grid of detailed agent cards
//! (current activity, metrics, token/budget gauges, diff stats, actions). Mirrors
//! the Mission Control screen in the design handoff (`Oryn.dc.html`).
//!
//! The data model ([`AgentRun`]) is pure and rendering-agnostic; [`mission_control`]
//! turns a slice of runs plus a [`Theme`] into a GPUI element tree.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, FontWeight, IntoElement, ParentElement, Styled, div, px, relative,
};

use crate::colors::{overlay, solid, tint};
use crate::theme::{Rgb, Theme};

/// Hard token cap per agent (matches the design's `TOKEN_CAP`).
pub const TOKEN_CAP: u32 = 300_000;
/// Hard USD budget cap per agent (matches the design's `COST_CAP`).
pub const COST_CAP: f64 = 4.00;

/// Lifecycle state of one agent's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Actively producing events.
    Running,
    /// Ended naturally.
    Finished,
    /// Hard-stopped (e.g. budget exceeded).
    Stopped,
}

/// One execution target racing on the mission, with its live telemetry.
#[derive(Debug, Clone)]
pub struct AgentRun {
    /// Stable id used for the worktree name (`oryn-<id>`).
    pub id: &'static str,
    /// CLI / framework display name.
    pub cli: &'static str,
    /// Model identifier.
    pub model: &'static str,
    /// Brand hue (0xRRGGBB) for dots, bars, and fills.
    pub color: Rgb,
    /// Whether this run currently leads the race.
    pub leading: bool,
    /// Tests passing, e.g. `"14/14"`.
    pub tests: &'static str,
    /// Whether all tests pass (drives the tests color).
    pub test_ok: bool,
    /// Lifecycle status.
    pub status: RunStatus,
    /// Wall-clock elapsed seconds.
    pub elapsed_sec: u32,
    /// Agent turns taken.
    pub turns: u32,
    /// Files touched.
    pub files: u32,
    /// Lines added.
    pub add: u32,
    /// Lines removed.
    pub del: u32,
    /// Tokens consumed.
    pub tokens: u32,
    /// USD spent.
    pub cost: f64,
    /// Race progress in `0.0..=1.0`.
    pub race: f32,
    /// Current tool name.
    pub cur_tool: &'static str,
    /// Current tool detail (file, command, …).
    pub cur_text: &'static str,
}

impl AgentRun {
    /// The four demo runs from the design handoff, for the shell preview.
    pub fn sample() -> Vec<AgentRun> {
        vec![
            AgentRun {
                id: "claude",
                cli: "claude",
                model: "opus-4.6",
                color: 0xC08CFF,
                leading: true,
                tests: "14/14",
                test_ok: true,
                status: RunStatus::Running,
                elapsed_sec: 461,
                turns: 41,
                files: 9,
                add: 486,
                del: 213,
                tokens: 184_200,
                cost: 2.41,
                race: 0.82,
                cur_tool: "Edit",
                cur_text: "src/auth/refreshQueue.ts",
            },
            AgentRun {
                id: "codex",
                cli: "codex",
                model: "gpt-5.2",
                color: 0x4ED99A,
                leading: false,
                tests: "11/14",
                test_ok: false,
                status: RunStatus::Running,
                elapsed_sec: 459,
                turns: 33,
                files: 6,
                add: 312,
                del: 147,
                tokens: 251_000,
                cost: 3.31,
                race: 0.61,
                cur_tool: "Bash",
                cur_text: "pnpm vitest run auth/refresh",
            },
            AgentRun {
                id: "gemini",
                cli: "gemini",
                model: "2.5-pro",
                color: 0x7FA8FF,
                leading: false,
                tests: "14/14",
                test_ok: true,
                status: RunStatus::Finished,
                elapsed_sec: 302,
                turns: 22,
                files: 5,
                add: 201,
                del: 96,
                tokens: 96_400,
                cost: 0.71,
                race: 1.0,
                cur_tool: "done",
                cur_text: "completed · all tests pass",
            },
            AgentRun {
                id: "amp",
                cli: "amp",
                model: "sonnet-4.6",
                color: 0xFFB454,
                leading: false,
                tests: "9/14",
                test_ok: false,
                status: RunStatus::Stopped,
                elapsed_sec: 598,
                turns: 58,
                files: 11,
                add: 734,
                del: 402,
                tokens: 300_000,
                cost: 3.97,
                race: 0.55,
                cur_tool: "killed",
                cur_text: "token budget exceeded",
            },
        ]
    }

    /// The status hue under `theme`.
    fn status_color(&self, theme: &Theme) -> Rgb {
        match self.status {
            RunStatus::Running => theme.status.green,
            RunStatus::Finished => theme.status.blue,
            RunStatus::Stopped => theme.status.red,
        }
    }

    fn status_label(&self) -> &'static str {
        match self.status {
            RunStatus::Running => "Running",
            RunStatus::Finished => "Finished",
            RunStatus::Stopped => "Stopped",
        }
    }

    /// The bar/fill hue: red when stopped (over-budget), else the brand color.
    fn fill_color(&self, theme: &Theme) -> Rgb {
        if self.status == RunStatus::Stopped {
            theme.status.red
        } else {
            self.color
        }
    }

    fn token_fraction(&self) -> f32 {
        (self.tokens as f32 / TOKEN_CAP as f32).min(1.0)
    }

    fn cost_fraction(&self) -> f32 {
        (self.cost as f32 / COST_CAP as f32).min(1.0)
    }
}

// ── formatting helpers ──────────────────────────────────────────────────────

fn fmt_k(n: u32) -> String {
    if n >= 100_000 {
        format!("{}k", (n as f32 / 1000.0).round() as u32)
    } else {
        let v = n as f32 / 1000.0;
        let s = format!("{v:.1}");
        format!("{}k", s.trim_end_matches(".0"))
    }
}

fn fmt_usd(n: f64) -> String {
    format!("${n:.2}")
}

fn fmt_elapsed(sec: u32) -> String {
    format!("{}m {:02}s", sec / 60, sec % 60)
}

// ── view ────────────────────────────────────────────────────────────────────

/// Render the full Mission Control view (header + race strip + agent cards).
pub fn mission_control(t: &Theme, agents: &[AgentRun]) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .min_h(px(0.0))
        .child(header(t))
        .child(
            // scrollable body
            div()
                .flex_1()
                .overflow_hidden()
                .flex()
                .flex_col()
                .px(px(24.0))
                .pt(px(18.0))
                .pb(px(28.0))
                .gap(px(18.0))
                .child(race_strip(t, agents))
                .child(card_grid(t, agents)),
        )
}

fn header(t: &Theme) -> impl IntoElement {
    let pill = |label: &'static str| {
        div()
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
            .child(label)
    };
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
                        .child(
                            div()
                                .text_size(px(9.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(solid(t.text.t5))
                                .child("MISSION CONTROL"),
                        )
                        .child(
                            div()
                                .text_size(px(21.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(solid(t.text.t1))
                                .child("Fix flaky token-refresh race"),
                        ),
                )
                .child(div().flex_1())
                .child(pill("Pause"))
                .child(pill("Compare diffs")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_size(px(11.5))
                .child(meta_label(t, "repo"))
                .child(meta_value(t, "acme/web-platform"))
                .child(meta_sep(t))
                .child(meta_label(t, "base"))
                .child(meta_value(t, "main@4f2ab1c"))
                .child(meta_sep(t))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(dot(px(6.0), t.status.green))
                        .child(meta_value(t, "2 running · 1 finished · 1 stopped")),
                ),
        )
}

fn meta_label(t: &Theme, s: &'static str) -> impl IntoElement {
    div().text_size(px(11.5)).text_color(solid(t.text.t5)).child(s)
}

fn meta_value(t: &Theme, s: &'static str) -> impl IntoElement {
    div().text_size(px(11.5)).text_color(solid(t.text.t2)).child(s)
}

fn meta_sep(t: &Theme) -> impl IntoElement {
    div().text_size(px(11.5)).text_color(solid(t.surfaces.dot_faint)).child("·")
}

fn dot(size: gpui::Pixels, color: Rgb) -> impl IntoElement {
    div().size(size).rounded_full().bg(solid(color))
}

fn lead_pill(t: &Theme, label: &'static str) -> impl IntoElement {
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

fn status_pill(t: &Theme, a: &AgentRun) -> impl IntoElement {
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

// ── race strip ──────────────────────────────────────────────────────────────

fn race_strip(t: &Theme, agents: &[AgentRun]) -> impl IntoElement {
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
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(t.text.t3))
                        .child("THE RACE"),
                )
                .child(dot(px(6.0), t.accent.base))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(solid(t.text.t5))
                        .child("live · isolated worktrees"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(solid(t.text.t5))
                        .child("progress · cost"),
                ),
        )
        .children(agents.iter().map(|a| race_row(t, a)))
}

fn race_row(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(14.0))
        .py(px(9.0))
        .border_t_1()
        .border_color(overlay(t.overlays.w04))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .w(px(190.0))
                .flex_none()
                .child(dot(px(9.0), a.color))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(12.5))
                        .text_color(solid(t.text.t1))
                        .child(a.cli),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(solid(t.text.t5))
                        .child(a.model),
                )
                .when(a.leading, |d| d.child(lead_pill(t, "LEAD"))),
        )
        // progress track
        .child(
            div()
                .flex_1()
                .h(px(7.0))
                .rounded(px(4.0))
                .bg(overlay(t.overlays.w05))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(a.race))
                        .rounded(px(4.0))
                        .bg(solid(a.fill_color(t))),
                ),
        )
        .child(
            div()
                .w(px(44.0))
                .flex_none()
                .text_size(px(11.5))
                .text_color(solid(t.text.t2))
                .child(fmt_usd(a.cost)),
        )
        .child(status_pill(t, a))
}

// ── agent cards ─────────────────────────────────────────────────────────────

fn card_grid(t: &Theme, agents: &[AgentRun]) -> impl IntoElement {
    // Emulate a 2-column grid with rows of two flex_1 cards.
    div().flex().flex_col().gap(px(14.0)).children(
        agents
            .chunks(2)
            .map(|pair| {
                div()
                    .flex()
                    .gap(px(14.0))
                    .children(pair.iter().map(|a| agent_card(t, a)))
            }),
    )
}

fn agent_card(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let border = if a.leading {
        tint(t.accent.base, 0.4)
    } else {
        overlay(t.overlays.w07)
    };
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
        // left accent bar
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(3.0))
                .bg(solid(a.color)),
        )
        .child(card_head(t, a))
        .child(current_activity(t, a))
        .child(metrics_row(t, a))
        .child(gauges(t, a))
        .child(card_footer(t, a))
}

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
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.5))
                                .text_color(solid(t.text.t1))
                                .child(a.cli),
                        )
                        .when(a.leading, |d| d.child(lead_pill(t, "LEADING"))),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(solid(t.text.t5))
                        .child(format!("{} · oryn-{}", a.model, a.id)),
                ),
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
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(11.0))
                .text_color(solid(t.text.t2))
                .child(a.cur_tool),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_size(px(11.0))
                .text_color(solid(t.text.t4))
                .child(a.cur_text),
        )
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
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child(label),
        )
        .child(div().text_size(px(13.0)).text_color(solid(value_color)).child(value))
}

fn gauges(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(11.0))
        .mb(px(15.0))
        .child(gauge(
            t,
            "Tokens",
            format!("{} / {}", fmt_k(a.tokens), fmt_k(TOKEN_CAP)),
            a.token_fraction(),
            a.fill_color(t),
        ))
        .child(gauge(
            t,
            "Budget",
            format!("{} / {}", fmt_usd(a.cost), fmt_usd(COST_CAP)),
            a.cost_fraction(),
            a.fill_color(t),
        ))
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

fn card_footer(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let btn = |label: &'static str| {
        div()
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
            .child(label)
    };
    let promote_enabled = a.test_ok;
    let promote = div()
        .flex()
        .items_center()
        .h(px(28.0))
        .px(px(11.0))
        .rounded(px(7.0))
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .map(|d| {
            if promote_enabled {
                d.bg(solid(t.accent.base)).text_color(solid(0x1A0F2E)).child("Promote")
            } else {
                d.bg(overlay(t.overlays.w03))
                    .border_1()
                    .border_color(overlay(t.overlays.w07))
                    .text_color(solid(t.text.t5))
                    .child("Promote")
            }
        });
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .pt(px(13.0))
        .border_t_1()
        .border_color(overlay(t.overlays.w06))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(solid(t.status.green))
                .child(format!("+{}", a.add)),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(solid(t.status.red))
                .child(format!("−{}", a.del)),
        )
        .child(div().flex_1())
        .child(btn("Timeline"))
        .child(btn("Diff"))
        .child(promote)
}

/// Erase the view's element type so callers can store it behind a `match`.
pub fn mission_control_any(t: &Theme, agents: &[AgentRun]) -> AnyElement {
    mission_control(t, agents).into_any_element()
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
    fn sample_has_four_distinct_runs() {
        let runs = AgentRun::sample();
        assert_eq!(runs.len(), 4);
        let mut ids: Vec<&str> = runs.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        // Exactly one leader.
        assert_eq!(runs.iter().filter(|r| r.leading).count(), 1);
    }

    #[test]
    fn fractions_are_clamped_to_unit() {
        let runs = AgentRun::sample();
        for r in &runs {
            assert!((0.0..=1.0).contains(&r.token_fraction()));
            assert!((0.0..=1.0).contains(&r.cost_fraction()));
        }
        // amp is at the token cap → fraction 1.0.
        let amp = runs.iter().find(|r| r.id == "amp").unwrap();
        assert_eq!(amp.token_fraction(), 1.0);
    }

    #[test]
    fn stopped_run_fills_with_red() {
        let t = Theme::default();
        let amp = AgentRun::sample().into_iter().find(|r| r.id == "amp").unwrap();
        assert_eq!(amp.fill_color(&t), t.status.red);
    }

    #[test]
    fn status_colors_track_lifecycle() {
        let t = Theme::default();
        for r in AgentRun::sample() {
            let expected = match r.status {
                RunStatus::Running => t.status.green,
                RunStatus::Finished => t.status.blue,
                RunStatus::Stopped => t.status.red,
            };
            assert_eq!(r.status_color(&t), expected);
        }
    }
}

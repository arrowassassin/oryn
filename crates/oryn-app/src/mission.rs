//! Mission Control view — the real cascade board.
//!
//! Renders the **real** [`crate::backend::LiveReport`] the orchestrator produced:
//! one row/card per execution attempt, with the tokens the provider reported, the
//! cost computed from live pricing, and the advisor's verdict. Honest empty,
//! running, and failed states cover the rest of the lifecycle — nothing here is
//! simulated.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px, relative};

use crate::Root;
use crate::Screen;
use crate::colors::{overlay, solid, tint};
use crate::state::{AgentRun, Msg, Phase, RunStatus};
use crate::theme::{Rgb, Theme};

// ── formatting helpers ──────────────────────────────────────────────────────

/// Compact thousands: `184200 → "184k"`, `2000 → "2k"`.
pub(crate) fn fmt_k(n: u64) -> String {
    if n >= 100_000 {
        format!("{}k", (n as f64 / 1000.0).round() as u64)
    } else if n >= 1000 {
        let s = format!("{:.1}", n as f64 / 1000.0);
        format!("{}k", s.trim_end_matches(".0"))
    } else {
        n.to_string()
    }
}

/// USD with two decimals.
pub(crate) fn fmt_usd(n: f64) -> String {
    format!("${n:.2}")
}

// ── shared little elements ────────────────────────────────────────────────────

pub(crate) fn dot(size: gpui::Pixels, color: Rgb) -> gpui::Div {
    div().size(size).rounded_full().bg(solid(color))
}

pub(crate) fn status_color(t: &Theme, a: &AgentRun) -> Rgb {
    match a.status {
        RunStatus::Passed => t.status.green,
        RunStatus::Failed => t.status.red,
    }
}

pub(crate) fn status_label(a: &AgentRun) -> &'static str {
    match (a.won, a.status) {
        (true, RunStatus::Passed) => "Verified winner",
        (true, RunStatus::Failed) => "Best-effort winner",
        (false, RunStatus::Passed) => "Passed · not chosen",
        (false, RunStatus::Failed) => "Failed verify",
    }
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
    let c = status_color(t, a);
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
        .child(status_label(a))
}

// ── view (methods on Root) ────────────────────────────────────────────────────

impl Root {
    /// Render the full Mission Control view.
    pub(crate) fn mission_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        let body = match self.phase {
            Phase::Idle => self.empty_state(cx, &t),
            Phase::Running => self.running_state(&t),
            Phase::Failed => self.failed_state(cx, &t),
            Phase::Done => self.board(cx, &t),
        };
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
                    .child(body),
            )
            .into_any_element()
    }

    fn mission_header(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let title = self.task_title();
        let running = self.phase == Phase::Running;
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
                            .min_w(px(0.0))
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
                                    .child(title),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("rerun")
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
                            .on_click(self.launch_run(cx))
                            .child(div().size(px(8.0)).rounded(px(2.0)).bg(solid(if running {
                                t.status.amber
                            } else {
                                t.text.t3
                            })))
                            .child(if running { "Running…" } else { "Re-run" }),
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
                            .child("Compare & promote"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .text_size(px(11.5))
                    .child(div().text_color(solid(t.text.t5)).child("repo"))
                    .child(
                        div()
                            .text_color(solid(t.text.t2))
                            .child(self.repo.label.clone()),
                    )
                    .child(div().text_color(solid(t.surfaces.dot_faint)).child("·"))
                    .child(div().text_color(solid(t.text.t5)).child("base"))
                    .child(
                        div()
                            .text_color(solid(t.text.t2))
                            .child(self.repo.base_ref()),
                    )
                    .child(div().text_color(solid(t.surfaces.dot_faint)).child("·"))
                    .child(dot(
                        px(6.0),
                        if running {
                            t.status.amber
                        } else {
                            t.status.green
                        },
                    ))
                    .child(
                        div()
                            .text_color(solid(t.text.t2))
                            .child(self.status_summary()),
                    ),
            )
    }

    // ── lifecycle states ────────────────────────────────────────────────────

    fn empty_state(&self, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        let go = self.on(cx, Msg::Navigate(Screen::Launch));
        notice_panel(
            t,
            t.accent.base,
            "No run yet",
            "Launch a run to route this task across the coding CLIs you have installed. Oryn discovers each CLI's real models, decomposes the task, and runs the cheapest-capable target first — escalating only when the advisor rejects a result.",
        )
        .child(self.primary_button("launch-empty", "Set up & launch →", go, t))
        .into_any_element()
    }

    fn running_state(&self, t: &Theme) -> AnyElement {
        notice_panel(t, t.status.amber, "Running", &self.run_note).into_any_element()
    }

    fn failed_state(&self, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        let go = self.on(cx, Msg::Navigate(Screen::Launch));
        notice_panel(
            t,
            t.status.red,
            "Run did not produce results",
            &self.run_note,
        )
        .child(self.primary_button("launch-failed", "Open Launch →", go, t))
        .into_any_element()
    }

    fn primary_button(
        &self,
        id: &'static str,
        label: &'static str,
        handler: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
        t: &Theme,
    ) -> impl IntoElement {
        div()
            .id(id)
            .mt(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(16.0))
            .rounded(px(9.0))
            .bg(solid(t.accent.base))
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(solid(0x1A0F2E))
            .cursor_pointer()
            .on_click(handler)
            .child(label)
    }

    // ── the real board ──────────────────────────────────────────────────────

    fn board(&self, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(self.cascade_strip(t))
            .child(self.card_grid(cx, t))
            .into_any_element()
    }

    fn cascade_strip(&self, t: &Theme) -> impl IntoElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        for a in &self.agents {
            rows.push(cascade_row(t, a).into_any_element());
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
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(solid(t.text.t3))
                            .child("THE CASCADE"),
                    )
                    .child(dot(px(6.0), t.accent.base))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(solid(t.text.t5))
                            .child("cheapest-capable first · advisor-gated"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(solid(t.text.t5))
                            .child("advisor score · cost"),
                    ),
            )
            .children(rows)
    }

    fn card_grid(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        for (row, pair) in self.agents.chunks(2).enumerate() {
            let mut cards: Vec<AnyElement> = Vec::new();
            for (col, a) in pair.iter().enumerate() {
                cards.push(self.agent_card(cx, t, row * 2 + col, a));
            }
            rows.push(
                div()
                    .flex()
                    .gap(px(14.0))
                    .children(cards)
                    .into_any_element(),
            );
        }
        div().flex().flex_col().gap(px(14.0)).children(rows)
    }

    fn agent_card(
        &self,
        cx: &mut Context<Self>,
        t: &Theme,
        idx: usize,
        a: &AgentRun,
    ) -> AnyElement {
        let promoted = self.promoted == Some(idx);
        let border = if a.won {
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
            .child(subtask_line(t, a))
            .child(metrics_row(t, a))
            .child(score_gauge(t, a))
            .child(response_snippet(t, a))
            .child(self.card_footer(cx, t, idx, a, promoted))
            .into_any_element()
    }

    fn card_footer(
        &self,
        cx: &mut Context<Self>,
        t: &Theme,
        idx: usize,
        a: &AgentRun,
        promoted: bool,
    ) -> impl IntoElement {
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
            .on_click(self.promote_run(cx, idx))
            .map(|d| {
                if promoted {
                    d.bg(solid(t.status.green))
                        .text_color(solid(0x07120B))
                        .child("Promoted ✓")
                } else if a.status == RunStatus::Passed {
                    d.bg(solid(t.accent.base))
                        .text_color(solid(0x1A0F2E))
                        .child("Promote")
                } else {
                    d.bg(overlay(t.overlays.w03))
                        .border_1()
                        .border_color(overlay(t.overlays.w07))
                        .text_color(solid(t.text.t4))
                        .child("Promote")
                }
            });
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .pt(px(13.0))
            .border_t_1()
            .border_color(overlay(t.overlays.w06))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(solid(t.text.t5))
                    .child(format!("tier {}", a.tier_rank)),
            )
            .when(a.won && a.files_changed > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(solid(t.text.t5))
                        .child(format!("· {} files", a.files_changed)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(solid(t.status.green))
                        .child(format!("+{}", a.added)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(solid(t.status.red))
                        .child(format!("−{}", a.removed)),
                )
            })
            .child(div().flex_1())
            .child(btn("tl", "Timeline", self.on(cx, Msg::OpenTimeline(idx))))
            .child(promote)
    }
}

// ── notice panel (empty / running / failed) ───────────────────────────────────

fn notice_panel(t: &Theme, accent: Rgb, title: &str, body: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_start()
        .max_w(px(640.0))
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(tint(accent, 0.28))
        .rounded(px(13.0))
        .px(px(22.0))
        .py(px(20.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .mb(px(10.0))
                .child(dot(px(8.0), accent))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(t.text.t1))
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(solid(t.text.t3))
                .child(body.to_string()),
        )
}

// ── card sub-parts (real data) ────────────────────────────────────────────────

fn card_head(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .mb(px(12.0))
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
                                .child(a.framework.clone()),
                        )
                        .when(a.won, |d| d.child(pill_chip(t, "WINNER"))),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(solid(t.text.t5))
                        .child(a.model.clone()),
                ),
        )
        .child(div().flex_1())
        .child(status_pill(t, a))
}

fn subtask_line(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(9.0))
        .mb(px(13.0))
        .px(px(11.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(overlay(t.overlays.w025))
        .border_1()
        .border_color(overlay(t.overlays.w05))
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child("SUBTASK"),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_size(px(11.0))
                .text_color(solid(t.text.t2))
                .child(a.subtask.clone()),
        )
}

fn metrics_row(t: &Theme, a: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .gap(px(2.0))
        .mb(px(14.0))
        .child(metric_tile(t, "IN", fmt_k(a.input_tokens), t.text.t1))
        .child(metric_tile(t, "OUT", fmt_k(a.output_tokens), t.text.t1))
        .child(metric_tile(t, "COST", fmt_usd(a.cost), t.text.t1))
        .child(metric_tile(
            t,
            "SCORE",
            format!("{:.2}", a.score),
            status_color(t, a),
        ))
}

fn metric_tile(
    t: &Theme,
    label: &'static str,
    value: String,
    value_color: Rgb,
) -> impl IntoElement {
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
        .child(
            div()
                .text_size(px(13.0))
                .text_color(solid(value_color))
                .child(value),
        )
}

fn score_gauge(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let frac = a.score.clamp(0.0, 1.0) as f32;
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .mb(px(13.0))
        .child(
            div()
                .flex()
                .justify_between()
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(solid(t.text.t4))
                        .child("Advisor verdict"),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(solid(t.text.t3))
                        .child(format!("{:.0}%", frac * 100.0)),
                ),
        )
        .child(
            div()
                .h(px(5.0))
                .rounded(px(3.0))
                .bg(overlay(t.overlays.w06))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(frac))
                        .rounded(px(3.0))
                        .bg(solid(status_color(t, a))),
                ),
        )
}

fn response_snippet(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let text = if a.response.trim().is_empty() {
        "(no winning output for this attempt)".to_string()
    } else {
        let snip: String = a.response.chars().take(220).collect();
        if a.response.chars().count() > 220 {
            format!("{snip}…")
        } else {
            snip
        }
    };
    div()
        .mb(px(13.0))
        .px(px(11.0))
        .py(px(9.0))
        .rounded(px(8.0))
        .bg(overlay(t.overlays.w03))
        .border_1()
        .border_color(overlay(t.overlays.w06))
        .text_size(px(11.0))
        .text_color(solid(t.text.t3))
        .child(text)
}

fn cascade_row(t: &Theme, a: &AgentRun) -> impl IntoElement {
    let frac = a.score.clamp(0.0, 1.0) as f32;
    let fill = status_color(t, a);
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
                .w(px(220.0))
                .flex_none()
                .child(dot(px(9.0), a.color))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(12.5))
                        .text_color(solid(t.text.t1))
                        .child(a.framework.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(solid(t.text.t5))
                        .child(a.model.clone()),
                )
                .when(a.won, |d| d.child(pill_chip(t, "WIN"))),
        )
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
                        .w(relative(frac))
                        .rounded(px(4.0))
                        .bg(solid(fill)),
                ),
        )
        .child(
            div()
                .w(px(48.0))
                .flex_none()
                .text_size(px(11.5))
                .text_color(solid(t.text.t2))
                .child(fmt_usd(a.cost)),
        )
        .child(status_pill(t, a))
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_k_compacts_thousands() {
        assert_eq!(fmt_k(184_200), "184k");
        assert_eq!(fmt_k(96_400), "96.4k");
        assert_eq!(fmt_k(2_000), "2k");
        assert_eq!(fmt_k(512), "512");
    }

    #[test]
    fn fmt_usd_two_places() {
        assert_eq!(fmt_usd(2.4), "$2.40");
        assert_eq!(fmt_usd(0.71), "$0.71");
    }

    #[test]
    fn status_labels_cover_quadrants() {
        let base = AgentRun {
            framework: "codex".into(),
            model: "m".into(),
            subtask: "s".into(),
            color: 0,
            won: true,
            status: RunStatus::Passed,
            score: 0.9,
            tier_rank: 0,
            input_tokens: 1,
            output_tokens: 1,
            cost: 0.0,
            response: String::new(),
            files_changed: 0,
            added: 0,
            removed: 0,
            worktree_session: "oryn-codex-m".into(),
        };
        assert_eq!(status_label(&base), "Verified winner");
        assert_eq!(
            status_label(&AgentRun {
                won: true,
                status: RunStatus::Failed,
                ..base.clone()
            }),
            "Best-effort winner"
        );
        assert_eq!(
            status_label(&AgentRun {
                won: false,
                status: RunStatus::Passed,
                ..base.clone()
            }),
            "Passed · not chosen"
        );
        assert_eq!(
            status_label(&AgentRun {
                won: false,
                status: RunStatus::Failed,
                ..base
            }),
            "Failed verify"
        );
    }
}

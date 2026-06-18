//! Timeline view — the faithful trace of a single run attempt.
//!
//! Tabs switch the selected run row; the status header and lifecycle banner
//! reflect that attempt's real state; and the event stream is reconstructed from
//! the attempt's **real** orchestrator record (routing tier, reported tokens,
//! advisor verdict, and the model's actual output) — not sample data.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px};

use crate::Root;
use crate::Screen;
use crate::colors::{overlay, solid, tint};
use crate::mission::{dot, fmt_k, fmt_usd, status_pill};
use crate::state::{AgentRun, Msg, RunStatus};
use crate::theme::{Rgb, Theme};

/// One reconstructed trace event for a single attempt.
struct TraceEvent {
    kind: String,
    hue: Rgb,
    title: String,
    detail: String,
}

/// Build the real event sequence for an attempt from its orchestrator record.
fn events_for(t: &Theme, a: &AgentRun) -> Vec<TraceEvent> {
    let mut ev = vec![
        TraceEvent {
            kind: "ROUTE".into(),
            hue: t.accent.base,
            title: format!("Routed to {} · {}", a.framework, a.model),
            detail: format!("subtask “{}” · tier {} (0 = cheapest-capable)", a.subtask, a.tier_rank),
        },
        TraceEvent {
            kind: "USAGE".into(),
            hue: t.status.blue,
            title: "Completion".into(),
            detail: format!("{} in · {} out · {}", fmt_k(a.input_tokens), fmt_k(a.output_tokens), fmt_usd(a.cost)),
        },
        TraceEvent {
            kind: "VERIFY".into(),
            hue: if a.status == RunStatus::Passed { t.status.green } else { t.status.red },
            title: format!("Advisor verdict · score {:.2}", a.score),
            detail: if a.status == RunStatus::Passed {
                "passed verification — cascade may stop here".into()
            } else {
                "rejected — orchestrator escalates to the next tier".into()
            },
        },
    ];
    if !a.response.trim().is_empty() {
        ev.push(TraceEvent {
            kind: "OUTPUT".into(),
            hue: t.text.t3,
            title: "Model output".into(),
            detail: a.response.clone(),
        });
    }
    ev
}

impl Root {
    /// Render the Timeline view for the currently-selected run row.
    pub(crate) fn timeline_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        if self.agents.is_empty() {
            return self.timeline_empty(&t);
        }
        let sel = &self.agents[self.selected.min(self.agents.len() - 1)];
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(self.timeline_header(cx, &t))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(run_status_header(&t, sel))
                    .child(self.lifecycle_banner(cx, &t, sel))
                    .child(event_stream(&t, sel)),
            )
            .into_any_element()
    }

    fn timeline_empty(&self, t: &Theme) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(t, "FAITHFUL TRACE", "Timeline"))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_size(px(12.5)).text_color(solid(t.text.t4)).child(self.status_summary())),
            )
            .into_any_element()
    }

    fn timeline_header(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let mut tabs = div()
            .flex()
            .gap(px(3.0))
            .p(px(3.0))
            .rounded(px(9.0))
            .bg(overlay(t.overlays.w035))
            .border_1()
            .border_color(overlay(t.overlays.w06));
        for (i, a) in self.agents.iter().enumerate() {
            let active = i == self.selected;
            tabs = tabs.child(
                div()
                    .id(("tltab", i))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(28.0))
                    .px(px(12.0))
                    .rounded(px(7.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::SelectAgent(i)))
                    .when(active, |d| d.bg(overlay(t.overlays.w09)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)))
                    .when(!active, |d| d.text_color(solid(t.text.t4)))
                    .child(dot(px(7.0), a.color))
                    .child(a.framework.clone()),
            );
        }
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.0))
            .px(px(24.0))
            .py(px(14.0))
            .border_b_1()
            .border_color(overlay(t.overlays.w06))
            .child(div().text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child("FAITHFUL TRACE"))
            .child(tabs)
    }

    fn lifecycle_banner(&self, cx: &mut Context<Self>, t: &Theme, sel: &AgentRun) -> AnyElement {
        let (hue, glyph, title, detail): (Rgb, &str, String, String) = match (sel.won, sel.status) {
            (true, RunStatus::Passed) => (t.status.green, "✓", "Verified winner".into(), "advisor passed this attempt; it was selected for the subtask".into()),
            (true, RunStatus::Failed) => (t.status.amber, "▲", "Best-effort winner".into(), "no candidate passed verification; the highest-scoring attempt was chosen".into()),
            (false, _) => (t.status.red, "■", "Not selected".into(), "a cheaper or higher-scoring attempt won this subtask".into()),
        };
        div()
            .flex()
            .items_center()
            .gap(px(11.0))
            .mx(px(24.0))
            .mt(px(13.0))
            .px(px(14.0))
            .py(px(11.0))
            .rounded(px(9.0))
            .bg(tint(hue, 0.06))
            .border_1()
            .border_color(tint(hue, 0.28))
            .child(div().text_size(px(13.0)).text_color(solid(hue)).child(glyph.to_string()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(hue)).child(title))
                    .child(div().text_size(px(10.5)).text_color(solid(t.text.t3)).child(detail)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("tl-review")
                    .flex()
                    .items_center()
                    .h(px(28.0))
                    .px(px(12.0))
                    .rounded(px(7.0))
                    .bg(tint(t.accent.base, 0.12))
                    .border_1()
                    .border_color(tint(t.accent.base, 0.34))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(t.accent.base))
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::Navigate(Screen::Review)))
                    .child("Review & promote →"),
            )
            .into_any_element()
    }
}

fn run_status_header(t: &Theme, sel: &AgentRun) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(24.0))
        .px(px(24.0))
        .py(px(16.0))
        .bg(solid(t.surfaces.bg2))
        .border_b_1()
        .border_color(overlay(t.overlays.w05))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(dot(px(9.0), sel.color))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(14.0)).text_color(solid(t.text.t1)).child(sel.framework.clone()))
                        .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child(sel.model.clone())),
                )
                .child(status_pill(t, sel)),
        )
        .child(div().flex_1())
        .child(mini_stat(t, "Tokens", format!("{} / {}", fmt_k(sel.input_tokens), fmt_k(sel.output_tokens))))
        .child(mini_stat(t, "Cost", fmt_usd(sel.cost)))
        .child(mini_stat(t, "Score", format!("{:.2}", sel.score)))
}

fn mini_stat(t: &Theme, label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .min_w(px(96.0))
        .child(div().text_size(px(9.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child(label))
        .child(div().text_size(px(13.0)).text_color(solid(t.text.t1)).child(value))
}

fn event_stream(t: &Theme, sel: &AgentRun) -> impl IntoElement {
    div()
        .flex_1()
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(24.0))
        .py(px(14.0))
        .children(events_for(t, sel).into_iter().map(|e| event_row(t, e)))
}

fn event_row(t: &Theme, e: TraceEvent) -> impl IntoElement {
    div()
        .flex()
        .gap(px(14.0))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .w(px(14.0))
                .flex_none()
                .child(dot(px(9.0), e.hue))
                .child(div().w(px(1.5)).flex_1().min_h(px(14.0)).bg(overlay(t.overlays.w07)).my(px(2.0))),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .pb(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .mb(px(3.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(tint(e.hue, 0.13))
                                .border_1()
                                .border_color(tint(e.hue, 0.3))
                                .text_size(px(8.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(solid(e.hue))
                                .child(e.kind),
                        )
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(12.5)).text_color(solid(t.text.t1)).child(e.title)),
                )
                .child(div().text_size(px(11.5)).text_color(solid(t.text.t3)).child(e.detail)),
        )
}

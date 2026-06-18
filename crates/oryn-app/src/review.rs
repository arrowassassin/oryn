//! Review view — compare finished runs and promote a winner.
//!
//! Ranks the race's agents (most progress / passing tests / cheapest first),
//! marks the top one recommended, lets you select any to inspect, and promotes
//! the chosen run to merge. Promotion finishes that agent and stops the rest
//! (see [`Root::promote`]).

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px};

use crate::Root;
use crate::colors::{overlay, solid, tint};
use crate::mission::{RunStatus, dot, fmt_usd, status_pill};
use crate::state::Msg;
use crate::theme::Theme;

impl Root {
    /// Display order: highest progress, then passing tests, then cheapest.
    fn ranking(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.agents.len()).collect();
        idx.sort_by(|&a, &b| {
            let x = &self.agents[a];
            let y = &self.agents[b];
            y.race
                .total_cmp(&x.race)
                .then(y.test_ok.cmp(&x.test_ok))
                .then(x.cost.total_cmp(&y.cost))
        });
        idx
    }

    /// Render the Review view.
    pub(crate) fn review_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        let order = self.ranking();
        let recommended = order.first().copied();

        let mut rows: Vec<AnyElement> = Vec::new();
        for (rank, &i) in order.iter().enumerate() {
            rows.push(self.review_row(cx, &t, rank, i, Some(i) == recommended));
        }

        let promote_bar = recommended.map(|i| self.promote_bar(cx, &t, i));

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(&t, "REVIEW & PROMOTE", "Compare the field"))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .px(px(24.0))
                    .pt(px(18.0))
                    .pb(px(24.0))
                    .children(rows)
                    .when_some(promote_bar, |d, bar| d.child(bar)),
            )
            .into_any_element()
    }

    fn review_row(&self, cx: &mut Context<Self>, t: &Theme, rank: usize, idx: usize, recommended: bool) -> AnyElement {
        let a = &self.agents[idx];
        let selected = idx == self.selected;
        let border = if recommended {
            tint(t.accent.base, 0.34)
        } else if selected {
            overlay(t.overlays.w12)
        } else {
            overlay(t.overlays.w07)
        };
        div()
            .id(("review", idx))
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .py(px(13.0))
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(border)
            .rounded(px(11.0))
            .cursor_pointer()
            .on_click(self.on(cx, Msg::SelectAgent(idx)))
            .child(div().w(px(18.0)).text_size(px(12.0)).text_color(solid(t.text.t5)).child(format!("{}", rank + 1)))
            .child(dot(px(9.0), a.color))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(13.0)).text_color(solid(t.text.t1)).child(a.cli))
                            .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child(a.model))
                            .when(recommended, |d| d.child(recommended_chip(t))),
                    )
                    .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child(format!("+{} −{} · {} files · {} turns", a.add, a.del, a.files, a.turns))),
            )
            .child(div().flex_1())
            .child(diff_stat(t, "tests", a.tests, if a.test_ok { t.status.green } else { t.status.amber }))
            .child(diff_stat(t, "spend", &fmt_usd(a.cost), t.text.t2))
            .child(status_pill(t, a))
            .into_any_element()
    }

    fn promote_bar(&self, cx: &mut Context<Self>, t: &Theme, idx: usize) -> AnyElement {
        let a = &self.agents[idx];
        let label = format!("Promote {} → merge", a.cli);
        let already = a.status == RunStatus::Finished && !self.playing;
        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .mt(px(6.0))
            .px(px(16.0))
            .py(px(14.0))
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(tint(t.accent.base, 0.3))
            .rounded(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)).child("Recommended winner"))
                    .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child("merges the worktree; losing runs are torn down, traces archived")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("promote-bar")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(34.0))
                    .px(px(16.0))
                    .rounded(px(8.0))
                    .bg(solid(t.accent.base))
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(0x1A0F2E))
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::Promote(idx)))
                    .child(if already { "Promoted ✓".to_string() } else { label }),
            )
            .into_any_element()
    }
}

fn recommended_chip(t: &Theme) -> impl IntoElement {
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(5.0))
        .bg(tint(t.accent.base, 0.14))
        .border_1()
        .border_color(tint(t.accent.base, 0.32))
        .text_size(px(8.5))
        .font_weight(FontWeight::BOLD)
        .text_color(solid(t.accent.base))
        .child("RECOMMENDED")
}

fn diff_stat(t: &Theme, label: &'static str, value: &str, value_color: crate::theme::Rgb) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(2.0))
        .min_w(px(56.0))
        .child(div().text_size(px(9.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child(label))
        .child(div().text_size(px(12.0)).text_color(solid(value_color)).child(value.to_string()))
}

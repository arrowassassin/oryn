//! Timeline view — the faithful trace of a single agent's run.
//!
//! Agent tabs switch the [`Root::selected`] run; the status header and lifecycle
//! banner reflect that run's live state; and a normalized event stream shows what
//! the agent did. The stream is representative sample data — the real adapter
//! event feed lands when the app is wired to `oryn-core`'s capture layer.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px, relative};

use crate::Root;
use crate::Screen;
use crate::colors::{overlay, solid, tint};
use crate::mission::{AgentRun, RunStatus, dot, fmt_k, status_pill};
use crate::state::Msg;
use crate::theme::{Rgb, Theme};

/// One normalized trace event.
struct TraceEvent {
    kind: &'static str,
    hue: Rgb,
    time: &'static str,
    title: &'static str,
    detail: &'static str,
    tok: Option<&'static str>,
}

fn sample_events(t: &Theme) -> Vec<TraceEvent> {
    vec![
        TraceEvent { kind: "MESSAGE", hue: t.accent.base, time: "00:03.1", title: "Assistant", detail: "Reading the auth client to locate the refresh race.", tok: Some("+1.2k") },
        TraceEvent { kind: "TOOL_USE", hue: t.status.blue, time: "00:11.4", title: "Read", detail: "src/auth/tokenClient.ts", tok: None },
        TraceEvent { kind: "TOOL_USE", hue: t.status.blue, time: "00:58.2", title: "Edit", detail: "src/auth/tokenClient.ts — add singleFlight guard", tok: None },
        TraceEvent { kind: "FILE_CHANGE", hue: t.status.amber, time: "01:12.5", title: "Write", detail: "src/auth/refreshQueue.ts", tok: None },
        TraceEvent { kind: "MESSAGE", hue: t.accent.base, time: "01:40.0", title: "Assistant", detail: "Coalescing concurrent refreshes behind a shared promise.", tok: Some("+2.0k") },
        TraceEvent { kind: "TOOL_USE", hue: t.status.blue, time: "02:05.7", title: "Edit", detail: "src/auth/__tests__/refresh.test.ts", tok: None },
        TraceEvent { kind: "TOOL_RESULT", hue: t.status.green, time: "02:38.9", title: "Bash", detail: "pnpm vitest run auth/refresh → 14 passed", tok: None },
        TraceEvent { kind: "COST", hue: t.accent.base, time: "02:41.0", title: "Usage", detail: "cumulative 184k tokens · $2.41", tok: Some("+0.4k") },
    ]
}

impl Root {
    /// Render the Timeline view for the currently-selected agent.
    pub(crate) fn timeline_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        let sel = &self.agents[self.selected.min(self.agents.len().saturating_sub(1))];
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
                    .child(event_stream(&t)),
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
                    .child(a.cli),
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
        match sel.status {
            RunStatus::Running => div()
                .flex()
                .items_center()
                .gap(px(11.0))
                .mx(px(24.0))
                .mt(px(13.0))
                .px(px(13.0))
                .py(px(9.0))
                .rounded(px(9.0))
                .bg(tint(t.status.green, 0.05))
                .border_1()
                .border_color(tint(t.status.green, 0.22))
                .child(dot(px(7.0), t.status.green))
                .child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.semantic.ok_fg)).child("Live"))
                .child(div().text_size(px(11.0)).text_color(solid(t.text.t3)).child(format!("streaming — {} is producing events", sel.cli)))
                .child(div().flex_1())
                .child(div().text_size(px(10.5)).text_color(solid(t.text.t6)).child("capturing stdout"))
                .into_any_element(),
            RunStatus::Finished => div()
                .flex()
                .items_center()
                .gap(px(11.0))
                .mx(px(24.0))
                .mt(px(13.0))
                .px(px(14.0))
                .py(px(11.0))
                .rounded(px(9.0))
                .bg(tint(t.status.blue, 0.06))
                .border_1()
                .border_color(tint(t.status.blue, 0.28))
                .child(div().text_size(px(13.0)).text_color(solid(t.status.blue)).child("✓"))
                .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.semantic.info_fg)).child("Completed"))
                .child(div().text_size(px(11.0)).text_color(solid(t.text.t3)).child("session ended naturally"))
                .child(div().flex_1())
                .child(
                    div()
                        .id("tl-review")
                        .flex()
                        .items_center()
                        .h(px(28.0))
                        .px(px(12.0))
                        .rounded(px(7.0))
                        .bg(tint(t.status.blue, 0.12))
                        .border_1()
                        .border_color(tint(t.status.blue, 0.34))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(t.status.blue))
                        .cursor_pointer()
                        .on_click(self.on(cx, Msg::Navigate(Screen::Review)))
                        .child("Review & promote →"),
                )
                .into_any_element(),
            RunStatus::Stopped => div()
                .flex()
                .items_center()
                .gap(px(11.0))
                .mx(px(24.0))
                .mt(px(13.0))
                .px(px(14.0))
                .py(px(11.0))
                .rounded(px(9.0))
                .bg(tint(t.status.red, 0.06))
                .border_1()
                .border_color(tint(t.status.red, 0.3))
                .child(div().text_size(px(12.0)).text_color(solid(t.status.red)).child("■"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.semantic.err_fg)).child("Agent stopped · budget exceeded"))
                        .child(div().text_size(px(10.5)).text_color(solid(t.text.t3)).child("hard stop — process terminated, worktree preserved")),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("tl-resume")
                        .flex()
                        .items_center()
                        .h(px(28.0))
                        .px(px(12.0))
                        .rounded(px(7.0))
                        .bg(tint(t.status.red, 0.1))
                        .border_1()
                        .border_color(tint(t.status.red, 0.34))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(t.semantic.err_fg))
                        .cursor_pointer()
                        .on_click(self.on(cx, Msg::Navigate(Screen::Launch)))
                        .child("Resume with higher cap"),
                )
                .into_any_element(),
        }
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
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(14.0)).text_color(solid(t.text.t1)).child(sel.cli))
                        .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child(format!("{} · oryn-{}", sel.model, sel.id))),
                )
                .child(status_pill(t, sel)),
        )
        .child(div().flex_1())
        .child(mini_gauge(t, "Tokens", format!("{} / {}", fmt_k(sel.tokens), fmt_k(crate::mission::TOKEN_CAP)), sel.token_fraction(), sel.fill_color(t)))
        .child(mini_gauge(t, "Budget", format!("${:.2} / $4.00", sel.cost), sel.cost_fraction(), sel.fill_color(t)))
}

fn mini_gauge(t: &Theme, label: &'static str, value: String, fraction: f32, fill: Rgb) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .min_w(px(120.0))
        .child(
            div()
                .flex()
                .justify_between()
                .child(div().text_size(px(9.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child(label))
                .child(div().text_size(px(10.0)).text_color(solid(t.text.t3)).child(value)),
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

fn event_stream(t: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(24.0))
        .py(px(14.0))
        .children(sample_events(t).into_iter().map(|e| event_row(t, e)))
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
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(12.5)).text_color(solid(t.text.t1)).child(e.title))
                        .child(div().flex_1())
                        .when(e.tok.is_some(), |d| d.child(div().text_size(px(10.5)).text_color(solid(t.accent.base)).child(e.tok.unwrap_or_default())))
                        .child(div().text_size(px(10.5)).text_color(solid(t.text.t6)).child(e.time)),
                )
                .child(div().text_size(px(11.5)).text_color(solid(t.text.t3)).child(e.detail)),
        )
}

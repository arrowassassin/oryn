//! Context Broker view — the cache-stable-prefix economics, made visible.
//!
//! Surfaces the dedup/cache story that underpins Oryn's "route, don't race"
//! thesis: how many artifacts the broker holds, the dedup ratio, and the USD the
//! shared cache-stable prefix saved this mission. Values are derived from live
//! mission spend where possible.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px};

use crate::Root;
use crate::colors::{overlay, solid};
use crate::theme::{Rgb, Theme};

impl Root {
    /// Render the Context Broker view.
    pub(crate) fn broker_view(&self, _cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        // Real numbers from the last run + the detected repo. Before any run, the
        // spend figures are zero and the repo-file count still reflects the real
        // cache-stable prefix the next run will share.
        // Real on-disk content-addressed store stats (persisted across runs).
        let store = crate::backend::ArtifactStore::open();
        let (artifacts, bytes) = store.stats();
        let dedup = self.report.as_ref().map(|r| r.dedup_ratio()).unwrap_or(1.0);
        let saved = self.report.as_ref().map(|r| r.saved_usd).unwrap_or(0.0);

        let stats = div()
            .flex()
            .gap(px(14.0))
            .child(stat_card(
                &t,
                "Artifacts",
                &artifacts.to_string(),
                &format!("{} on disk · content-addressed", fmt_bytes(bytes)),
                t.text.t1,
            ))
            .child(stat_card(
                &t,
                "Dedup ratio",
                &format!("{dedup:.1}×"),
                "context offered ÷ uniquely stored",
                t.status.green,
            ))
            .child(stat_card(
                &t,
                "Cache savings",
                &format!("${saved:.2}"),
                "prompt-cache reads vs. no-cache",
                t.status.green,
            ));

        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(
                &t,
                "CONTEXT BROKER · M2",
                "Shared, deduplicated context",
            ))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .px(px(24.0))
                    .pt(px(18.0))
                    .pb(px(28.0))
                    .child(stats)
                    .child(explainer(&t))
                    .child(prefix_panel(&t)),
            )
            .into_any_element()
    }
}

/// Human-readable byte size (B / KB / MB).
fn fmt_bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn stat_card(
    t: &Theme,
    label: &'static str,
    value: &str,
    sub: &str,
    value_color: Rgb,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w07))
        .rounded(px(12.0))
        .p(px(16.0))
        .child(
            div()
                .mb(px(10.0))
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child(label),
        )
        .child(
            div()
                .text_size(px(26.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(value_color))
                .child(value.to_string()),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(11.0))
                .text_color(solid(t.text.t5))
                .child(sub.to_string()),
        )
}

fn explainer(t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w07))
        .rounded(px(12.0))
        .p(px(16.0))
        .child(div().text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child("HOW IT WORKS"))
        .child(div().text_size(px(12.0)).text_color(solid(t.text.t2)).child("Every racing agent shares one byte-identical, cache-stable prompt prefix. The broker stores it once, content-addressed, and each provider serves it from its prompt cache — so the volatile per-subtask suffix is all that's re-billed."))
}

fn prefix_panel(t: &Theme) -> impl IntoElement {
    let rows = [
        ("system", "persona · capabilities · guardrails", "cached"),
        (
            "repo_map",
            "sorted file index · stable across the race",
            "cached",
        ),
        ("task", "mission goal · acceptance criteria", "cached"),
        ("suffix", "per-subtask instruction", "volatile"),
    ];
    div()
        .flex()
        .flex_col()
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w07))
        .rounded(px(12.0))
        .px(px(16.0))
        .py(px(14.0))
        .child(
            div()
                .mb(px(10.0))
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child("CACHE-STABLE PREFIX"),
        )
        .children(rows.into_iter().map(|(name, desc, tag)| {
            let cached = tag == "cached";
            let tag_color = if cached {
                t.status.green
            } else {
                t.status.amber
            };
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .py(px(9.0))
                .border_t_1()
                .border_color(overlay(t.overlays.w04))
                .child(
                    div()
                        .w(px(76.0))
                        .flex_none()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(solid(t.text.t1))
                        .child(name),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(solid(t.text.t4))
                        .child(desc),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .bg(crate::colors::tint(tag_color, 0.13))
                        .border_1()
                        .border_color(crate::colors::tint(tag_color, 0.3))
                        .text_size(px(9.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(tag_color))
                        .child(tag),
                )
        }))
}

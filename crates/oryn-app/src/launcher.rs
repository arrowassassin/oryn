//! Launcher view — "Launch a race", fully interactive.
//!
//! Pure data ([`Adapter`], [`Estimate`]) plus render methods on [`crate::Root`].
//! Adapter rows toggle selection, the estimate recomputes live from the current
//! selection, and "Launch race" starts a fresh simulation with exactly the
//! selected frameworks. The adapter list is the UI face of the orchestrator's
//! `(framework, model)` discovery.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px, relative};

use crate::Root;
use crate::colors::{overlay, solid, tint};
use crate::mission::COST_CAP;
use crate::state::Msg;
use crate::theme::{Rgb, Theme};

/// Fraction of the worst-case spend expected once the cache-stable prefix is
/// reused across agents (the design shows ~$7.20 of a $16.00 ceiling).
const CACHE_SPEND_FACTOR: f64 = 0.45;

/// One agent framework the user can race.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: &'static str,
    pub cli: &'static str,
    pub color: Rgb,
    pub enabled: bool,
    pub tag: &'static str,
}

impl Adapter {
    /// Frameworks from the design: four credentialed + selected, two off.
    pub fn available() -> Vec<Adapter> {
        vec![
            Adapter { name: "Claude Code", cli: "claude", color: 0xC08CFF, enabled: true, tag: "opus-4.6" },
            Adapter { name: "Codex", cli: "codex", color: 0x4ED99A, enabled: true, tag: "gpt-5.2" },
            Adapter { name: "Gemini CLI", cli: "gemini", color: 0x7FA8FF, enabled: true, tag: "2.5-pro" },
            Adapter { name: "Amp", cli: "amp", color: 0xFFB454, enabled: true, tag: "sonnet-4.6" },
            Adapter { name: "Aider", cli: "aider", color: 0x8B8B95, enabled: false, tag: "available" },
            Adapter { name: "Cursor Agent", cli: "cursor", color: 0x4A4A53, enabled: false, tag: "planned" },
        ]
    }

    fn selectable(&self) -> bool {
        self.tag != "planned"
    }
}

/// Cost estimate derived from the selected adapters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub agents: usize,
    pub max_spend: f64,
    pub with_cache: f64,
}

/// Compute the [`Estimate`] for `adapters`.
pub fn estimate(adapters: &[Adapter]) -> Estimate {
    let agents = adapters.iter().filter(|a| a.enabled).count();
    let max_spend = agents as f64 * COST_CAP;
    Estimate { agents, max_spend, with_cache: max_spend * CACHE_SPEND_FACTOR }
}

fn fmt_usd(n: f64) -> String {
    format!("${n:.2}")
}

// ── view (methods on Root) ────────────────────────────────────────────────────

impl Root {
    /// Render the full Launcher view.
    pub(crate) fn launcher_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(&t, "NEW RUN", "Launch a race"))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .justify_center()
                    .gap(px(22.0))
                    .px(px(24.0))
                    .pt(px(22.0))
                    .pb(px(40.0))
                    .child(self.launch_form(cx, &t))
                    .child(self.estimate_card(cx, &t)),
            )
            .into_any_element()
    }

    fn launch_form(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        div()
            .flex_1()
            .max_w(px(620.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(section(
                t,
                "REPOSITORY",
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(40.0))
                    .px(px(13.0))
                    .bg(solid(t.surfaces.panel))
                    .border_1()
                    .border_color(overlay(t.overlays.w09))
                    .rounded(px(9.0))
                    .child(div().text_size(px(12.5)).text_color(solid(t.text.t1)).child("acme/web-platform"))
                    .child(div().flex_1())
                    .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child("main@4f2ab1c")),
            ))
            .child(section(
                t,
                "TASK",
                div()
                    .min_h(px(96.0))
                    .px(px(14.0))
                    .py(px(13.0))
                    .bg(solid(t.surfaces.panel))
                    .border_1()
                    .border_color(overlay(t.overlays.w09))
                    .rounded(px(9.0))
                    .text_size(px(12.5))
                    .text_color(solid(t.text.t2))
                    .child("The token refresh fires twice under concurrent 401s, causing a refresh race. Add a single-flight guard so concurrent refreshes coalesce, and make auth/refresh.test.ts pass."),
            ))
            .child(self.agents_section(cx, t))
            .child(section(t, "PER-AGENT HARD STOP", caps_row(t)))
            .child(scrub_toggle(self, cx, t))
    }

    fn agents_section(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let selected = self.adapters.iter().filter(|a| a.enabled).count();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (row, pair) in self.adapters.chunks(2).enumerate() {
            let mut cells: Vec<AnyElement> = Vec::new();
            for (col, a) in pair.iter().enumerate() {
                cells.push(self.adapter_row(cx, t, row * 2 + col, a));
            }
            rows.push(div().flex().gap(px(9.0)).children(cells).into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .gap(px(9.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label(t, "AGENTS TO RACE"))
                    .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child(format!("{selected} selected · each gets an isolated worktree"))),
            )
            .child(div().flex().flex_col().gap(px(9.0)).children(rows))
    }

    fn adapter_row(&self, cx: &mut Context<Self>, t: &Theme, idx: usize, a: &Adapter) -> AnyElement {
        let (bg, border) = if a.enabled {
            (tint(t.accent.base, 0.07), tint(t.accent.base, 0.3))
        } else {
            (solid(t.surfaces.panel), overlay(t.overlays.w07))
        };
        let mut row = div()
            .id(("adapter", idx))
            .flex_1()
            .flex()
            .items_center()
            .gap(px(9.0))
            .px(px(12.0))
            .py(px(11.0))
            .rounded(px(9.0))
            .bg(bg)
            .border_1()
            .border_color(border);
        if a.selectable() {
            row = row.cursor_pointer().on_click(self.on(cx, Msg::ToggleAdapter(idx)));
        }
        row.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.0))
                .rounded(px(5.0))
                .border_1()
                .map(|d| {
                    if a.enabled {
                        d.bg(solid(t.accent.base)).border_color(solid(t.accent.base)).text_size(px(11.0)).text_color(solid(0x1A0F2E)).child("✓")
                    } else {
                        d.border_color(overlay(t.overlays.w18))
                    }
                }),
        )
        .child(div().size(px(9.0)).rounded(px(3.0)).bg(solid(a.color)))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)).child(a.name))
                .child(div().text_size(px(10.0)).text_color(solid(t.text.t5)).child(a.cli)),
        )
        .child(div().flex_1())
        .child(
            div()
                .px(px(7.0))
                .py(px(2.0))
                .rounded(px(5.0))
                .bg(overlay(t.overlays.w04))
                .border_1()
                .border_color(overlay(t.overlays.w07))
                .text_size(px(9.5))
                .text_color(solid(t.text.t5))
                .child(a.tag),
        )
        .into_any_element()
    }

    fn estimate_card(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let est = estimate(&self.adapters);
        div()
            .w(px(280.0))
            .flex_none()
            .flex()
            .flex_col()
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(overlay(t.overlays.w09))
            .rounded(px(13.0))
            .p(px(18.0))
            .child(div().mb(px(16.0)).text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t3)).child("ESTIMATE"))
            .child(estimate_row(t, "Agents", est.agents.to_string(), t.text.t1, true))
            .child(estimate_row(t, "Max spend", fmt_usd(est.max_spend), t.text.t1, true))
            .child(estimate_row(t, "Est. with cache", fmt_usd(est.with_cache), t.status.green, true))
            .child(estimate_row(t, "Wall clock cap", "15 min".to_string(), t.text.t1, false))
            .child(
                div()
                    .id("launch")
                    .mt(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(40.0))
                    .rounded(px(9.0))
                    .bg(solid(t.accent.base))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(0x1A0F2E))
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::StartRace))
                    .child("▸ Launch race"),
            )
            .child(div().mt(px(9.0)).flex().justify_center().text_size(px(10.5)).text_color(solid(t.text.t6)).child("⌘↵ to launch"))
    }
}

// ── static / shared sub-parts ──────────────────────────────────────────────────

fn scrub_toggle(root: &Root, cx: &mut Context<Root>, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(11.0))
        .px(px(14.0))
        .py(px(12.0))
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w09))
        .rounded(px(9.0))
        .child(crate::settings::toggle_switch(root, cx, t, "scrub", root.settings.scrub, Msg::ToggleScrub))
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(solid(t.text.t1)).child("Scrub secrets before persist"))
                .child(div().text_size(px(10.5)).text_color(solid(t.text.t5)).child("redact tokens & keys from raw payloads")),
        )
}

fn caps_row(t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.0))
        .child(cap_card(t, "Token cap", "300k", 0.6, "kill agent on exceed"))
        .child(cap_card(t, "USD cap", "$4.00", 0.5, "tracked from cost events"))
}

fn cap_card(t: &Theme, label: &'static str, value: &'static str, fraction: f32, note: &'static str) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .px(px(14.0))
        .py(px(13.0))
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w09))
        .rounded(px(9.0))
        .child(
            div()
                .flex()
                .justify_between()
                .mb(px(9.0))
                .child(div().text_size(px(11.0)).text_color(solid(t.text.t3)).child(label))
                .child(div().text_size(px(12.0)).text_color(solid(t.accent.base)).child(value)),
        )
        .child(
            div()
                .h(px(5.0))
                .rounded(px(3.0))
                .bg(overlay(t.overlays.w07))
                .overflow_hidden()
                .child(div().h_full().w(relative(fraction)).rounded(px(3.0)).bg(solid(t.accent.base))),
        )
        .child(div().mt(px(8.0)).text_size(px(10.0)).text_color(solid(t.text.t6)).child(note))
}

fn estimate_row(t: &Theme, label: &'static str, value: String, value_color: Rgb, border: bool) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .py(px(9.0))
        .when(border, |d| d.border_b_1().border_color(overlay(t.overlays.w05)))
        .child(div().text_size(px(12.0)).text_color(solid(t.text.t3)).child(label))
        .child(div().text_size(px(12.5)).text_color(solid(value_color)).child(value))
}

pub(crate) fn section_label(t: &Theme, label: &'static str) -> impl IntoElement {
    div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t3)).child(label)
}

pub(crate) fn section(t: &Theme, label: &'static str, body: impl IntoElement) -> impl IntoElement {
    div().flex().flex_col().gap(px(9.0)).child(section_label(t, label)).child(body)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_has_four_enabled() {
        let ads = Adapter::available();
        assert_eq!(ads.len(), 6);
        assert_eq!(ads.iter().filter(|a| a.enabled).count(), 4);
    }

    #[test]
    fn estimate_matches_design_numbers() {
        let est = estimate(&Adapter::available());
        assert_eq!(est.agents, 4);
        assert!((est.max_spend - 16.00).abs() < 1e-9);
        assert!((est.with_cache - 7.20).abs() < 1e-9);
    }

    #[test]
    fn estimate_scales_with_selection() {
        let e = estimate(&[]);
        assert_eq!(e.agents, 0);
        assert_eq!(e.max_spend, 0.0);
        assert_eq!(e.with_cache, 0.0);
    }

    #[test]
    fn cache_estimate_below_max_spend() {
        let est = estimate(&Adapter::available());
        assert!(est.with_cache < est.max_spend);
    }

    #[test]
    fn planned_is_not_selectable() {
        let cursor = Adapter { name: "Cursor Agent", cli: "cursor", color: 0, enabled: false, tag: "planned" };
        assert!(!cursor.selectable());
    }
}

//! Settings view — Preferences.
//!
//! Appearance (theme, accent, density, UI font, reduce-motion), run defaults
//! (token/USD caps, auto-tear-down), privacy (secret scrub, telemetry), the
//! worktree base path, and a live-preview card. Mirrors the Settings screen in the
//! design handoff (`Oryn.dc.html`).
//!
//! Rendering reflects the currently-resolved [`Theme`]: the active theme mode and
//! accent swatch are highlighted. Wiring the controls to mutate state is a
//! follow-up (it needs GPUI click handlers threaded through every screen).

use gpui::prelude::FluentBuilder;
use gpui::{AnyElement, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px, relative};

use crate::colors::{overlay, solid, tint};
use crate::theme::{ACCENTS, Mode, Rgb, Theme};

/// Render the full Settings view.
pub fn settings(t: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .min_h(px(0.0))
        .child(super::view_header(t, "SETTINGS", "Preferences"))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .flex()
                .justify_center()
                .gap(px(20.0))
                .px(px(24.0))
                .pt(px(20.0))
                .pb(px(40.0))
                .child(settings_column(t))
                .child(preview_column(t)),
        )
}

fn settings_column(t: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .max_w(px(600.0))
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(appearance_card(t))
        .child(run_defaults_card(t))
        .child(privacy_card(t))
        .child(worktree_card(t))
}

// ── cards ─────────────────────────────────────────────────────────────────────

fn card(t: &Theme, title: &'static str, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w07))
        .rounded(px(12.0))
        .px(px(18.0))
        .py(px(16.0))
        .child(
            div()
                .mb(px(8.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t3))
                .child(title),
        )
        .child(body)
}

fn appearance_card(t: &Theme) -> impl IntoElement {
    let accent_name = t.accent.name;
    card(
        t,
        "APPEARANCE",
        div()
            .flex()
            .flex_col()
            .child(setting_row(
                t,
                "Theme",
                "surface tone across the app",
                segmented(t, &["Dark", "Light", "Auto"], if t.mode == Mode::Dark { 0 } else { 1 }),
            ))
            .child(setting_row(
                t,
                format!("Accent · {accent_name}"),
                "focus, primary actions, live state",
                accent_swatches(t),
            ))
            .child(setting_row(t, "Density", "row height and padding", segmented(t, &["Compact", "Comfortable", "Spacious"], 1)))
            .child(setting_row(t, "UI font", "data & code always use the mono", segmented(t, &["Geist", "IBM Plex", "System"], 0)))
            .child(setting_row(t, "Reduce motion", "pauses the live race & gauge animation", toggle(t, false))),
    )
}

fn run_defaults_card(t: &Theme) -> impl IntoElement {
    card(
        t,
        "RUN DEFAULTS",
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap(px(14.0))
                    .mb(px(6.0))
                    .child(cap_field(t, "Default token cap", "300k", 0.6))
                    .child(cap_field(t, "Default USD cap", "$4.00", 0.5)),
            )
            .child(setting_row(
                t,
                "Auto-tear-down losing worktrees",
                "remove non-promoted worktrees after a race",
                toggle(t, true),
            )),
    )
}

fn privacy_card(t: &Theme) -> impl IntoElement {
    card(
        t,
        "PRIVACY & DATA",
        div()
            .flex()
            .flex_col()
            .child(setting_row(t, "Scrub secrets before persist", "redact tokens & keys from raw payloads", toggle(t, true)))
            .child(setting_row(t, "Anonymous telemetry", "never includes code or payloads", toggle(t, false))),
    )
}

fn worktree_card(t: &Theme) -> impl IntoElement {
    card(
        t,
        "WORKTREE",
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .h(px(38.0))
            .px(px(13.0))
            .bg(solid(t.surfaces.bg2))
            .border_1()
            .border_color(overlay(t.overlays.w09))
            .rounded(px(9.0))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(t.text.t5))
                    .child("BASE"),
            )
            .child(div().text_size(px(12.0)).text_color(solid(t.text.t1)).child("~/.oryn/worktrees")),
    )
}

// ── controls ────────────────────────────────────────────────────────────────

fn setting_row(t: &Theme, title: impl Into<SharedString>, sub: &'static str, control: impl IntoElement) -> impl IntoElement {
    let title: SharedString = title.into();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .py(px(12.0))
        .border_t_1()
        .border_color(overlay(t.overlays.w05))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(solid(t.text.t1))
                        .child(title),
                )
                .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child(sub)),
        )
        .child(control)
}

fn segmented(t: &Theme, labels: &[&'static str], active: usize) -> impl IntoElement {
    div()
        .flex()
        .gap(px(3.0))
        .p(px(3.0))
        .rounded(px(9.0))
        .bg(overlay(t.overlays.w035))
        .border_1()
        .border_color(overlay(t.overlays.w06))
        .children(labels.iter().enumerate().map(|(i, label)| {
            let is_active = i == active;
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.0))
                .px(px(14.0))
                .rounded(px(7.0))
                .text_size(px(12.0))
                .when(is_active, |d| {
                    d.bg(overlay(t.overlays.w09))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(t.text.t1))
                })
                .when(!is_active, |d| d.text_color(solid(t.text.t4)))
                .child(*label)
        }))
}

fn accent_swatches(t: &Theme) -> impl IntoElement {
    div().flex().gap(px(9.0)).children(ACCENTS.iter().map(|a| {
        let selected = a.base == t.accent.base;
        div()
            .size(px(32.0))
            .rounded(px(9.0))
            .bg(solid(a.base))
            .border_1()
            .border_color(overlay(t.overlays.w10))
            // selection ring approximated with an accent-colored border highlight
            .when(selected, |d| d.border_2().border_color(solid(a.base)))
    }))
}

fn toggle(t: &Theme, on: bool) -> impl IntoElement {
    let track = div()
        .relative()
        .w(px(36.0))
        .h(px(21.0))
        .rounded(px(11.0))
        .flex_none()
        .border_1();
    let track = if on {
        track.bg(solid(t.accent.base)).border_color(tint(t.accent.base, 0.0))
    } else {
        track.bg(overlay(t.overlays.w13)).border_color(overlay(t.overlays.w08))
    };
    track.child(
        div()
            .absolute()
            .top(px(2.0))
            .map(|d| if on { d.right(px(2.0)) } else { d.left(px(2.0)) })
            .size(px(15.0))
            .rounded_full()
            .bg(solid(0xFFFFFF)),
    )
}

fn cap_field(t: &Theme, label: &'static str, value: &'static str, fraction: f32) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .justify_between()
                .mb(px(8.0))
                .child(div().text_size(px(11.5)).text_color(solid(t.text.t2)).child(label))
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
}

// ── live preview ──────────────────────────────────────────────────────────────

fn preview_column(t: &Theme) -> impl IntoElement {
    div()
        .w(px(268.0))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child("LIVE PREVIEW"),
        )
        .child(preview_card(t))
        .child(
            div()
                .px(px(2.0))
                .text_size(px(10.5))
                .text_color(solid(t.text.t5))
                .child(
                    "Accent applies to focus rings, primary actions, the leading agent, and live \
                     indicators across every screen.",
                ),
        )
}

fn preview_card(t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(tint(t.accent.base, 0.4))
        .rounded(px(13.0))
        .p(px(16.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .mb(px(13.0))
                .child(div().size(px(9.0)).rounded(px(3.0)).bg(solid(t.accent.base)))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.0))
                        .text_color(solid(t.text.t1))
                        .child("claude"),
                )
                .child(
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
                        .child("LEADING"),
                ),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .mb(px(6.0))
                .child(div().text_size(px(10.0)).text_color(solid(t.text.t4)).child("Tokens"))
                .child(div().text_size(px(10.5)).text_color(solid(t.text.t3)).child("184k / 300k")),
        )
        .child(
            div()
                .h(px(6.0))
                .mb(px(14.0))
                .rounded(px(3.0))
                .bg(overlay(t.overlays.w06))
                .overflow_hidden()
                .child(div().h_full().w(relative(0.61)).rounded(px(3.0)).bg(solid(t.accent.base))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(32.0))
                .rounded(px(8.0))
                .bg(solid(t.accent.base))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(0x1A0F2E))
                .child("Promote"),
        )
}

/// Convenience: the active accent's base hue under `t` (used by tests/preview).
pub fn active_accent(t: &Theme) -> Rgb {
    t.accent.base
}

/// Type-erased entry point for screen dispatch.
pub fn settings_any(t: &Theme) -> AnyElement {
    settings(t).into_any_element()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_accent_tracks_theme() {
        let t = Theme::default();
        assert_eq!(active_accent(&t), t.accent.base);
        assert_eq!(active_accent(&t), 0xC08CFF);
    }

    #[test]
    fn light_and_dark_resolve_distinct_surfaces() {
        let d = Theme::resolve(Mode::Dark, ACCENTS[0]);
        let l = Theme::resolve(Mode::Light, ACCENTS[0]);
        assert_ne!(d.surfaces.panel, l.surfaces.panel);
    }
}

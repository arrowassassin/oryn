//! Settings view — Preferences, fully interactive.
//!
//! Every control mutates [`crate::state::Settings`] through a [`Msg`] and the
//! theme re-resolves live, so changing the accent or mode repaints the whole app
//! instantly. Also exposes the reusable [`toggle_switch`] and segmented-control
//! builders used by other screens (e.g. the Launcher's scrub toggle).

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, SharedString, Styled, div, px, relative};

use crate::Root;
use crate::colors::{overlay, solid, tint};
use crate::state::{Density, FontChoice, Msg, ThemeChoice};
use crate::theme::{ACCENTS, Theme};

impl Root {
    /// Render the full Settings view.
    pub(crate) fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(&t, "SETTINGS", "Preferences"))
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
                    .child(self.settings_column(cx, &t))
                    .child(preview_column(&t)),
            )
            .into_any_element()
    }

    fn settings_column(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        div()
            .flex_1()
            .max_w(px(600.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.appearance_card(cx, t))
            .child(self.run_defaults_card(cx, t))
            .child(self.privacy_card(cx, t))
            .child(worktree_card(t))
    }

    fn appearance_card(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let s = &self.settings;
        let theme_seg = self.segmented(
            cx,
            t,
            "theme",
            vec![
                ("Dark", Msg::SetTheme(ThemeChoice::Dark), s.theme == ThemeChoice::Dark),
                ("Light", Msg::SetTheme(ThemeChoice::Light), s.theme == ThemeChoice::Light),
                ("Auto", Msg::SetTheme(ThemeChoice::Auto), s.theme == ThemeChoice::Auto),
            ],
        );
        let density_seg = self.segmented(
            cx,
            t,
            "density",
            vec![
                ("Compact", Msg::SetDensity(Density::Compact), s.density == Density::Compact),
                ("Comfortable", Msg::SetDensity(Density::Comfortable), s.density == Density::Comfortable),
                ("Spacious", Msg::SetDensity(Density::Spacious), s.density == Density::Spacious),
            ],
        );
        let font_seg = self.segmented(
            cx,
            t,
            "font",
            vec![
                ("Geist", Msg::SetFont(FontChoice::Geist), s.font == FontChoice::Geist),
                ("IBM Plex", Msg::SetFont(FontChoice::IbmPlex), s.font == FontChoice::IbmPlex),
                ("System", Msg::SetFont(FontChoice::System), s.font == FontChoice::System),
            ],
        );
        card(
            t,
            "APPEARANCE",
            div()
                .flex()
                .flex_col()
                .child(setting_row(t, "Theme", "surface tone across the app", theme_seg))
                .child(setting_row(t, format!("Accent · {}", t.accent.name), "focus, primary actions, live state", self.accent_swatches(cx, t)))
                .child(setting_row(t, "Density", "row height and padding", density_seg))
                .child(setting_row(t, "UI font", "data & code always use the mono", font_seg))
                .child(setting_row(t, "Reduce motion", "pauses the live race & gauge animation", toggle_switch(self, cx, t, "reduce", self.settings.reduce_motion, Msg::ToggleReduceMotion))),
        )
    }

    fn run_defaults_card(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
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
                .child(setting_row(t, "Auto-tear-down losing worktrees", "remove non-promoted worktrees after a race", toggle_switch(self, cx, t, "cleanup", self.settings.auto_cleanup, Msg::ToggleAutoCleanup))),
        )
    }

    fn privacy_card(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        card(
            t,
            "PRIVACY & DATA",
            div()
                .flex()
                .flex_col()
                .child(setting_row(t, "Scrub secrets before persist", "redact tokens & keys from raw payloads", toggle_switch(self, cx, t, "scrub2", self.settings.scrub, Msg::ToggleScrub)))
                .child(setting_row(t, "Anonymous telemetry", "never includes code or payloads", toggle_switch(self, cx, t, "telemetry", self.settings.telemetry, Msg::ToggleTelemetry))),
        )
    }

    fn segmented(&self, cx: &mut Context<Self>, t: &Theme, id: &'static str, opts: Vec<(&'static str, Msg, bool)>) -> AnyElement {
        let mut row = div()
            .flex()
            .gap(px(3.0))
            .p(px(3.0))
            .rounded(px(9.0))
            .bg(overlay(t.overlays.w035))
            .border_1()
            .border_color(overlay(t.overlays.w06));
        for (i, (label, msg, active)) in opts.into_iter().enumerate() {
            row = row.child(
                div()
                    .id((id, i))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .px(px(14.0))
                    .rounded(px(7.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .on_click(self.on(cx, msg))
                    .when(active, |d| d.bg(overlay(t.overlays.w09)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)))
                    .when(!active, |d| d.text_color(solid(t.text.t4)))
                    .child(label),
            );
        }
        row.into_any_element()
    }

    fn accent_swatches(&self, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        let mut row = div().flex().gap(px(9.0));
        for (i, a) in ACCENTS.iter().enumerate() {
            let selected = a.base == t.accent.base;
            row = row.child(
                div()
                    .id(("accent", i))
                    .size(px(32.0))
                    .rounded(px(9.0))
                    .bg(solid(a.base))
                    .border_1()
                    .border_color(overlay(t.overlays.w10))
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::SetAccent(i)))
                    .when(selected, |d| d.border_2().border_color(solid(a.base))),
            );
        }
        row.into_any_element()
    }
}

// ── reusable controls ──────────────────────────────────────────────────────────

/// An interactive on/off switch bound to `msg`. Shared across screens.
pub(crate) fn toggle_switch(root: &Root, cx: &mut Context<Root>, t: &Theme, id: &'static str, on: bool, msg: Msg) -> impl IntoElement {
    let track = div()
        .id(id)
        .relative()
        .w(px(36.0))
        .h(px(21.0))
        .rounded(px(11.0))
        .flex_none()
        .border_1()
        .cursor_pointer()
        .on_click(root.on(cx, msg));
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

// ── static sub-parts ────────────────────────────────────────────────────────────

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
        .child(div().mb(px(8.0)).text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t3)).child(title))
        .child(body)
}

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
                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(solid(t.text.t1)).child(title))
                .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child(sub)),
        )
        .child(control)
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
            .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child("BASE"))
            .child(div().text_size(px(12.0)).text_color(solid(t.text.t1)).child("~/.oryn/worktrees")),
    )
}

fn preview_column(t: &Theme) -> impl IntoElement {
    div()
        .w(px(268.0))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child("LIVE PREVIEW"))
        .child(preview_card(t))
        .child(div().px(px(2.0)).text_size(px(10.5)).text_color(solid(t.text.t5)).child("Accent applies to focus rings, primary actions, the leading agent, and live indicators across every screen."))
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
                .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(13.0)).text_color(solid(t.text.t1)).child("claude"))
                .child(crate::mission::pill_chip(t, "LEADING")),
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

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Mode;

    #[test]
    fn light_and_dark_resolve_distinct_surfaces() {
        let d = Theme::resolve(Mode::Dark, ACCENTS[0]);
        let l = Theme::resolve(Mode::Light, ACCENTS[0]);
        assert_ne!(d.surfaces.panel, l.surfaces.panel);
    }
}

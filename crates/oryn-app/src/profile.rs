//! Profile view — account identity, workspace, and a live preferences summary.
//!
//! The preferences block reflects the current [`crate::state::Settings`] so the
//! screen stays in sync with whatever was changed in Settings.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px};

use crate::Root;
use crate::colors::{overlay, solid, tint};
use crate::state::{Density, FontChoice, ThemeChoice};
use crate::theme::Theme;

impl Root {
    /// Render the Profile view.
    pub(crate) fn profile_view(&self, _cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(&t, "ACCOUNT", "Profile"))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .justify_center()
                    .px(px(24.0))
                    .pt(px(20.0))
                    .pb(px(40.0))
                    .child(
                        div()
                            .flex_1()
                            .max_w(px(640.0))
                            .flex()
                            .flex_col()
                            .gap(px(16.0))
                            .child(self.identity_card(&t))
                            .child(self.workspace_card(&t))
                            .child(self.preferences_card(&t)),
                    ),
            )
            .into_any_element()
    }

    fn preferences_card(&self, t: &Theme) -> impl IntoElement {
        let s = &self.settings;
        let theme_label = match s.theme {
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
            ThemeChoice::Auto => "Auto",
        };
        let density_label = match s.density {
            Density::Compact => "Compact",
            Density::Comfortable => "Comfortable",
            Density::Spacious => "Spacious",
        };
        let font_label = match s.font {
            FontChoice::Geist => "Geist",
            FontChoice::IbmPlex => "IBM Plex",
            FontChoice::System => "System",
        };
        card(
            t,
            "PREFERENCES",
            div()
                .flex()
                .flex_col()
                .child(pref_row(t, "Theme", theme_label.to_string()))
                .child(pref_row(t, "Accent", t.accent.name.to_string()))
                .child(pref_row(t, "Density", density_label.to_string()))
                .child(pref_row(t, "UI font", font_label.to_string()))
                .child(pref_row(
                    t,
                    "Telemetry",
                    if s.telemetry {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                ))
                .child(pref_row(
                    t,
                    "Auto-tear-down",
                    if s.auto_cleanup {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                )),
        )
    }

    fn identity_card(&self, t: &Theme) -> impl IntoElement {
        let id = &self.identity;
        div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(overlay(t.overlays.w07))
            .rounded(px(13.0))
            .p(px(18.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(54.0))
                    .rounded_full()
                    .bg(tint(t.accent.base, 0.18))
                    .border_1()
                    .border_color(tint(t.accent.base, 0.4))
                    .text_size(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(solid(t.accent.base))
                    .child(id.initials.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(17.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(solid(t.text.t1))
                            .child(id.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(solid(t.text.t3))
                            .child(id.email.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(7.0))
                            .mt(px(5.0))
                            .child(badge("LOCAL", t.accent.base))
                            .child(badge_str(self.repo.label.clone(), t.text.t3)),
                    ),
            )
    }

    fn workspace_card(&self, t: &Theme) -> impl IntoElement {
        let frameworks = self.adapters.iter().filter(|a| a.enabled).count();
        card(
            t,
            "WORKSPACE",
            div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .gap(px(20.0))
                        .child(field(t, "Repository", self.repo.label.clone()))
                        .child(field(t, "Base", self.repo.base_ref()))
                        .child(field(t, "Frameworks", format!("{frameworks} selected"))),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(20.0))
                        .child(field(t, "Advisor", self.advisor.model.clone()))
                        .child(field(t, "Endpoint", self.advisor.endpoint.clone()))
                        .child(field(
                            t,
                            "Worktrees",
                            crate::backend::worktree_base_display(),
                        )),
                ),
        )
    }
}

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
                .mb(px(10.0))
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child(title),
        )
        .child(body)
}

fn field(t: &Theme, label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .min_w(px(0.0))
        .child(
            div()
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child(label),
        )
        .child(
            div()
                .overflow_hidden()
                .text_size(px(13.0))
                .text_color(solid(t.text.t1))
                .child(value),
        )
}

fn pref_row(t: &Theme, label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .py(px(10.0))
        .border_t_1()
        .border_color(overlay(t.overlays.w05))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(solid(t.text.t3))
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(solid(t.text.t1))
                .child(value),
        )
}

fn badge(label: &'static str, hue: crate::theme::Rgb) -> impl IntoElement {
    badge_str(label.to_string(), hue)
}

fn badge_str(label: String, hue: crate::theme::Rgb) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(5.0))
        .bg(tint(hue, 0.13))
        .border_1()
        .border_color(tint(hue, 0.3))
        .text_size(px(9.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(solid(hue))
        .child(label)
}

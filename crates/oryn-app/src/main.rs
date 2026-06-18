//! Oryn desktop app — GPUI frontend.
//!
//! This is the application shell (top bar + left navigation rail) rendered from
//! the design tokens in [`theme`]. Individual views (Mission Control, Timeline,
//! Review, Broker, Launcher, Settings, Profile) are layered in next; for now the
//! main area shows the Mission Control header as a placeholder.

// The frontend is under active construction: the full design tokens are
// transcribed up front, but not every one is wired into a view yet.
#![allow(dead_code)]

mod colors;
mod launcher;
mod mission;
mod settings;
mod theme;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Application, Bounds, Context, FontWeight, Window, WindowBounds, WindowOptions,
    div, px, size,
};

use colors::{overlay, solid, tint};
use launcher::Adapter;
use mission::AgentRun;
use theme::{ACCENTS, Mode, Theme};

/// A simple view header: an uppercase kicker over a large title. Shared by the
/// screens whose design uses this pattern (Launcher, Settings, …).
pub(crate) fn view_header(t: &Theme, kicker: &'static str, title: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .px(px(24.0))
        .pt(px(18.0))
        .pb(px(14.0))
        .border_b_1()
        .border_color(overlay(t.overlays.w06))
        .child(
            div()
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t5))
                .child(kicker),
        )
        .child(
            div()
                .text_size(px(21.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(solid(t.text.t1))
                .child(title),
        )
}

/// Which primary view is active in the main area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Mission,
    Timeline,
    Review,
    Broker,
    Launch,
    Settings,
    Profile,
}

struct Root {
    theme: Theme,
    screen: Screen,
    agents: Vec<AgentRun>,
    adapters: Vec<Adapter>,
}

impl Root {
    fn new() -> Self {
        Self {
            theme: Theme::resolve(Mode::Dark, ACCENTS[0]),
            screen: Screen::Mission,
            agents: AgentRun::sample(),
            adapters: Adapter::available(),
        }
    }

    fn top_bar(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_none()
            .h(px(46.0))
            .items_center()
            .gap(px(14.0))
            .pl(px(16.0))
            .pr(px(14.0))
            .bg(solid(t.surfaces.bg3))
            .border_b_1()
            .border_color(overlay(t.overlays.w07))
            // brand
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(
                        div()
                            .size(px(16.0))
                            .rounded(px(4.0))
                            .bg(solid(t.accent.base)),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(solid(t.text.t1))
                            .child("oryn"),
                    ),
            )
            .child(div().w(px(1.0)).h(px(18.0)).bg(overlay(t.overlays.w10)))
            // mission selector
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(solid(t.text.t1))
                    .child("Fix flaky token-refresh race"),
            )
            .child(div().flex_1())
            // command palette search
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(30.0))
                    .min_w(px(240.0))
                    .pl(px(12.0))
                    .pr(px(10.0))
                    .rounded(px(8.0))
                    .bg(overlay(t.overlays.w04))
                    .border_1()
                    .border_color(overlay(t.overlays.w08))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(solid(t.text.t3))
                            .child("Search agents, files, commands…"),
                    )
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .bg(overlay(t.overlays.w07))
                            .text_size(px(10.5))
                            .text_color(solid(t.text.t3))
                            .child("⌘K"),
                    ),
            )
            // mission burn
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .h(px(30.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .bg(overlay(t.overlays.w03))
                    .border_1()
                    .border_color(overlay(t.overlays.w07))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(solid(t.text.t5))
                            .child("MISSION BURN"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(solid(t.text.t1))
                            .child("$8.40"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(solid(t.text.t5))
                            .child("/ $16.00"),
                    ),
            )
            // new run
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .h(px(30.0))
                    .px(px(13.0))
                    .rounded(px(8.0))
                    .bg(solid(t.accent.base))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(0x1A0F2E))
                    .child("+ New run"),
            )
    }

    fn nav_item(
        &self,
        cx: &mut Context<Self>,
        screen: Screen,
        label: &'static str,
    ) -> impl IntoElement {
        let t = &self.theme;
        let active = self.screen == screen;
        let icon_color = if active {
            solid(t.accent.base)
        } else {
            solid(t.text.t5)
        };
        let label_color = if active {
            solid(t.text.t2)
        } else {
            solid(t.text.t5)
        };
        div()
            .id(label)
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .w(px(50.0))
            .py(px(8.0))
            .rounded(px(9.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.screen = screen;
                cx.notify();
            }))
            .when(active, |d| d.bg(overlay(t.overlays.w05)))
            // active accent bar
            .when(active, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(-8.0))
                        .top(px(13.0))
                        .bottom(px(13.0))
                        .w(px(2.5))
                        .rounded(px(3.0))
                        .bg(solid(t.accent.base)),
                )
            })
            .child(div().size(px(18.0)).rounded(px(5.0)).bg(icon_color))
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(label_color)
                    .child(label),
            )
    }

    fn left_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .gap(px(3.0))
            .w(px(66.0))
            .py(px(10.0))
            .bg(solid(t.surfaces.bg2))
            .border_r_1()
            .border_color(overlay(t.overlays.w07))
            .child(self.nav_item(cx, Screen::Mission, "Mission"))
            .child(self.nav_item(cx, Screen::Timeline, "Timeline"))
            .child(self.nav_item(cx, Screen::Review, "Review"))
            .child(self.nav_item(cx, Screen::Broker, "Broker"))
            .child(div().flex_1())
            .child(self.nav_item(cx, Screen::Launch, "Launch"))
            .child(div().w(px(30.0)).h(px(1.0)).my(px(6.0)).bg(overlay(t.overlays.w08)))
            .child(self.nav_item(cx, Screen::Settings, "Settings"))
            .child(self.nav_item(cx, Screen::Profile, "You"))
    }

    fn main_area(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .bg(solid(t.surfaces.bg))
            .child(match self.screen {
                Screen::Mission => mission::mission_control_any(t, &self.agents),
                Screen::Launch => launcher::launcher_any(t, &self.adapters),
                Screen::Settings => settings::settings_any(t),
                other => self.placeholder(other),
            })
    }

    /// A labelled placeholder for views not yet implemented.
    fn placeholder(&self, screen: Screen) -> AnyElement {
        let t = &self.theme;
        let label = match screen {
            Screen::Mission => "Mission Control",
            Screen::Timeline => "Timeline — faithful trace",
            Screen::Review => "Review & promote",
            Screen::Broker => "Context broker",
            Screen::Launch => "Launch a race",
            Screen::Settings => "Settings",
            Screen::Profile => "Profile",
        };
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .px(px(14.0))
                    .py(px(9.0))
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(tint(t.accent.base, 0.3))
                    .bg(tint(t.accent.base, 0.08))
                    .text_size(px(12.0))
                    .text_color(solid(t.accent.base))
                    .child(format!("{label} — coming next")),
            )
            .into_any_element()
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(solid(self.theme.surfaces.bg))
            .text_color(solid(self.theme.text.t1))
            .child(self.top_bar())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h(px(0.0))
                    .child(self.left_rail(cx))
                    .child(self.main_area()),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| Root::new()),
        )
        .unwrap();
    });
}

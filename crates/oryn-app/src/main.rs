//! Oryn desktop app — GPUI frontend.
//!
//! A fully interactive mission-control shell over the Oryn orchestrator: a live,
//! simulated agent race plus configuration, trace, review, broker, settings, and
//! profile screens. State lives on [`Root`]; all mutation flows through
//! [`state::Msg`] (see [`state`]). The render tree is split per screen across the
//! view modules, each implementing methods on [`Root`].

mod backend;
mod broker;
mod colors;
mod launcher;
mod mission;
mod profile;
mod review;
mod settings;
mod state;
mod theme;
mod timeline;

use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    App, Application, Bounds, Context, FontWeight, Task, Window, WindowBounds, WindowOptions, div,
    px, size,
};

use colors::{overlay, solid};
use launcher::Adapter;
use mission::{AgentRun, fmt_usd};
use oryn_core::orchestrator::catalog_store::CatalogBundle;
use state::{AdvisorPrefs, Msg, Settings};
use theme::Theme;

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
        .child(div().text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child(kicker))
        .child(div().text_size(px(21.0)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)).child(title))
}

/// Which primary view is active in the main area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Mission,
    Timeline,
    Review,
    Broker,
    Launch,
    Settings,
    Profile,
}

/// The root application state.
pub struct Root {
    pub settings: Settings,
    pub screen: Screen,
    pub agents: Vec<AgentRun>,
    pub adapters: Vec<Adapter>,
    /// Index of the agent shown in the Timeline / Review detail screens.
    pub selected: usize,
    /// Whether the live race simulation is advancing.
    pub playing: bool,
    /// User-chosen local advisor connection (endpoint + model).
    pub advisor: AdvisorPrefs,
    /// The pinned model catalog (capability + live pricing) for this session.
    pub catalog: CatalogBundle,
    /// Background ticker driving the simulation (None in headless tests).
    _timer: Option<Task<()>>,
    /// Background ticker refreshing the parked catalog on an interval.
    _catalog_timer: Option<Task<()>>,
}

impl Root {
    /// Construct the app and start the 0.9s simulation ticker.
    fn new(cx: &mut Context<Self>) -> Self {
        let timer = cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(900)).await;
                let alive = weak
                    .update(cx, |this, cx| {
                        this.tick();
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        // Refresh the parked catalog off the UI thread: on startup, then every
        // 30 min check staleness and re-park if a refresh is due (offline-safe).
        let catalog_timer = cx.spawn(async move |weak, cx| {
            loop {
                let bundle = cx.background_executor().spawn(async { backend::load_catalog() }).await;
                let alive = weak
                    .update(cx, |this, cx| {
                        this.catalog = bundle;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(30 * 60)).await;
            }
        });

        let mut root = Self::headless();
        root._timer = Some(timer);
        root._catalog_timer = Some(catalog_timer);
        root
    }

    /// Construct the app without tickers and with the offline seed catalog — used
    /// by unit tests and as the instant, network-free startup state.
    pub fn headless() -> Self {
        Self {
            settings: Settings::default(),
            screen: Screen::Mission,
            agents: AgentRun::sample(),
            adapters: Adapter::available(),
            selected: 0,
            playing: true,
            advisor: AdvisorPrefs::from_env(),
            catalog: CatalogBundle::seed(),
            _timer: None,
            _catalog_timer: None,
        }
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
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
            .child(
                div()
                    .id("brand")
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::Navigate(Screen::Mission)))
                    .child(div().size(px(16.0)).rounded(px(4.0)).bg(solid(t.accent.base)))
                    .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t1)).child("oryn")),
            )
            .child(div().w(px(1.0)).h(px(18.0)).bg(overlay(t.overlays.w10)))
            .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(solid(t.text.t1)).child("Fix flaky token-refresh race"))
            .child(div().flex_1())
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
                    .child(div().flex_1().text_size(px(12.0)).text_color(solid(t.text.t3)).child("Search agents, files, commands…"))
                    .child(div().px(px(6.0)).py(px(2.0)).rounded(px(5.0)).bg(overlay(t.overlays.w07)).text_size(px(10.5)).text_color(solid(t.text.t3)).child("⌘K")),
            )
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
                    .child(div().text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(solid(t.text.t5)).child("MISSION BURN"))
                    .child(div().text_size(px(12.0)).text_color(solid(t.text.t1)).child(fmt_usd(self.mission_spend())))
                    .child(div().text_size(px(11.0)).text_color(solid(t.text.t5)).child(format!("/ {}", fmt_usd(self.mission_cap())))),
            )
            .child(
                div()
                    .id("newrun")
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
                    .cursor_pointer()
                    .on_click(self.on(cx, Msg::Navigate(Screen::Launch)))
                    .child("+ New run"),
            )
    }

    fn nav_item(&self, cx: &mut Context<Self>, screen: Screen, label: &'static str) -> impl IntoElement {
        let t = self.theme();
        let active = self.screen == screen;
        let icon_color = if active { solid(t.accent.base) } else { solid(t.text.t5) };
        let label_color = if active { solid(t.text.t2) } else { solid(t.text.t5) };
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
            .on_click(self.on(cx, Msg::Navigate(screen)))
            .when(active, |d| d.bg(overlay(t.overlays.w05)))
            .when(active, |d| {
                d.child(div().absolute().left(px(-8.0)).top(px(13.0)).bottom(px(13.0)).w(px(2.5)).rounded(px(3.0)).bg(solid(t.accent.base)))
            })
            .child(div().size(px(18.0)).rounded(px(5.0)).bg(icon_color))
            .child(div().text_size(px(9.0)).text_color(label_color).child(label))
    }

    fn left_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
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

    fn main_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .bg(solid(t.surfaces.bg))
            .child(match self.screen {
                Screen::Mission => self.mission_view(cx),
                Screen::Timeline => self.timeline_view(cx),
                Screen::Review => self.review_view(cx),
                Screen::Broker => self.broker_view(cx),
                Screen::Launch => self.launcher_view(cx),
                Screen::Settings => self.settings_view(cx),
                Screen::Profile => self.profile_view(cx),
            })
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(solid(t.surfaces.bg))
            .text_color(solid(t.text.t1));
        if let Some(font) = self.settings.font_family() {
            root = root.font_family(font);
        }
        root.child(self.top_bar(cx)).child(
            div()
                .flex_1()
                .flex()
                .min_h(px(0.0))
                .child(self.left_rail(cx))
                .child(self.main_area(cx)),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(Root::new),
        );
        if let Err(err) = opened {
            // No display (headless CI, missing DISPLAY/WAYLAND_DISPLAY, …):
            // report cleanly and exit rather than panicking with a backtrace.
            eprintln!("oryn: could not open a window: {err}");
            std::process::exit(1);
        }
    });
}

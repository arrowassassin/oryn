//! Oryn desktop app — GPUI frontend.
//!
//! A real mission-control shell over the Oryn orchestrator. Launching a run
//! discovers the models each installed coding CLI reports, decomposes the task,
//! and runs the deterministic "route, don't race" cascade through the real
//! engine on a background thread; Mission Control, Timeline, and Review render
//! the **real** [`backend::LiveReport`] that comes back. No simulation.
//!
//! State lives on [`Root`]; pure mutations flow through [`state::Msg`], while
//! side-effecting actions (launching a run, editing the task) have dedicated
//! handlers here.

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
    App, Application, Bounds, Context, FocusHandle, FontWeight, KeyDownEvent, Task, Window,
    WindowBounds, WindowOptions, div, px, size,
};

use backend::{LiveReport, RepoInfo};
use colors::{overlay, solid};
use launcher::Adapter;
use mission::fmt_usd;
use oryn_core::orchestrator::catalog_store::CatalogBundle;
use state::{AdvisorPrefs, AgentRun, CatalogSource, Msg, Phase, Settings};
use theme::Theme;

/// The default task text shown in a fresh session, so the Launch screen is never
/// blank — the user edits it in place before launching.
const DEFAULT_TASK: &str = "Describe the change you want. e.g. Fix the flaky token-refresh race so concurrent 401s coalesce behind a single-flight guard, and make the refresh test pass.";

/// A simple view header: an uppercase kicker over a large title.
pub(crate) fn view_header(
    t: &Theme,
    kicker: &'static str,
    title: &'static str,
) -> impl IntoElement {
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
    /// Real run rows (one per orchestrator attempt); empty until a run completes.
    pub agents: Vec<AgentRun>,
    pub adapters: Vec<Adapter>,
    /// Index of the run row shown in the Timeline / Review detail screens.
    pub selected: usize,
    /// The run row the user promoted as the winner, if any.
    pub promoted: Option<usize>,
    /// The git repository the app was launched in (real).
    pub repo: RepoInfo,
    /// The editable task description that becomes the mission goal.
    pub task: String,
    /// Focus handle for the task editor (None in headless tests).
    pub task_focus: Option<FocusHandle>,
    /// Lifecycle of the current run.
    pub phase: Phase,
    /// The last real engine result.
    pub report: Option<LiveReport>,
    /// Human-readable status line for the current phase.
    pub run_note: String,
    /// User-chosen local advisor connection (endpoint + model).
    pub advisor: AdvisorPrefs,
    /// The pinned model catalog (capability + live pricing) for this session.
    pub catalog: CatalogBundle,
    /// Where catalog pricing + benchmark data is fetched from.
    pub catalog_source: CatalogSource,
    /// Last data-source verification result, shown in Settings.
    pub source_status: Option<String>,
    /// Background ticker refreshing the parked catalog on an interval.
    _catalog_timer: Option<Task<()>>,
    /// Handle to the in-flight engine run, if any.
    _run: Option<Task<()>>,
}

impl Root {
    /// Construct the app, detect the repo, and start the catalog refresher.
    fn new(cx: &mut Context<Self>) -> Self {
        let catalog_timer = cx.spawn(async move |weak, cx| {
            loop {
                let Ok(source) = weak.update(cx, |this, _| this.catalog_source) else {
                    break;
                };
                let bundle = cx
                    .background_executor()
                    .spawn(async move { backend::load_catalog(source) })
                    .await;
                if weak
                    .update(cx, |this, cx| {
                        this.catalog = bundle;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(30 * 60))
                    .await;
            }
        });

        let mut root = Self::headless();
        root.task_focus = Some(cx.focus_handle());
        root._catalog_timer = Some(catalog_timer);
        root
    }

    /// Construct the app without async tasks — used by unit tests and as the
    /// instant, network-free startup state. Repo is detected from the cwd.
    pub fn headless() -> Self {
        Self {
            settings: Settings::default(),
            screen: Screen::Mission,
            agents: Vec::new(),
            adapters: Adapter::available(),
            selected: 0,
            promoted: None,
            repo: RepoInfo::detect(),
            task: DEFAULT_TASK.to_string(),
            task_focus: None,
            phase: Phase::Idle,
            report: None,
            run_note: String::new(),
            advisor: AdvisorPrefs::from_env(),
            catalog: CatalogBundle::seed(),
            catalog_source: CatalogSource::default_from_env(),
            source_status: None,
            _catalog_timer: None,
            _run: None,
        }
    }

    /// Launch a real engine run on a background thread. Snapshots the current
    /// inputs, flips to the Running phase, jumps to Mission Control, then ingests
    /// the [`LiveReport`] when the orchestrator returns. Ignored while a run is
    /// already in flight.
    pub fn launch_run(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, _e: &gpui::ClickEvent, _w, cx| {
            if this.phase == Phase::Running {
                return;
            }
            this.phase = Phase::Running;
            this.run_note = "running — discovering models and routing subtasks".into();
            this.agents.clear();
            this.report = None;
            this.promoted = None;
            this.screen = Screen::Mission;
            cx.notify();

            let adapters = this.adapters.clone();
            let endpoint = this.advisor.endpoint.clone();
            let model = this.advisor.model.clone();
            let bundle = this.catalog.clone();
            let repo = this.repo.clone();
            let task = this.task.clone();

            let task_handle = cx.spawn(async move |weak, cx| {
                let report = cx
                    .background_executor()
                    .spawn(async move {
                        backend::run_live(&adapters, &endpoint, &model, &bundle, &repo, &task)
                    })
                    .await;
                let _ = weak.update(cx, |this, cx| {
                    this.ingest_report(report);
                    cx.notify();
                });
            });
            this._run = Some(task_handle);
        })
    }

    /// Key handler for the editable task field: appends typed characters and
    /// handles backspace / space / enter, so the task is genuinely edited in the
    /// app (not a static label).
    pub fn task_key(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, e: &KeyDownEvent, _w, cx| {
            let k = &e.keystroke;
            // Ignore command/control chords (shortcuts), not text entry.
            if k.modifiers.control || k.modifiers.platform || k.modifiers.function {
                return;
            }
            match k.key.as_str() {
                "backspace" => {
                    this.task.pop();
                }
                "space" => this.task.push(' '),
                "enter" => this.task.push('\n'),
                _ => {
                    if let Some(ch) = &k.key_char {
                        this.task.push_str(ch);
                    } else if k.key.chars().count() == 1 {
                        this.task.push_str(&k.key);
                    } else {
                        return;
                    }
                }
            }
            cx.notify();
        })
    }

    /// Focus the task editor (and clear the placeholder on first focus).
    pub fn focus_task(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, _e: &gpui::ClickEvent, window: &mut Window, cx| {
            if this.task == DEFAULT_TASK {
                this.task.clear();
            }
            if let Some(fh) = this.task_focus.clone() {
                window.focus(&fh);
            }
            cx.notify();
        })
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let title = self.task_title();
        let saved = self.report.as_ref().map(|r| r.saved_usd).unwrap_or(0.0);
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
            .child(
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(solid(t.text.t1))
                    .child(title),
            )
            .child(div().flex_1())
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
                            .child(fmt_usd(self.mission_spend())),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(solid(t.status.green))
                            .child(format!("saved {}", fmt_usd(saved))),
                    ),
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

    /// A one-line title for the current task (first line, trimmed/elided).
    pub(crate) fn task_title(&self) -> String {
        let first = self.task.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            "Untitled run".to_string()
        } else if first.chars().count() > 64 {
            format!("{}…", first.chars().take(63).collect::<String>())
        } else {
            first.to_string()
        }
    }

    fn nav_item(
        &self,
        cx: &mut Context<Self>,
        screen: Screen,
        label: &'static str,
    ) -> impl IntoElement {
        let t = self.theme();
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
            .on_click(self.on(cx, Msg::Navigate(screen)))
            .when(active, |d| d.bg(overlay(t.overlays.w05)))
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
            .child(
                div()
                    .w(px(30.0))
                    .h(px(1.0))
                    .my(px(6.0))
                    .bg(overlay(t.overlays.w08)),
            )
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
            eprintln!("oryn: could not open a window: {err}");
            std::process::exit(1);
        }
    });
}

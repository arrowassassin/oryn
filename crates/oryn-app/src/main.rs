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

use backend::{LiveReport, RepoInfo, UserIdentity};
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
    /// The local developer identity (git config / OS user).
    pub identity: UserIdentity,
    /// The editable task description that becomes the mission goal.
    pub task: String,
    /// Caret position as a byte index into `task` (always on a char boundary).
    pub task_cursor: usize,
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
        // Restore persisted preferences over the env-derived defaults.
        if let Some(cfg) = backend::load_config() {
            root.apply_config(cfg);
        }
        root.task_focus = Some(cx.focus_handle());
        root._catalog_timer = Some(catalog_timer);
        root
    }

    /// Construct the app without async tasks — used by unit tests and as the
    /// instant, network-free startup state. Repo is detected from the cwd.
    pub fn headless() -> Self {
        let repo = RepoInfo::detect();
        let identity = UserIdentity::detect(&repo.root);
        Self {
            settings: Settings::default(),
            screen: Screen::Mission,
            agents: Vec::new(),
            adapters: Adapter::available(),
            selected: 0,
            promoted: None,
            repo,
            identity,
            task: DEFAULT_TASK.to_string(),
            task_cursor: DEFAULT_TASK.len(),
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

    /// Promote run row `idx`: mark it the winner, then apply its worktree's
    /// changes onto the repo working tree (and tear down the losing worktrees when
    /// auto-cleanup is on) on a background thread.
    pub fn promote_run(
        &self,
        cx: &mut Context<Self>,
        idx: usize,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _e: &gpui::ClickEvent, _w, cx| {
            if idx >= this.agents.len() {
                return;
            }
            this.promoted = Some(idx);
            this.selected = idx;
            let winner = this.agents[idx].worktree_session.clone();
            if winner.is_empty() {
                cx.notify();
                return;
            }
            let losers: Vec<String> = this
                .agents
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != idx)
                .map(|(_, a)| a.worktree_session.clone())
                .filter(|s| !s.is_empty())
                .collect();
            let repo = this.repo.root.clone();
            let cleanup = this.settings.auto_cleanup;
            this.run_note = "promoting…".into();
            cx.notify();

            let handle = cx.spawn(async move |weak, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { backend::promote_winner(&repo, &winner, &losers, cleanup) })
                    .await;
                let _ = weak.update(cx, |this, cx| {
                    this.run_note = match result {
                        Ok(msg) => msg,
                        Err(e) => format!("promote failed: {e}"),
                    };
                    cx.notify();
                });
            });
            this._run = Some(handle);
        })
    }

    // ── task editing (pure, cursor-aware, unit-tested) ──────────────────────

    /// Clamp the caret to a valid char boundary within `task`.
    fn clamp_cursor(&mut self) {
        if self.task_cursor > self.task.len() {
            self.task_cursor = self.task.len();
        }
        while self.task_cursor < self.task.len() && !self.task.is_char_boundary(self.task_cursor) {
            self.task_cursor += 1;
        }
    }

    /// Insert text at the caret and advance past it.
    pub fn task_insert(&mut self, s: &str) {
        self.clamp_cursor();
        self.task.insert_str(self.task_cursor, s);
        self.task_cursor += s.len();
    }

    /// Delete the char before the caret (backspace).
    pub fn task_backspace(&mut self) {
        self.clamp_cursor();
        if self.task_cursor == 0 {
            return;
        }
        let prev = self.task[..self.task_cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(0);
        let start = self.task_cursor - prev;
        self.task.replace_range(start..self.task_cursor, "");
        self.task_cursor = start;
    }

    /// Delete the char at the caret (forward delete).
    pub fn task_delete(&mut self) {
        self.clamp_cursor();
        if self.task_cursor >= self.task.len() {
            return;
        }
        let next = self.task[self.task_cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(0);
        self.task
            .replace_range(self.task_cursor..self.task_cursor + next, "");
    }

    /// Move the caret one char left.
    pub fn task_left(&mut self) {
        self.clamp_cursor();
        if self.task_cursor > 0 {
            let prev = self.task[..self.task_cursor]
                .chars()
                .next_back()
                .map(char::len_utf8)
                .unwrap_or(0);
            self.task_cursor -= prev;
        }
    }

    /// Move the caret one char right.
    pub fn task_right(&mut self) {
        self.clamp_cursor();
        if self.task_cursor < self.task.len() {
            let next = self.task[self.task_cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
            self.task_cursor += next;
        }
    }

    /// The task with a caret glyph rendered at the current position.
    pub(crate) fn task_with_caret(&self) -> String {
        let cur = self.task_cursor.min(self.task.len());
        let cur = if self.task.is_char_boundary(cur) {
            cur
        } else {
            self.task.len()
        };
        format!("{}\u{2502}{}", &self.task[..cur], &self.task[cur..])
    }

    /// Key handler for the editable task field — a real single-field editor with
    /// caret movement (arrows/home/end), insert, backspace, forward-delete, and
    /// clipboard paste (cmd/ctrl+V).
    pub fn task_key(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, e: &KeyDownEvent, _w, cx| {
            let k = &e.keystroke;
            let chord = k.modifiers.control || k.modifiers.platform;
            // Paste is the one chord we handle as text entry.
            if chord {
                if k.key == "v"
                    && let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
                {
                    this.task_insert(&text);
                    cx.notify();
                }
                return;
            }
            if k.modifiers.function {
                return;
            }
            match k.key.as_str() {
                "backspace" => this.task_backspace(),
                "delete" => this.task_delete(),
                "left" => this.task_left(),
                "right" => this.task_right(),
                "home" | "up" => this.task_cursor = 0,
                "end" | "down" => this.task_cursor = this.task.len(),
                "space" => this.task_insert(" "),
                "enter" => this.task_insert("\n"),
                _ => {
                    if let Some(ch) = &k.key_char {
                        this.task_insert(ch);
                    } else if k.key.chars().count() == 1 {
                        let key = k.key.clone();
                        this.task_insert(&key);
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
                this.task_cursor = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str, cursor: usize) -> Root {
        let mut r = Root::headless();
        r.task = text.to_string();
        r.task_cursor = cursor;
        r
    }

    #[test]
    fn insert_at_caret_advances() {
        let mut r = editor("helo", 2);
        r.task_insert("X");
        assert_eq!(r.task, "heXlo");
        assert_eq!(r.task_cursor, 3);
    }

    #[test]
    fn backspace_and_delete_at_caret() {
        let mut r = editor("abcd", 2);
        r.task_backspace();
        assert_eq!((r.task.as_str(), r.task_cursor), ("acd", 1));
        r.task_delete();
        assert_eq!((r.task.as_str(), r.task_cursor), ("ad", 1));
    }

    #[test]
    fn caret_navigation_bounds() {
        let mut r = editor("ab", 0);
        r.task_left(); // no-op at start
        assert_eq!(r.task_cursor, 0);
        r.task_right();
        r.task_right();
        r.task_right(); // clamps at end
        assert_eq!(r.task_cursor, 2);
    }

    #[test]
    fn editing_is_utf8_safe() {
        // Caret between two multibyte chars; backspace removes a whole char.
        let mut r = editor("áé", "á".len());
        r.task_insert("x");
        assert_eq!(r.task, "áxé");
        r.task_backspace();
        assert_eq!(r.task, "áé");
        // Caret rendering never slices mid-char.
        let _ = r.task_with_caret();
    }

    #[test]
    fn caret_glyph_rendered_at_position() {
        let r = editor("ab", 1);
        assert_eq!(r.task_with_caret(), "a\u{2502}b");
    }
}

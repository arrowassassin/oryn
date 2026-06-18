//! Application state and the update path.
//!
//! [`Settings`] and the running [`mission::AgentRun`] simulation live on the
//! `Root` view (defined in `main.rs`). All mutation funnels through a single
//! [`Msg`] enum applied by [`crate::Root::apply`], which keeps the update logic in
//! one tested place and the render functions free of side effects. The pure state
//! transitions ([`tick`](crate::Root::tick), [`toggle_adapter`](crate::Root::toggle_adapter),
//! [`start_race`](crate::Root::start_race), …) are unit-tested without a GPUI
//! context.

use gpui::{App, ClickEvent, Context, Window};

use crate::Root;
use crate::Screen;
use crate::mission::{AgentRun, COST_CAP, RunStatus, TOKEN_CAP};
use crate::theme::{ACCENTS, Accent, Mode, Theme};

// ── settings ──────────────────────────────────────────────────────────────────

/// Theme selection (Auto resolves to Dark — no system signal is available).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    Dark,
    Light,
    Auto,
}

/// Row-height / padding density.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,
    Comfortable,
    Spacious,
}

/// UI font family choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontChoice {
    Geist,
    IbmPlex,
    System,
}

/// User preferences, applied live across the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub accent_idx: usize,
    pub density: Density,
    pub font: FontChoice,
    pub reduce_motion: bool,
    pub scrub: bool,
    pub telemetry: bool,
    pub auto_cleanup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            accent_idx: 0,
            density: Density::Comfortable,
            font: FontChoice::Geist,
            reduce_motion: false,
            scrub: true,
            telemetry: false,
            auto_cleanup: true,
        }
    }
}

impl Settings {
    /// The resolved color [`Mode`] for the current theme choice.
    pub fn mode(&self) -> Mode {
        match self.theme {
            ThemeChoice::Light => Mode::Light,
            ThemeChoice::Dark | ThemeChoice::Auto => Mode::Dark,
        }
    }

    /// The active accent.
    pub fn accent(&self) -> Accent {
        ACCENTS[self.accent_idx.min(ACCENTS.len() - 1)]
    }

    /// The font-family name passed to GPUI (None → use the default).
    pub fn font_family(&self) -> Option<&'static str> {
        match self.font {
            FontChoice::Geist => None,
            FontChoice::IbmPlex => Some("IBM Plex Sans"),
            FontChoice::System => Some(".SystemUIFont"),
        }
    }
}

// ── messages ──────────────────────────────────────────────────────────────────

/// Every state transition the UI can request. `Copy` so handlers can move it into
/// click listeners cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Navigate(Screen),
    SetTheme(ThemeChoice),
    SetAccent(usize),
    SetDensity(Density),
    SetFont(FontChoice),
    ToggleReduceMotion,
    ToggleScrub,
    ToggleTelemetry,
    ToggleAutoCleanup,
    ToggleAdapter(usize),
    TogglePlay,
    SelectAgent(usize),
    /// Open a specific agent's timeline.
    OpenTimeline(usize),
    StartRace,
    Promote(usize),
    /// Choose the local advisor model from [`ADVISOR_MODELS`].
    SetAdvisorModel(usize),
}

/// Local advisor models the user can pick in Settings (deterministic + reasoning).
pub const ADVISOR_MODELS: [&str; 5] =
    ["qwen2.5-coder:7b", "deepseek-r1:7b", "qwq", "llama3.2:3b", "qwen2.5-coder:1.5b"];

/// User-chosen connection to the local advisor model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisorPrefs {
    /// OpenAI-compatible endpoint base URL (Ollama / llamafile / llama.cpp).
    pub endpoint: String,
    /// The selected model name.
    pub model: String,
    /// Last readiness-check result, shown in Settings.
    pub status: Option<String>,
}

impl AdvisorPrefs {
    /// Read defaults from the environment (`ORYN_ADVISOR_ENDPOINT`,
    /// `ORYN_ADVISOR_MODEL`), falling back to a local Ollama with the default model.
    pub fn from_env() -> Self {
        let endpoint =
            std::env::var("ORYN_ADVISOR_ENDPOINT").unwrap_or_else(|_| "http://localhost:11434".into());
        let model = std::env::var("ORYN_ADVISOR_MODEL").unwrap_or_else(|_| ADVISOR_MODELS[0].into());
        Self { endpoint, model, status: None }
    }
}

impl Root {
    /// Apply a [`Msg`], mutating state. Pure except for the routing; the heavy
    /// transitions live in dedicated tested methods.
    pub fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Navigate(s) => self.screen = s,
            Msg::SetTheme(c) => self.settings.theme = c,
            Msg::SetAccent(i) => self.settings.accent_idx = i.min(ACCENTS.len() - 1),
            Msg::SetDensity(d) => self.settings.density = d,
            Msg::SetFont(f) => self.settings.font = f,
            Msg::ToggleReduceMotion => self.settings.reduce_motion = !self.settings.reduce_motion,
            Msg::ToggleScrub => self.settings.scrub = !self.settings.scrub,
            Msg::ToggleTelemetry => self.settings.telemetry = !self.settings.telemetry,
            Msg::ToggleAutoCleanup => self.settings.auto_cleanup = !self.settings.auto_cleanup,
            Msg::ToggleAdapter(i) => self.toggle_adapter(i),
            Msg::TogglePlay => self.playing = !self.playing,
            Msg::SelectAgent(i) => self.select_agent(i),
            Msg::OpenTimeline(i) => {
                self.select_agent(i);
                self.screen = Screen::Timeline;
            }
            Msg::StartRace => self.start_race(),
            Msg::Promote(i) => self.promote(i),
            Msg::SetAdvisorModel(i) => {
                if let Some(m) = ADVISOR_MODELS.get(i) {
                    self.advisor.model = (*m).to_string();
                    self.advisor.status = None;
                }
            }
        }
    }

    /// The live-resolved theme.
    pub fn theme(&self) -> Theme {
        Theme::resolve(self.settings.mode(), self.settings.accent())
    }

    /// Build a click handler that applies `msg` and requests a re-render. The
    /// single place [`Context::notify`] is called for user actions.
    pub fn on(
        &self,
        cx: &mut Context<Self>,
        msg: Msg,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _e: &ClickEvent, _w, cx| {
            this.apply(msg);
            cx.notify();
        })
    }

    /// Toggle an adapter's selected state, if the index is valid and the adapter
    /// is selectable (planned adapters cannot be enabled).
    pub fn toggle_adapter(&mut self, i: usize) {
        if let Some(a) = self.adapters.get_mut(i)
            && a.tag != "planned"
        {
            a.enabled = !a.enabled;
        }
    }

    /// Select an agent for the Timeline / Review detail views.
    pub fn select_agent(&mut self, i: usize) {
        if i < self.agents.len() {
            self.selected = i;
        }
    }

    /// Begin a fresh race using exactly the enabled adapters, then jump to Mission
    /// Control with the simulation playing.
    pub fn start_race(&mut self) {
        let fresh: Vec<AgentRun> = self
            .adapters
            .iter()
            .filter(|a| a.enabled)
            .map(AgentRun::launching)
            .collect();
        if !fresh.is_empty() {
            self.agents = fresh;
            self.selected = 0;
            self.recompute_leader();
        }
        self.playing = true;
        self.screen = Screen::Mission;
    }

    /// Mark agent `i` the promoted winner: it finishes, the rest stop.
    pub fn promote(&mut self, i: usize) {
        if i >= self.agents.len() {
            return;
        }
        for (j, a) in self.agents.iter_mut().enumerate() {
            if j == i {
                a.status = RunStatus::Finished;
                a.race = 1.0;
            } else if a.status == RunStatus::Running {
                a.status = RunStatus::Stopped;
            }
        }
        self.playing = false;
        self.recompute_leader();
    }

    /// Advance the simulation by one tick. No-op when paused or reduce-motion is
    /// on. Running agents accrue tokens/cost/turns, advance toward the finish, and
    /// hard-stop when a budget cap is hit.
    pub fn tick(&mut self) {
        if !self.playing || self.settings.reduce_motion {
            return;
        }
        for a in &mut self.agents {
            if a.status != RunStatus::Running {
                continue;
            }
            a.elapsed_sec += 1;
            let delta = 1_200 + (a.turns % 5) * 300;
            a.tokens = (a.tokens + delta).min(TOKEN_CAP);
            // ~$12 per million tokens — believable blended rate for the sim.
            a.cost = (a.cost + delta as f64 * 0.000_012).min(COST_CAP);
            if a.elapsed_sec.is_multiple_of(6) {
                a.turns += 1;
            }
            a.race = (a.race + (0.985 - a.race) * 0.05).min(0.985);

            if a.tokens >= TOKEN_CAP {
                a.status = RunStatus::Stopped;
                a.cur_tool = "killed";
                a.cur_text = "token budget exceeded";
            } else if a.cost >= COST_CAP {
                a.status = RunStatus::Stopped;
                a.cur_tool = "killed";
                a.cur_text = "USD budget exceeded";
            } else if a.race >= 0.97 {
                a.status = RunStatus::Finished;
                a.race = 1.0;
                a.cur_tool = "done";
                a.cur_text = "completed";
            }
        }
        self.recompute_leader();
    }

    /// Recompute which non-stopped agent leads the race (highest progress).
    pub fn recompute_leader(&mut self) {
        let mut best: Option<usize> = None;
        let mut best_race = f32::NEG_INFINITY;
        for (i, a) in self.agents.iter().enumerate() {
            if a.status == RunStatus::Stopped {
                continue;
            }
            if a.race > best_race {
                best_race = a.race;
                best = Some(i);
            }
        }
        for (i, a) in self.agents.iter_mut().enumerate() {
            a.leading = Some(i) == best;
        }
    }

    /// Total USD spent across all agents in the current race.
    pub fn mission_spend(&self) -> f64 {
        self.agents.iter().map(|a| a.cost).sum()
    }

    /// The mission budget ceiling: per-agent cap × number of agents.
    pub fn mission_cap(&self) -> f64 {
        self.agents.len() as f64 * COST_CAP
    }

    /// One-line race status summary for the header.
    pub fn status_summary(&self) -> String {
        let mut running = 0;
        let mut finished = 0;
        let mut stopped = 0;
        for a in &self.agents {
            match a.status {
                RunStatus::Running => running += 1,
                RunStatus::Finished => finished += 1,
                RunStatus::Stopped => stopped += 1,
            }
        }
        format!("{running} running · {finished} finished · {stopped} stopped")
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Root {
        Root::headless()
    }

    #[test]
    fn default_settings_match_design() {
        let s = Settings::default();
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.accent_idx, 0);
        assert!(s.scrub);
        assert!(!s.telemetry);
        assert!(s.auto_cleanup);
        assert!(!s.reduce_motion);
    }

    #[test]
    fn auto_theme_resolves_to_dark() {
        let auto = Settings { theme: ThemeChoice::Auto, ..Default::default() };
        assert_eq!(auto.mode(), Mode::Dark);
        let light = Settings { theme: ThemeChoice::Light, ..Default::default() };
        assert_eq!(light.mode(), Mode::Light);
    }

    #[test]
    fn set_accent_clamps_and_updates_theme() {
        let mut r = root();
        r.apply(Msg::SetAccent(2));
        assert_eq!(r.settings.accent_idx, 2);
        assert_eq!(r.theme().accent.base, ACCENTS[2].base);
        r.apply(Msg::SetAccent(99));
        assert_eq!(r.settings.accent_idx, ACCENTS.len() - 1);
    }

    #[test]
    fn toggles_flip() {
        let mut r = root();
        assert!(!r.settings.reduce_motion);
        r.apply(Msg::ToggleReduceMotion);
        assert!(r.settings.reduce_motion);
        r.apply(Msg::ToggleScrub);
        assert!(!r.settings.scrub);
    }

    #[test]
    fn planned_adapter_cannot_be_enabled() {
        let mut r = root();
        let cursor = r.adapters.iter().position(|a| a.tag == "planned").unwrap();
        assert!(!r.adapters[cursor].enabled);
        r.apply(Msg::ToggleAdapter(cursor));
        assert!(!r.adapters[cursor].enabled, "planned adapters stay off");
    }

    #[test]
    fn available_adapter_toggles() {
        let mut r = root();
        let aider = r.adapters.iter().position(|a| a.tag == "available").unwrap();
        r.apply(Msg::ToggleAdapter(aider));
        assert!(r.adapters[aider].enabled);
        r.apply(Msg::ToggleAdapter(aider));
        assert!(!r.adapters[aider].enabled);
    }

    #[test]
    fn start_race_uses_enabled_adapters_and_plays() {
        let mut r = root();
        // Disable one of the four enabled adapters → race has three agents.
        let codex = r.adapters.iter().position(|a| a.cli == "codex").unwrap();
        r.apply(Msg::ToggleAdapter(codex));
        r.apply(Msg::StartRace);
        assert_eq!(r.screen, Screen::Mission);
        assert!(r.playing);
        assert_eq!(r.agents.len(), 3);
        assert!(r.agents.iter().all(|a| a.status == RunStatus::Running));
    }

    #[test]
    fn tick_advances_running_agents_only_when_playing() {
        let mut r = root();
        r.playing = false;
        let before: Vec<u32> = r.agents.iter().map(|a| a.tokens).collect();
        r.tick();
        let after: Vec<u32> = r.agents.iter().map(|a| a.tokens).collect();
        assert_eq!(before, after, "paused tick is a no-op");

        r.playing = true;
        r.tick();
        let running = r.agents.iter().find(|a| a.status == RunStatus::Running).unwrap();
        assert!(running.tokens > 0);
    }

    #[test]
    fn reduce_motion_pauses_tick() {
        let mut r = root();
        r.playing = true;
        r.settings.reduce_motion = true;
        let before: Vec<u32> = r.agents.iter().map(|a| a.tokens).collect();
        r.tick();
        let after: Vec<u32> = r.agents.iter().map(|a| a.tokens).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn tick_hard_stops_at_token_cap() {
        let mut r = root();
        r.playing = true;
        // Drive many ticks; every initially-running agent must hit a terminal cap.
        for _ in 0..1000 {
            r.tick();
        }
        assert!(r.agents.iter().all(|a| a.status != RunStatus::Running || a.race >= 0.98));
        // amp starts at the token cap → stopped quickly.
        let amp = r.agents.iter().find(|a| a.cli == "amp");
        if let Some(amp) = amp {
            assert_eq!(amp.status, RunStatus::Stopped);
        }
    }

    #[test]
    fn promote_finishes_winner_and_stops_rest() {
        let mut r = root();
        r.apply(Msg::Promote(0));
        assert_eq!(r.agents[0].status, RunStatus::Finished);
        assert_eq!(r.agents[0].race, 1.0);
        assert!(!r.playing);
        for a in r.agents.iter().skip(1) {
            assert_ne!(a.status, RunStatus::Running);
        }
    }

    #[test]
    fn leader_is_highest_progress_non_stopped() {
        let mut r = root();
        r.recompute_leader();
        let leader = r.agents.iter().position(|a| a.leading).unwrap();
        let leader_race = r.agents[leader].race;
        for a in &r.agents {
            if a.status != RunStatus::Stopped {
                assert!(a.race <= leader_race + f32::EPSILON);
            }
        }
        assert_eq!(r.agents.iter().filter(|a| a.leading).count(), 1);
    }

    #[test]
    fn open_timeline_selects_and_navigates() {
        let mut r = root();
        r.apply(Msg::OpenTimeline(2));
        assert_eq!(r.selected, 2);
        assert_eq!(r.screen, Screen::Timeline);
    }

    #[test]
    fn mission_spend_sums_costs() {
        let r = root();
        let expected: f64 = r.agents.iter().map(|a| a.cost).sum();
        assert!((r.mission_spend() - expected).abs() < 1e-9);
        assert!((r.mission_cap() - r.agents.len() as f64 * COST_CAP).abs() < 1e-9);
    }

    #[test]
    fn status_summary_counts_states() {
        let r = root();
        // sample(): 2 running, 1 finished, 1 stopped.
        assert_eq!(r.status_summary(), "2 running · 1 finished · 1 stopped");
    }
}

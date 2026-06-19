//! Application state and the update path.
//!
//! [`Settings`] plus the **real** run state live on the `Root` view (defined in
//! `main.rs`). Simple, pure state transitions funnel through a single [`Msg`]
//! enum applied by [`crate::Root::apply`]; side-effecting actions that touch the
//! engine (launching a run, editing the task field) have dedicated handlers in
//! `main.rs`/`launcher.rs` because they spawn background work.
//!
//! There is no simulation here: [`AgentRun`] rows are built from a real
//! [`crate::backend::LiveReport`] produced by the orchestrator.

use gpui::{App, ClickEvent, Context, Window};
use serde::{Deserialize, Serialize};

use crate::Root;
use crate::Screen;
use crate::backend::{LiveAttempt, LiveReport};
use crate::theme::{ACCENTS, Accent, Mode, Rgb, Theme};

// ── settings ──────────────────────────────────────────────────────────────────

/// Theme selection (Auto resolves to Dark — no system signal is available).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    Dark,
    Light,
    Auto,
}

/// Row-height / padding density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    Compact,
    Comfortable,
    Spacious,
}

/// UI font family choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontChoice {
    Geist,
    IbmPlex,
    System,
}

/// User preferences, applied live across the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

// ── run lifecycle ───────────────────────────────────────────────────────────

/// The lifecycle of the current mission run — strictly engine-driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No run launched yet this session.
    Idle,
    /// A real engine run is in flight on a background thread.
    Running,
    /// A run completed; [`Root::report`] holds the real result.
    Done,
    /// A run could not be assembled or started (see [`Root::run_note`]).
    Failed,
}

/// Per-attempt verdict status, derived from the advisor's real verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// The advisor verified this attempt as acceptable.
    Passed,
    /// The advisor rejected this attempt (or the advisor was unreachable).
    Failed,
}

/// One rendered run row — a single real execution attempt the orchestrator made.
/// Built from a [`LiveAttempt`]; no simulated fields.
#[derive(Debug, Clone)]
pub struct AgentRun {
    pub framework: String,
    pub model: String,
    pub subtask: String,
    pub color: Rgb,
    /// This attempt won (was selected for) its subtask.
    pub won: bool,
    pub status: RunStatus,
    /// Advisor quality score in `0.0..=1.0`, drives the progress bar.
    pub score: f64,
    /// 0 = cheapest tier candidate tried first.
    pub tier_rank: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    /// The winning attempt's response text (empty for non-winners).
    pub response: String,
    /// Real worktree diff stats for the winner (0 for non-winners).
    pub files_changed: usize,
    pub added: usize,
    pub removed: usize,
    /// The worktree session id (used to promote/clean up this attempt).
    pub worktree_session: String,
}

/// Deterministic accent color per framework, so the same framework reads the same
/// across the board.
pub fn framework_color(framework: &str) -> Rgb {
    match framework {
        "claude-code" => 0xC08CFF,
        "codex" => 0x4ED99A,
        "gemini-cli" => 0x7FA8FF,
        "aider" => 0xFFB454,
        "cursor" => 0x6AD6E0,
        _ => 0x8B8B95,
    }
}

impl AgentRun {
    /// Build a row from one real orchestrator attempt.
    pub fn from_attempt(a: &LiveAttempt) -> Self {
        Self {
            framework: a.framework.clone(),
            model: a.model.clone(),
            subtask: a.subtask.clone(),
            color: framework_color(&a.framework),
            won: a.won,
            status: if a.passed {
                RunStatus::Passed
            } else {
                RunStatus::Failed
            },
            score: a.score,
            tier_rank: a.tier_rank,
            input_tokens: a.input_tokens,
            output_tokens: a.output_tokens,
            cost: a.cost_usd,
            response: a.response.clone(),
            files_changed: a.files_changed,
            added: a.added,
            removed: a.removed,
            worktree_session: a.worktree_session.clone(),
        }
    }

    /// Build the full set of rows for a report, in orchestrator (cascade) order.
    pub fn rows(report: &LiveReport) -> Vec<AgentRun> {
        report.attempts.iter().map(AgentRun::from_attempt).collect()
    }
}

// ── messages ──────────────────────────────────────────────────────────────────

/// Every *pure* state transition the UI can request (no I/O). Side-effecting
/// actions (launch a run, edit the task) have their own handlers.
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
    SelectAgent(usize),
    /// Open a specific run row's timeline.
    OpenTimeline(usize),
    /// Mark a run row as the promoted winner.
    Promote(usize),
    /// Choose the local advisor model from [`ADVISOR_MODELS`].
    SetAdvisorModel(usize),
    /// Choose where pricing + benchmark data comes from.
    SetCatalogSource(CatalogSource),
}

/// Where the model catalog's pricing + benchmark data is sourced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogSource {
    /// OpenRouter (pricing) + the public Aider leaderboard (benchmarks). No key.
    Keyless,
    /// Artificial Analysis — pricing + benchmarks in one API (needs a free key).
    ArtificialAnalysis,
}

impl CatalogSource {
    /// Default to Artificial Analysis when an API key is present (richer data),
    /// else the keyless OpenRouter + Aider combo.
    pub fn default_from_env() -> Self {
        if std::env::var("ARTIFICIALANALYSIS_API_KEY").is_ok_and(|k| !k.is_empty()) {
            CatalogSource::ArtificialAnalysis
        } else {
            CatalogSource::Keyless
        }
    }
}

/// Local advisor models the user can pick in Settings (deterministic + reasoning).
pub const ADVISOR_MODELS: [&str; 5] = [
    "qwen2.5-coder:7b",
    "deepseek-r1:7b",
    "qwq",
    "llama3.2:3b",
    "qwen2.5-coder:1.5b",
];

/// User-chosen connection to the local advisor model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorPrefs {
    /// OpenAI-compatible endpoint base URL (Ollama / llamafile / llama.cpp).
    pub endpoint: String,
    /// The selected model name.
    pub model: String,
    /// Last readiness-check result, shown in Settings. Transient — never persisted.
    #[serde(skip)]
    pub status: Option<String>,
}

impl AdvisorPrefs {
    /// Read defaults from the environment (`ORYN_ADVISOR_ENDPOINT`,
    /// `ORYN_ADVISOR_MODEL`), falling back to a local Ollama with the default model.
    pub fn from_env() -> Self {
        let endpoint = std::env::var("ORYN_ADVISOR_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:11434".into());
        let model =
            std::env::var("ORYN_ADVISOR_MODEL").unwrap_or_else(|_| ADVISOR_MODELS[0].into());
        Self {
            endpoint,
            model,
            status: None,
        }
    }
}

// ── persisted configuration ───────────────────────────────────────────────────

/// The slice of state that survives across launches, written to disk as JSON.
/// Run state, focus, and transient status are deliberately excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedConfig {
    pub settings: Settings,
    pub advisor: AdvisorPrefs,
    pub catalog_source: CatalogSource,
}

impl Msg {
    /// Whether applying this message changes persisted configuration (so the UI
    /// should write it to disk). Navigation, selection, and run actions don't.
    pub fn persists(&self) -> bool {
        matches!(
            self,
            Msg::SetTheme(_)
                | Msg::SetAccent(_)
                | Msg::SetDensity(_)
                | Msg::SetFont(_)
                | Msg::ToggleReduceMotion
                | Msg::ToggleScrub
                | Msg::ToggleTelemetry
                | Msg::ToggleAutoCleanup
                | Msg::SetAdvisorModel(_)
                | Msg::SetCatalogSource(_)
        )
    }
}

impl Root {
    /// Apply a pure [`Msg`], mutating state. No I/O.
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
            Msg::SelectAgent(i) => self.select_agent(i),
            Msg::OpenTimeline(i) => {
                self.select_agent(i);
                self.screen = Screen::Timeline;
            }
            Msg::Promote(i) => self.promote(i),
            Msg::SetAdvisorModel(i) => {
                if let Some(m) = ADVISOR_MODELS.get(i) {
                    self.advisor.model = (*m).to_string();
                    self.advisor.status = None;
                }
            }
            Msg::SetCatalogSource(s) => {
                self.catalog_source = s;
                self.source_status = None;
            }
        }
    }

    /// The live-resolved theme.
    pub fn theme(&self) -> Theme {
        Theme::resolve(self.settings.mode(), self.settings.accent())
    }

    /// Snapshot the persisted slice of state.
    pub fn to_config(&self) -> PersistedConfig {
        PersistedConfig {
            settings: self.settings,
            advisor: self.advisor.clone(),
            catalog_source: self.catalog_source,
        }
    }

    /// Apply a loaded config over the defaults (called once at startup).
    pub fn apply_config(&mut self, cfg: PersistedConfig) {
        self.settings = cfg.settings;
        self.advisor = cfg.advisor;
        self.catalog_source = cfg.catalog_source;
    }

    /// Build a click handler that applies `msg`, persists config if the message
    /// changes it, and requests a re-render.
    pub fn on(
        &self,
        cx: &mut Context<Self>,
        msg: Msg,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _e: &ClickEvent, _w, cx| {
            this.apply(msg);
            if msg.persists() {
                crate::backend::save_config(&this.to_config());
            }
            cx.notify();
        })
    }

    /// Toggle an adapter's selected state (planned adapters cannot be enabled).
    pub fn toggle_adapter(&mut self, i: usize) {
        if let Some(a) = self.adapters.get_mut(i)
            && a.tag != "planned"
        {
            a.enabled = !a.enabled;
        }
    }

    /// Select a run row for the Timeline / Review detail views.
    pub fn select_agent(&mut self, i: usize) {
        if i < self.agents.len() {
            self.selected = i;
        }
    }

    /// Mark run row `i` as the promoted winner.
    pub fn promote(&mut self, i: usize) {
        if i < self.agents.len() {
            self.promoted = Some(i);
            self.selected = i;
        }
    }

    /// Ingest a finished engine run: store the report, build real rows, and
    /// preselect the overall winner.
    pub fn ingest_report(&mut self, report: LiveReport) {
        self.agents = AgentRun::rows(&report);
        self.run_note = report.note.clone();
        self.selected = self
            .agents
            .iter()
            .position(|a| a.won && a.status == RunStatus::Passed)
            .unwrap_or(0);
        self.promoted = None;
        self.phase = if self.agents.is_empty() {
            Phase::Failed
        } else {
            Phase::Done
        };
        self.report = Some(report);
    }

    /// Total real USD spent across the last run (0 before any run).
    pub fn mission_spend(&self) -> f64 {
        self.report.as_ref().map(|r| r.gross_usd).unwrap_or(0.0)
    }

    /// One-line status summary for the headers, derived from real state.
    pub fn status_summary(&self) -> String {
        match self.phase {
            Phase::Idle => "no run yet — launch one".into(),
            Phase::Running => "running — orchestrator routing subtasks".into(),
            Phase::Failed => self.run_note.clone(),
            Phase::Done => {
                let passed = self
                    .agents
                    .iter()
                    .filter(|a| a.won && a.status == RunStatus::Passed)
                    .count();
                let won = self.agents.iter().filter(|a| a.won).count();
                format!(
                    "{} attempt(s) · {won} winner(s) · {passed} verified",
                    self.agents.len()
                )
            }
        }
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn report(attempts: Vec<LiveAttempt>) -> LiveReport {
        let gross = attempts.iter().map(|a| a.cost_usd).sum();
        LiveReport {
            goal: "g".into(),
            repo_label: "acme/web".into(),
            base_ref: "main@abc1234".into(),
            advisor: "qwen @ x".into(),
            discovered: 2,
            subtasks: 1,
            attempts,
            gross_usd: gross,
            saved_usd: 0.0,
            note: "done".into(),
            store_artifacts: 0,
            store_bytes: 0,
            ctx_offered_bytes: 0,
            ctx_stored_bytes: 0,
        }
    }

    fn attempt(framework: &str, won: bool, passed: bool, cost: f64) -> LiveAttempt {
        LiveAttempt {
            subtask: "implement".into(),
            framework: framework.into(),
            model: "m".into(),
            tier_rank: 0,
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: cost,
            passed,
            score: if passed { 0.9 } else { 0.2 },
            won,
            response: if won {
                "did the thing".into()
            } else {
                String::new()
            },
            files_changed: if won { 2 } else { 0 },
            added: if won { 10 } else { 0 },
            removed: if won { 3 } else { 0 },
            worktree_session: format!("oryn-{framework}-m"),
        }
    }

    #[test]
    fn default_settings_match_design() {
        let s = Settings::default();
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert!(s.scrub && s.auto_cleanup && !s.telemetry && !s.reduce_motion);
    }

    #[test]
    fn from_attempt_maps_real_fields() {
        let row = AgentRun::from_attempt(&attempt("claude-code", true, true, 1.5));
        assert_eq!(row.framework, "claude-code");
        assert_eq!(row.color, framework_color("claude-code"));
        assert!(row.won && row.status == RunStatus::Passed);
        assert_eq!(row.response, "did the thing");
    }

    #[test]
    fn ingest_report_builds_rows_and_selects_winner() {
        let mut r = Root::headless();
        assert_eq!(r.phase, Phase::Idle);
        r.ingest_report(report(vec![
            attempt("codex", false, false, 0.4),
            attempt("claude-code", true, true, 1.5),
        ]));
        assert_eq!(r.phase, Phase::Done);
        assert_eq!(r.agents.len(), 2);
        assert_eq!(r.selected, 1, "winner preselected");
        assert!((r.mission_spend() - 1.9).abs() < 1e-9);
    }

    #[test]
    fn empty_report_is_failed_phase() {
        let mut r = Root::headless();
        r.ingest_report(report(vec![]));
        assert_eq!(r.phase, Phase::Failed);
    }

    #[test]
    fn promote_marks_winner() {
        let mut r = Root::headless();
        r.ingest_report(report(vec![attempt("codex", true, true, 0.4)]));
        r.apply(Msg::Promote(0));
        assert_eq!(r.promoted, Some(0));
    }

    #[test]
    fn planned_adapter_cannot_be_enabled() {
        let mut r = Root::headless();
        let cursor = r.adapters.iter().position(|a| a.tag == "planned").unwrap();
        r.apply(Msg::ToggleAdapter(cursor));
        assert!(!r.adapters[cursor].enabled);
    }

    #[test]
    fn set_accent_clamps() {
        let mut r = Root::headless();
        r.apply(Msg::SetAccent(99));
        assert_eq!(r.settings.accent_idx, ACCENTS.len() - 1);
    }

    #[test]
    fn idle_status_summary() {
        let r = Root::headless();
        assert_eq!(r.status_summary(), "no run yet — launch one");
    }

    #[test]
    fn config_snapshot_and_apply_round_trip() {
        let mut r = Root::headless();
        r.settings.accent_idx = 3;
        r.advisor.model = "qwq".into();
        r.catalog_source = CatalogSource::ArtificialAnalysis;
        let cfg = r.to_config();

        let mut other = Root::headless();
        other.apply_config(cfg.clone());
        assert_eq!(other.settings.accent_idx, 3);
        assert_eq!(other.advisor.model, "qwq");
        assert_eq!(other.catalog_source, CatalogSource::ArtificialAnalysis);
        assert_eq!(other.to_config(), cfg);
    }

    #[test]
    fn only_config_messages_persist() {
        assert!(Msg::SetTheme(ThemeChoice::Light).persists());
        assert!(Msg::ToggleTelemetry.persists());
        assert!(Msg::SetCatalogSource(CatalogSource::Keyless).persists());
        assert!(!Msg::Navigate(Screen::Mission).persists());
        assert!(!Msg::SelectAgent(0).persists());
        assert!(!Msg::Promote(0).persists());
    }
}

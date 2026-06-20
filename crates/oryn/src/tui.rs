//! `oryn tui` — a terminal dashboard to read and visualize project state:
//! selection, the green cache, crate fingerprints, and flaky-test statistics.

use anyhow::Result;
use oryn_core::dashboard::{CrateStatus, Dashboard};
use oryn_core::flaky::FlakyVerdict;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, TableState, Tabs},
    Frame,
};
use std::time::Duration;

use crate::runner;

const TABS: [&str; 5] = ["Overview", "Selection", "Crates", "Flaky", "Help"];

/// TUI application state.
pub struct App {
    dash: Dashboard,
    since: Option<String>,
    level: f64,
    tab: usize,
    row: usize,
    status: String,
    quit: bool,
    /// Set when the user presses `t`; the run loop leaves the TUI, runs
    /// `oryn test`, then re-enters and refreshes.
    run_requested: bool,
}

impl App {
    /// Construct from a prebuilt dashboard.
    #[must_use]
    pub fn new(dash: Dashboard, since: Option<String>, level: f64) -> Self {
        Self {
            dash,
            since,
            level,
            tab: 0,
            row: 0,
            status: "ready".into(),
            quit: false,
            run_requested: false,
        }
    }

    fn list_len(&self) -> usize {
        match self.tab {
            1 | 2 => self.dash.crates.len(),
            3 => self
                .dash
                .flaky
                .tests
                .iter()
                .filter(|t| {
                    t.verdict != FlakyVerdict::StablePass && t.verdict != FlakyVerdict::Unknown
                })
                .count(),
            _ => 0,
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                self.tab = (self.tab + 1) % TABS.len();
                self.row = 0;
            }
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => {
                self.tab = (self.tab + TABS.len() - 1) % TABS.len();
                self.row = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = self.list_len();
                if n > 0 {
                    self.row = (self.row + 1) % n;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let n = self.list_len();
                if n > 0 {
                    self.row = (self.row + n - 1) % n;
                }
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('t') => self.run_requested = true,
            KeyCode::Char(c @ '1'..='5') => {
                self.tab = (c as usize) - ('1' as usize);
                self.row = 0;
            }
            _ => {}
        }
    }

    fn refresh(&mut self) {
        match runner::collect_dashboard(self.since.as_deref(), self.level) {
            Ok(d) => {
                self.dash = d;
                self.status = "refreshed".into();
            }
            Err(e) => self.status = format!("refresh failed: {e}"),
        }
    }
}

/// Run the TUI until the user quits.
///
/// # Errors
/// Propagates terminal or data-collection errors.
pub fn run(since: Option<&str>, level: f64) -> Result<()> {
    let dash = runner::collect_dashboard(since, level)?;
    let mut app = App::new(dash, since.map(str::to_string), level);
    let mut term = ratatui::init();
    let result = loop {
        if let Err(e) = term.draw(|f| ui(f, &app)) {
            break Err(e.into());
        }
        if app.quit {
            break Ok(());
        }
        match event::poll(Duration::from_millis(250)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => app.on_key(k.code),
                Ok(_) => {}
                Err(e) => break Err(e.into()),
            },
            Ok(false) => {}
            Err(e) => break Err(e.into()),
        }
        if app.run_requested {
            app.run_requested = false;
            // Leave the alternate screen, run `oryn test` with the terminal
            // handed back to it, then re-enter and refresh the dashboard.
            ratatui::restore();
            run_tests_interactive(app.since.as_deref());
            term = ratatui::init();
            app.refresh();
        }
    };
    ratatui::restore();
    result
}

/// Run `oryn test` for the current selection as a child process with the real
/// terminal, then wait for a keypress before returning to the dashboard. Runs
/// the binary itself so the child owns its own exit code (it never kills the TUI).
fn run_tests_interactive(since: Option<&str>) {
    use std::io::Write;
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("oryn"));
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("test");
    if let Some(s) = since {
        cmd.args(["--since", s]);
    }
    println!("\n── oryn test ──\n");
    let _ = cmd.status();
    print!("\n[tests finished — press Enter to return to the dashboard] ");
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}

/// Render one full frame (separated for `TestBackend` unit tests).
pub fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    let titles: Vec<Line> = TABS
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!(" {}·{} ", i + 1, t)))
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.tab)
        .block(Block::default().borders(Borders::ALL).title(" Oryn "))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    match app.tab {
        0 => overview(f, chunks[1], app),
        1 => selection(f, chunks[1], app),
        2 => crates(f, chunks[1], app),
        3 => flaky(f, chunks[1], app),
        _ => help(f, chunks[1]),
    }

    let footer = Line::from(vec![
        Span::styled(" q", Style::default().fg(Color::Cyan)),
        Span::raw(" quit  "),
        Span::styled("←/→", Style::default().fg(Color::Cyan)),
        Span::raw(" tabs  "),
        Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
        Span::raw(" move  "),
        Span::styled("r", Style::default().fg(Color::Cyan)),
        Span::raw(" refresh  "),
        Span::styled("t", Style::default().fg(Color::Cyan)),
        Span::raw(format!(" run tests   [{}]", app.status)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
}

fn overview(f: &mut Frame, area: Rect, app: &App) {
    let d = &app.dash;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area);

    let lines = vec![
        Line::from(vec![
            Span::styled("workspace  ", Style::default().fg(Color::DarkGray)),
            Span::raw(&d.workspace_root),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{:>4}", d.crate_count),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" crates   "),
            Span::styled(
                format!("{:>4}", d.affected_count),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" affected by your change   "),
            Span::styled(
                format!("{:>4}", d.cached_count),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" cached green   "),
            Span::styled(
                format!("{:>4}", d.skipped_count),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(" skipped"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("selection: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&d.plan_reason),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{:>4}", d.flaky.tests.len()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" tests in history   "),
            Span::styled(
                format!("{:>4}", d.flaky.flaky_count),
                Style::default().fg(Color::Magenta),
            ),
            Span::raw(" flaky   "),
            Span::styled(
                format!("{:>4}", d.flaky.always_fail_count),
                Style::default().fg(Color::Red),
            ),
            Span::raw(" always-failing"),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).block(block("Overview")), rows[0]);

    // Cache-hit gauge over the affected set.
    let ratio = if d.affected_count == 0 {
        1.0
    } else {
        d.cached_count as f64 / d.affected_count as f64
    };
    let gauge = Gauge::default()
        .block(block("cache hit on affected crates"))
        .gauge_style(Style::default().fg(Color::Green))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(format!("{:.0}% skippable", ratio * 100.0));
    f.render_widget(gauge, rows[1]);
}

fn status_cell(s: CrateStatus) -> Cell<'static> {
    match s {
        CrateStatus::Changed => Cell::from("CHANGED").style(Style::default().fg(Color::Yellow)),
        CrateStatus::Affected => Cell::from("affected").style(Style::default().fg(Color::Cyan)),
        CrateStatus::Skipped => Cell::from("skipped").style(Style::default().fg(Color::DarkGray)),
    }
}

fn selection(f: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Row> = app
        .dash
        .crates
        .iter()
        .map(|c| {
            let green = if c.cached_green {
                Cell::from("green ✓").style(Style::default().fg(Color::Green))
            } else {
                Cell::from("—").style(Style::default().fg(Color::DarkGray))
            };
            Row::new(vec![
                Cell::from(c.name.clone()),
                status_cell(c.status),
                green,
                Cell::from(c.short_fp.clone()),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(14),
        ],
    )
    .header(
        Row::new(vec!["crate", "status", "cache", "fingerprint"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 60)))
    .block(block("Selection — what your change affects"));
    f.render_stateful_widget(table, area, &mut selected_state(app.row));
}

/// A fresh `TableState` selecting `row`; ratatui scrolls the viewport so the
/// selected row stays visible (the fix for workspaces taller than the screen).
fn selected_state(row: usize) -> TableState {
    TableState::default().with_selected(Some(row))
}

fn crates(f: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Row> = app
        .dash
        .crates
        .iter()
        .map(|c| {
            Row::new(vec![
                Cell::from(c.name.clone()),
                Cell::from(c.short_fp.clone()),
                Cell::from(if c.cached_green { "green" } else { "stale" }),
                Cell::from(c.tests.to_string()),
                Cell::from(format!("{} ms", c.total_ms)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(14),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["crate", "fingerprint", "cache", "tests", "time"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 60)))
    .block(block("Crates"));
    f.render_stateful_widget(table, area, &mut selected_state(app.row));
}

/// A 10-cell unicode bar for a 0..1 value.
fn bar(x: f64) -> String {
    let filled = (x.clamp(0.0, 1.0) * 10.0).round() as usize;
    let mut s = String::new();
    for i in 0..10 {
        s.push(if i < filled { '█' } else { '░' });
    }
    s
}

fn flaky(f: &mut Frame, area: Rect, app: &App) {
    let interesting: Vec<_> = app
        .dash
        .flaky
        .tests
        .iter()
        .filter(|t| t.verdict == FlakyVerdict::Flaky || t.verdict == FlakyVerdict::StableFail)
        .collect();

    if interesting.is_empty() {
        let p = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No flaky or failing tests in history ✓",
                Style::default().fg(Color::Green),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Run `oryn setup` then `oryn test` to collect per-test data.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(block("Flaky tests"));
        f.render_widget(p, area);
        return;
    }

    let rows: Vec<Row> = interesting
        .iter()
        .map(|t| {
            let (tag, color) = match t.verdict {
                FlakyVerdict::Flaky => ("FLAKY", Color::Magenta),
                _ => ("FAIL", Color::Red),
            };
            Row::new(vec![
                Cell::from(tag).style(Style::default().fg(color)),
                Cell::from(t.id.clone()),
                Cell::from(format!("{:>5.1}%", t.flake_rate * 100.0)),
                Cell::from(bar(t.flake_rate)).style(Style::default().fg(color)),
                Cell::from(format!("{:.0}-{:.0}%", t.ci.low * 100.0, t.ci.high * 100.0)),
                Cell::from(format!(
                    "{:.0}-{:.0}%",
                    t.posterior.low * 100.0,
                    t.posterior.high * 100.0
                )),
                Cell::from(
                    t.reruns_to_reproduce_95
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".into()),
                ),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Min(16),
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new(vec![
            "", "test", "rate", "rate bar", "Wilson", "Bayes", "reruns",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 60)))
    .block(block(
        "Flaky tests — frequentist (Wilson) + Bayesian intervals, rerun budget",
    ));
    f.render_stateful_widget(table, area, &mut selected_state(app.row));
}

fn help(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "  Oryn — compile less, test less, trust the results",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  1–5 / ←/→ / Tab    switch tabs"),
        Line::from("  ↑/↓ (or j/k)       move selection"),
        Line::from("  r                  refresh (recompute selection, fingerprints, stats)"),
        Line::from("  t                  run `oryn test` for the selection, then return"),
        Line::from("  q / Esc            quit"),
        Line::from(""),
        Line::from(Span::styled("  Tabs", Style::default().fg(Color::Cyan))),
        Line::from("  Overview   counts, selection reason, cache-hit gauge"),
        Line::from("  Selection  what the current git diff affects (changed/affected/skipped)"),
        Line::from("  Crates     fingerprints, cache state, recorded test counts & time"),
        Line::from("  Flaky      Wilson + Bayesian flake intervals and rerun budgets"),
    ];
    f.render_widget(Paragraph::new(lines).block(block("Help")), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use oryn_core::dashboard::CrateView;
    use oryn_core::flaky::{self, TestRuns};
    use ratatui::{backend::TestBackend, Terminal};

    fn sample() -> Dashboard {
        let flaky = flaky::analyze(
            &[
                TestRuns::new("core::net::flaky", 95, 5),
                TestRuns::new("core::ok", 100, 0),
            ],
            0.95,
        );
        Dashboard {
            workspace_root: "/ws".into(),
            crate_count: 2,
            affected_count: 2,
            cached_count: 1,
            skipped_count: 0,
            plan_reason: "core changed; testing it + 1 dependent".into(),
            crates: vec![
                CrateView {
                    name: "core".into(),
                    short_fp: "abc123def456".into(),
                    cached_green: false,
                    status: CrateStatus::Changed,
                    tests: 2,
                    total_ms: 30,
                },
                CrateView {
                    name: "cli".into(),
                    short_fp: "999888777666".into(),
                    cached_green: true,
                    status: CrateStatus::Affected,
                    tests: 0,
                    total_ms: 0,
                },
            ],
            flaky,
        }
    }

    fn render_tab(tab: usize) -> String {
        let mut app = App::new(sample(), None, 0.95);
        app.tab = tab;
        let backend = TestBackend::new(120, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui(f, &app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }

    #[test]
    fn every_tab_renders_without_panicking() {
        for tab in 0..TABS.len() {
            let s = render_tab(tab);
            assert!(!s.trim().is_empty(), "tab {tab} rendered empty");
        }
    }

    #[test]
    fn overview_shows_counts_and_root() {
        let s = render_tab(0);
        assert!(s.contains("crates"));
        assert!(s.contains("/ws"));
    }

    #[test]
    fn selection_lists_crates_with_status() {
        let s = render_tab(1);
        assert!(s.contains("core"));
        assert!(s.contains("CHANGED"));
    }

    #[test]
    fn flaky_tab_shows_flaky_test() {
        let s = render_tab(3);
        assert!(s.contains("FLAKY"));
        assert!(s.contains("net::flaky"));
    }

    #[test]
    fn navigation_wraps_tabs_and_rows() {
        let mut app = App::new(sample(), None, 0.95);
        app.on_key(KeyCode::Left); // wraps to last tab
        assert_eq!(app.tab, TABS.len() - 1);
        app.on_key(KeyCode::Char('2')); // jump to Selection
        assert_eq!(app.tab, 1);
        app.on_key(KeyCode::Down);
        assert_eq!(app.row, 1);
        app.on_key(KeyCode::Down); // wraps (2 crates)
        assert_eq!(app.row, 0);
    }

    #[test]
    fn t_requests_a_test_run() {
        let mut app = App::new(sample(), None, 0.95);
        assert!(!app.run_requested);
        app.on_key(KeyCode::Char('t'));
        assert!(app.run_requested);
    }

    #[test]
    fn selection_scrolls_to_keep_far_row_visible() {
        // 60 crates, 30-row terminal: selecting row 55 must still render it.
        let mut dash = sample();
        dash.crates = (0..60)
            .map(|i| CrateView {
                name: format!("crate{i:02}"),
                short_fp: "fp".into(),
                cached_green: false,
                status: CrateStatus::Affected,
                tests: 0,
                total_ms: 0,
            })
            .collect();
        let mut app = App::new(dash, None, 0.95);
        app.tab = 1;
        app.row = 55;
        let backend = TestBackend::new(120, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui(f, &app)).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            s.contains("crate55"),
            "viewport did not scroll to the selection"
        );
    }
}

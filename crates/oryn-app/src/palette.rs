//! Command palette — a real, keyboard-driven overlay.
//!
//! Opened from the top-bar search box (or the ⌘K affordance), it filters a list
//! of real commands (navigate to any screen, launch/cancel a run) by substring,
//! supports up/down selection and Enter-to-run, and Esc to close. The query field
//! is a genuine editor (typing/backspace), reusing the same focus + key plumbing
//! as the task editor.

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, FontWeight, KeyDownEvent, ParentElement, Styled, Window, div, px,
};

use crate::Root;
use crate::Screen;
use crate::colors::{overlay, solid, tint};
use crate::state::Phase;

/// What running a palette command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    Go(Screen),
    Launch,
    Cancel,
}

/// One palette command.
#[derive(Debug, Clone, Copy)]
pub struct Command {
    pub label: &'static str,
    pub hint: &'static str,
    pub action: PaletteAction,
}

/// The full command set, in display order. Always the same — filtering happens
/// against this.
pub fn all_commands() -> Vec<Command> {
    vec![
        Command {
            label: "Launch run",
            hint: "run the task across selected frameworks",
            action: PaletteAction::Launch,
        },
        Command {
            label: "Cancel run",
            hint: "stop the in-flight run",
            action: PaletteAction::Cancel,
        },
        Command {
            label: "Go to Mission Control",
            hint: "the cascade board",
            action: PaletteAction::Go(Screen::Mission),
        },
        Command {
            label: "Go to Timeline",
            hint: "faithful trace of an attempt",
            action: PaletteAction::Go(Screen::Timeline),
        },
        Command {
            label: "Go to Review",
            hint: "compare & promote",
            action: PaletteAction::Go(Screen::Review),
        },
        Command {
            label: "Go to Broker",
            hint: "shared context economics",
            action: PaletteAction::Go(Screen::Broker),
        },
        Command {
            label: "Go to Launch",
            hint: "configure a new run",
            action: PaletteAction::Go(Screen::Launch),
        },
        Command {
            label: "Go to Settings",
            hint: "preferences & data source",
            action: PaletteAction::Go(Screen::Settings),
        },
        Command {
            label: "Go to Profile",
            hint: "identity & workspace",
            action: PaletteAction::Go(Screen::Profile),
        },
    ]
}

/// Case-insensitive subsequence match (the usual fuzzy-palette feel): every char
/// of `query` appears in `label` in order.
pub fn matches(query: &str, label: &str) -> bool {
    let mut q = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .peekable();
    if q.peek().is_none() {
        return true;
    }
    let mut want = q.next();
    for c in label.chars().map(|c| c.to_ascii_lowercase()) {
        if Some(c) == want {
            want = q.next();
            if want.is_none() {
                return true;
            }
        }
    }
    false
}

/// Commands matching the current query, in order.
pub fn filtered(query: &str) -> Vec<Command> {
    all_commands()
        .into_iter()
        .filter(|c| matches(query, c.label))
        .collect()
}

impl Root {
    /// Open the palette (resetting query + selection) and focus its input.
    pub fn open_palette(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, _e: &gpui::ClickEvent, window: &mut Window, cx| {
            this.palette_open = true;
            this.palette_query.clear();
            this.palette_sel = 0;
            if let Some(fh) = this.palette_focus.clone() {
                window.focus(&fh);
            }
            cx.notify();
        })
    }

    /// Run the currently-highlighted command (or `action` directly).
    fn run_palette_action(&mut self, action: PaletteAction, cx: &mut Context<Self>) {
        self.palette_open = false;
        match action {
            PaletteAction::Go(s) => self.screen = s,
            PaletteAction::Cancel => self.cancel_run(),
            PaletteAction::Launch => {
                // Mirror launch_run's snapshot path on the current state.
                if self.phase != Phase::Running {
                    self.begin_run(cx);
                }
            }
        }
    }

    /// Key handler for the palette input: filter, navigate, run, close.
    pub fn palette_key(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, e: &KeyDownEvent, _w, cx| {
            let k = &e.keystroke;
            if k.modifiers.control || k.modifiers.platform || k.modifiers.function {
                return;
            }
            let n = filtered(&this.palette_query).len();
            match k.key.as_str() {
                "escape" => this.palette_open = false,
                "backspace" => {
                    this.palette_query.pop();
                    this.palette_sel = 0;
                }
                "up" => this.palette_sel = this.palette_sel.saturating_sub(1),
                "down" => {
                    if n > 0 {
                        this.palette_sel = (this.palette_sel + 1).min(n - 1);
                    }
                }
                "enter" => {
                    if let Some(cmd) = filtered(&this.palette_query).get(this.palette_sel).copied()
                    {
                        this.run_palette_action(cmd.action, cx);
                    }
                }
                "space" => {
                    this.palette_query.push(' ');
                    this.palette_sel = 0;
                }
                _ => {
                    if let Some(ch) = &k.key_char {
                        this.palette_query.push_str(ch);
                        this.palette_sel = 0;
                    } else if k.key.chars().count() == 1 {
                        let key = k.key.clone();
                        this.palette_query.push_str(&key);
                        this.palette_sel = 0;
                    } else {
                        return;
                    }
                }
            }
            cx.notify();
        })
    }

    /// The palette overlay element (rendered only when open).
    pub(crate) fn palette_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        let cmds = filtered(&self.palette_query);
        let sel = self.palette_sel.min(cmds.len().saturating_sub(1));

        let mut list = div().flex().flex_col().gap(px(2.0)).mt(px(8.0));
        if cmds.is_empty() {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(px(12.0))
                    .text_color(solid(t.text.t5))
                    .child("no matching commands"),
            );
        }
        for (i, cmd) in cmds.iter().enumerate() {
            let active = i == sel;
            let action = cmd.action;
            list = list.child(
                div()
                    .id(("palcmd", i))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(8.0))
                    .when(active, |d| d.bg(tint(t.accent.base, 0.13)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.run_palette_action(action, cx);
                        cx.notify();
                    }))
                    .child(div().size(px(7.0)).rounded(px(2.0)).bg(solid(if active {
                        t.accent.base
                    } else {
                        t.text.t6
                    })))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(solid(t.text.t1))
                                    .child(cmd.label),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(solid(t.text.t5))
                                    .child(cmd.hint),
                            ),
                    ),
            );
        }

        let mut input = div()
            .id("palette-input")
            .h(px(40.0))
            .flex()
            .items_center()
            .px(px(14.0))
            .rounded(px(9.0))
            .bg(overlay(t.overlays.w04))
            .border_1()
            .border_color(tint(t.accent.base, 0.3))
            .text_size(px(13.0))
            .text_color(solid(t.text.t1))
            .child(format!("{}\u{2502}", self.palette_query));
        if let Some(fh) = &self.palette_focus {
            input = input.track_focus(fh).on_key_down(self.palette_key(cx));
        }

        // Full-screen scrim that closes on click, with the panel on top.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .bg(tint(0x000000, 0.5))
            .child(
                div()
                    .id("palette-scrim")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.palette_open = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .mt(px(96.0))
                    .w(px(560.0))
                    .flex()
                    .flex_col()
                    .bg(solid(t.surfaces.panel))
                    .border_1()
                    .border_color(overlay(t.overlays.w12))
                    .rounded(px(13.0))
                    .p(px(12.0))
                    .child(input)
                    .child(list),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_subsequence_case_insensitive() {
        assert!(matches("", "anything"));
        assert!(matches("gtl", "Go to Launch"));
        assert!(matches("review", "Go to Review"));
        assert!(!matches("zzz", "Go to Mission Control"));
    }

    #[test]
    fn filtered_narrows_and_empty_query_returns_all() {
        assert_eq!(filtered("").len(), all_commands().len());
        let f = filtered("launch");
        assert!(!f.is_empty());
        assert!(f.iter().all(|c| matches("launch", c.label)));
    }

    #[test]
    fn cancel_only_acts_while_running() {
        let mut r = crate::Root::headless();
        r.cancel_run(); // no-op when idle
        assert_eq!(r.phase, Phase::Idle);
        r.phase = Phase::Running;
        r.cancel_run();
        assert_eq!(r.phase, Phase::Failed);
        assert_eq!(r.run_note, "run cancelled");
    }
}

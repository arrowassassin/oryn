//! Launcher view — configure and launch a real run.
//!
//! The repository panel reflects the **real** git repo the app was launched in;
//! the task field is genuinely editable (focus + key handling in `main.rs`); the
//! agent rows toggle which installed CLIs to route across; and "Launch" fires the
//! real engine run on a background thread (see [`crate::Root::launch_run`]). The
//! adapter list is the UI face of the orchestrator's `(framework, model)`
//! discovery — Oryn lists exactly the models each selected CLI reports.

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, ParentElement, Styled, div, px};

use crate::Root;
use crate::colors::{overlay, solid, tint};
use crate::state::Msg;
use crate::theme::{Rgb, Theme};

/// One agent framework the user can route across. `cli` is the binary Oryn runs
/// for live model discovery and execution.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: &'static str,
    pub cli: &'static str,
    pub color: Rgb,
    pub enabled: bool,
    /// Short status/auth hint shown on the row.
    pub tag: &'static str,
}

impl Adapter {
    /// The frameworks Oryn can drive, mapped to their real CLIs.
    pub fn available() -> Vec<Adapter> {
        vec![
            Adapter {
                name: "Claude Code",
                cli: "claude",
                color: 0xC08CFF,
                enabled: true,
                tag: "subscription",
            },
            Adapter {
                name: "Codex",
                cli: "codex",
                color: 0x4ED99A,
                enabled: true,
                tag: "subscription",
            },
            Adapter {
                name: "Gemini CLI",
                cli: "gemini",
                color: 0x7FA8FF,
                enabled: true,
                tag: "keyless",
            },
            Adapter {
                name: "Aider",
                cli: "aider",
                color: 0xFFB454,
                enabled: false,
                tag: "api key",
            },
            Adapter {
                name: "Cursor Agent",
                cli: "cursor",
                color: 0x6AD6E0,
                enabled: false,
                tag: "planned",
            },
        ]
    }

    fn selectable(&self) -> bool {
        self.tag != "planned"
    }
}

/// Number of frameworks selected to route across.
pub fn selected_count(adapters: &[Adapter]) -> usize {
    adapters.iter().filter(|a| a.enabled).count()
}

// ── view (methods on Root) ────────────────────────────────────────────────────

impl Root {
    /// Render the full Launcher view.
    pub(crate) fn launcher_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(crate::view_header(&t, "NEW RUN", "Launch a run"))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .justify_center()
                    .gap(px(22.0))
                    .px(px(24.0))
                    .pt(px(22.0))
                    .pb(px(40.0))
                    .child(self.launch_form(cx, &t))
                    .child(self.estimate_card(cx, &t)),
            )
            .into_any_element()
    }

    fn launch_form(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        div()
            .flex_1()
            .max_w(px(620.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(section(
                t,
                "REPOSITORY",
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(40.0))
                    .px(px(13.0))
                    .bg(solid(t.surfaces.panel))
                    .border_1()
                    .border_color(overlay(t.overlays.w09))
                    .rounded(px(9.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(solid(t.text.t1))
                            .child(self.repo.label.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(solid(t.text.t5))
                            .child(format!(
                                "{} · {} files",
                                self.repo.base_ref(),
                                self.repo.files.len()
                            )),
                    ),
            ))
            .child(section(t, "TASK", self.task_editor(cx, t)))
            .child(self.agents_section(cx, t))
            .child(section(
                t,
                "VERIFICATION",
                advisor_row(t, &self.advisor.endpoint, &self.advisor.model),
            ))
            .child(scrub_toggle(self, cx, t))
    }

    /// The editable task field. Clicking focuses it; typing edits `self.task`.
    fn task_editor(&self, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        let mut field = div()
            .id("task")
            .min_h(px(96.0))
            .px(px(14.0))
            .py(px(13.0))
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(overlay(t.overlays.w09))
            .rounded(px(9.0))
            .text_size(px(12.5))
            .text_color(solid(t.text.t2))
            .cursor_pointer()
            .on_click(self.focus_task(cx))
            .child(format!("{}\u{2588}", self.task));
        if let Some(fh) = &self.task_focus {
            field = field
                .track_focus(fh)
                .on_key_down(self.task_key(cx))
                .border_color(tint(t.accent.base, 0.34));
        }
        field.into_any_element()
    }

    fn agents_section(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let selected = selected_count(&self.adapters);
        let mut rows: Vec<AnyElement> = Vec::new();
        for (row, pair) in self.adapters.chunks(2).enumerate() {
            let mut cells: Vec<AnyElement> = Vec::new();
            for (col, a) in pair.iter().enumerate() {
                cells.push(self.adapter_row(cx, t, row * 2 + col, a));
            }
            rows.push(div().flex().gap(px(9.0)).children(cells).into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .gap(px(9.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label(t, "FRAMEWORKS TO ROUTE ACROSS"))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(solid(t.text.t5))
                            .child(format!(
                                "{selected} selected · each gets an isolated worktree"
                            )),
                    ),
            )
            .child(div().flex().flex_col().gap(px(9.0)).children(rows))
    }

    fn adapter_row(
        &self,
        cx: &mut Context<Self>,
        t: &Theme,
        idx: usize,
        a: &Adapter,
    ) -> AnyElement {
        let (bg, border) = if a.enabled {
            (tint(t.accent.base, 0.07), tint(t.accent.base, 0.3))
        } else {
            (solid(t.surfaces.panel), overlay(t.overlays.w07))
        };
        let mut row = div()
            .id(("adapter", idx))
            .flex_1()
            .flex()
            .items_center()
            .gap(px(9.0))
            .px(px(12.0))
            .py(px(11.0))
            .rounded(px(9.0))
            .bg(bg)
            .border_1()
            .border_color(border);
        if a.selectable() {
            row = row
                .cursor_pointer()
                .on_click(self.on(cx, Msg::ToggleAdapter(idx)));
        }
        row.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.0))
                .rounded(px(5.0))
                .border_1()
                .map(|d| {
                    if a.enabled {
                        d.bg(solid(t.accent.base))
                            .border_color(solid(t.accent.base))
                            .text_size(px(11.0))
                            .text_color(solid(0x1A0F2E))
                            .child("✓")
                    } else {
                        d.border_color(overlay(t.overlays.w18))
                    }
                }),
        )
        .child(div().size(px(9.0)).rounded(px(3.0)).bg(solid(a.color)))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(solid(t.text.t1))
                        .child(a.name),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(solid(t.text.t5))
                        .child(a.cli),
                ),
        )
        .child(div().flex_1())
        .child(
            div()
                .px(px(7.0))
                .py(px(2.0))
                .rounded(px(5.0))
                .bg(overlay(t.overlays.w04))
                .border_1()
                .border_color(overlay(t.overlays.w07))
                .text_size(px(9.5))
                .text_color(solid(t.text.t5))
                .child(a.tag),
        )
        .into_any_element()
    }

    fn estimate_card(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement {
        let selected = selected_count(&self.adapters);
        div()
            .w(px(280.0))
            .flex_none()
            .flex()
            .flex_col()
            .bg(solid(t.surfaces.panel))
            .border_1()
            .border_color(overlay(t.overlays.w09))
            .rounded(px(13.0))
            .p(px(18.0))
            .child(
                div()
                    .mb(px(16.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(t.text.t3))
                    .child("PLAN"),
            )
            .child(estimate_row(
                t,
                "Frameworks",
                selected.to_string(),
                t.text.t1,
                true,
            ))
            .child(estimate_row(
                t,
                "Repo files",
                self.repo.files.len().to_string(),
                t.text.t1,
                true,
            ))
            .child(estimate_row(
                t,
                "Routing",
                "cheapest-capable first".into(),
                t.status.green,
                true,
            ))
            .child(estimate_row(
                t,
                "Cost",
                "measured per run".into(),
                t.text.t1,
                false,
            ))
            .child(
                div()
                    .id("launch")
                    .mt(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(40.0))
                    .rounded(px(9.0))
                    .bg(solid(t.accent.base))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(solid(0x1A0F2E))
                    .cursor_pointer()
                    .on_click(self.launch_run(cx))
                    .child("▸ Launch run"),
            )
            .child(
                div()
                    .mt(px(9.0))
                    .flex()
                    .justify_center()
                    .text_size(px(10.5))
                    .text_color(solid(t.text.t6))
                    .child("routes via the real engine"),
            )
    }
}

// ── static / shared sub-parts ──────────────────────────────────────────────────

fn advisor_row(t: &Theme, endpoint: &str, model: &str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(11.0))
        .px(px(14.0))
        .py(px(12.0))
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w09))
        .rounded(px(9.0))
        .child(
            div()
                .size(px(9.0))
                .rounded(px(3.0))
                .bg(solid(t.accent.base)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(solid(t.text.t1))
                        .child(format!("Advisor verifies each result · {model}")),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(solid(t.text.t5))
                        .child(endpoint.to_string()),
                ),
        )
}

fn scrub_toggle(root: &Root, cx: &mut Context<Root>, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(11.0))
        .px(px(14.0))
        .py(px(12.0))
        .bg(solid(t.surfaces.panel))
        .border_1()
        .border_color(overlay(t.overlays.w09))
        .rounded(px(9.0))
        .child(crate::settings::toggle_switch(
            root,
            cx,
            t,
            "scrub",
            root.settings.scrub,
            Msg::ToggleScrub,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(solid(t.text.t1))
                        .child("Scrub secrets before persist"),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(solid(t.text.t5))
                        .child("redact tokens & keys from raw payloads"),
                ),
        )
}

fn estimate_row(
    t: &Theme,
    label: &'static str,
    value: String,
    value_color: Rgb,
    border: bool,
) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .py(px(9.0))
        .when(border, |d| {
            d.border_b_1().border_color(overlay(t.overlays.w05))
        })
        .child(
            div()
                .text_size(px(12.0))
                .text_color(solid(t.text.t3))
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(solid(value_color))
                .child(value),
        )
}

pub(crate) fn section_label(t: &Theme, label: &'static str) -> impl IntoElement {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(solid(t.text.t3))
        .child(label)
}

pub(crate) fn section(t: &Theme, label: &'static str, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(section_label(t, label))
        .child(body)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_frameworks_map_to_clis() {
        let ads = Adapter::available();
        assert!(ads.iter().any(|a| a.cli == "claude"));
        assert!(ads.iter().any(|a| a.cli == "codex"));
        assert_eq!(selected_count(&ads), 3);
    }

    #[test]
    fn planned_is_not_selectable() {
        let cursor = Adapter::available()
            .into_iter()
            .find(|a| a.tag == "planned")
            .unwrap();
        assert!(!cursor.selectable());
    }

    #[test]
    fn selected_count_tracks_enabled() {
        let mut ads = Adapter::available();
        for a in &mut ads {
            a.enabled = false;
        }
        assert_eq!(selected_count(&ads), 0);
    }
}

use gpui::{
    App, Context, Div, InteractiveElement, IntoElement, ParentElement, Role,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};
use pipkin_core::*;

use super::commands::{self, Cmd};
use super::controls::*;
use super::workspace::{Overlay, Workspace};
use crate::model::DemoControls;
use crate::theme::ActiveTheme;

fn native_usage(profile: ExecutionProfile, usage: Option<SessionUsage>) -> Option<SessionUsage> {
    usage.filter(|_| profile == ExecutionProfile::NativeAgent)
}

/// Put a submenu beside its parent, preferring the right edge. On a narrow window
/// neither side fits, so overlap the parent instead of losing the submenu offscreen.
fn submenu_x(parent_x: f32, parent_width: f32, submenu_width: f32, window_width: f32) -> f32 {
    let right = parent_x + parent_width + 6.0;
    let left = parent_x - submenu_width - 6.0;
    if right + submenu_width <= window_width - 8.0 {
        right
    } else if left >= 8.0 {
        left
    } else {
        right.clamp(8.0, (window_width - submenu_width - 8.0).max(8.0))
    }
}

/// Choose the vertical edge offering the most room, unless downward already fits.
/// The returned height includes the menu's chrome, not just its scrolling list.
fn submenu_vertical(top: f32, bottom: f32, height: f32, desired: f32) -> (bool, f32, f32) {
    let top = top.clamp(8.0, (height - 8.0).max(8.0));
    let bottom = bottom.clamp(8.0, (height - 8.0).max(8.0));
    let down = (height - 8.0 - top).max(0.0);
    let up = (bottom - 8.0).max(0.0);
    if down >= desired || down >= up {
        (false, top, down.min(desired))
    } else {
        (true, bottom, up.min(desired))
    }
}

pub(super) fn format_tokens(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i != 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

pub(super) fn format_cost(cost: Option<f64>) -> String {
    match cost {
        None => "Not reported".into(),
        Some(n) if n > 0.0 && n < 0.0001 => "<$0.0001".into(),
        Some(n) if n < 1.0 => format!("${n:.4}"),
        Some(n) => format!("${n:.2}"),
    }
}

pub(super) fn effort_label(level: &str) -> String {
    match level {
        "off" => "Off".into(),
        "xhigh" => "Extra high".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().collect::<String>() + chars.as_str()
            })
        }
    }
}

impl Workspace {
    fn palette_items(&self, cx: &gpui::App) -> Vec<Cmd> {
        let q = self.overlay_input.read(cx).text();
        let demo = cx.try_global::<DemoControls>();
        let mut scored: Vec<(usize, Cmd)> = commands::build(self.state(cx), demo)
            .into_iter()
            .filter_map(|mut c| {
                if let super::commands::Run::LocalCli(existing) = &c.run {
                    c.enabled = self.local_cli_entry_enabled(*existing, cx);
                }
                commands::fuzzy_score(q.trim(), &format!("{} {}", c.group, c.title)).map(|s| (s, c))
            })
            .collect();
        scored.sort_by_key(|(s, _)| *s);
        scored.into_iter().map(|(_, c)| c).collect()
    }

    pub(super) fn overlay_len(&self, cx: &gpui::App) -> usize {
        match self.overlay {
            Overlay::Palette => self.palette_items(cx).len(),
            Overlay::Model => self.state(cx).models.len(),
            Overlay::ModelSettings => 2,
            Overlay::Effort => self
                .state(cx)
                .current()
                .map_or(0, |c| c.thinking_levels.len()),
            // The projects, then "Open project folder…" when one can be opened.
            Overlay::Project => {
                self.state(cx).projects.len() + usize::from(self.state(cx).can_create)
            }
            Overlay::Question => match self.waiting_question(cx, true) {
                Some(q) => match q.kind {
                    UiRequestKind::Select => q.items.len(),
                    UiRequestKind::Confirm => 2,
                    UiRequestKind::Input => 0,
                    UiRequestKind::Form => form_options(&q).len(),
                },
                None => 0,
            },
            _ => 0,
        }
    }

    pub(super) fn confirm_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.overlay_sel;
        match self.overlay {
            Overlay::Palette => {
                let items = self.palette_items(cx);
                if let Some(cmd) = items.get(sel) {
                    if !cmd.enabled {
                        return;
                    }
                    self.close_overlay(window, cx);
                    self.run_command(&cmd.run, window, cx);
                }
            }
            Overlay::ModelSettings => match sel {
                0 => self.open_overlay(Overlay::Model, window, cx),
                1 if self
                    .state(cx)
                    .current()
                    .is_some_and(|c| !c.thinking_levels.is_empty()) =>
                {
                    self.open_overlay(Overlay::Effort, window, cx);
                }
                _ => {}
            },
            Overlay::Effort => {
                let level = self
                    .state(cx)
                    .current()
                    .and_then(|c| c.thinking_levels.get(sel).cloned());
                if let Some(level) = level {
                    self.dispatch(Command::SetThinkingLevel(level), cx);
                    self.close_overlay(window, cx);
                }
            }
            Overlay::Model => {
                if let Some(m) = self.state(cx).models.get(sel).map(|m| m.id.clone()) {
                    self.dispatch(Command::SetModel(m), cx);
                    self.close_overlay(window, cx);
                }
            }
            Overlay::Project => {
                let projects = self.state(cx).projects.len();
                if sel == projects && self.state(cx).can_create {
                    // The last row opens the folder picker instead of choosing a project.
                    self.close_overlay(window, cx);
                    self.open_project(cx);
                } else if let Some(p) = self.state(cx).projects.get(sel).map(|p| p.id) {
                    self.dispatch(Command::SelectProject(p), cx);
                    self.close_overlay(window, cx);
                }
            }
            Overlay::Question => {
                let Some(q) = self.waiting_question(cx, true) else {
                    return self.close_overlay(window, cx);
                };
                let answer = match q.kind {
                    UiRequestKind::Select => {
                        q.items.get(sel).map(|i| UiAnswer::Choice(i.value.clone()))
                    }
                    UiRequestKind::Confirm => Some(UiAnswer::Confirm(sel == 0)),
                    UiRequestKind::Input => {
                        Some(UiAnswer::Text(self.overlay_input.read(cx).text()))
                    }
                    // Enter toggles the highlighted option; only the Answer button submits.
                    UiRequestKind::Form => {
                        if let Some((question, option)) = form_options(&q).get(sel).cloned() {
                            self.dispatch(Command::ToggleFormOption { question, option }, cx);
                        }
                        None
                    }
                };
                if let Some(answer) = answer {
                    self.dispatch(Command::AnswerUiRequest { id: q.id, answer }, cx);
                    self.close_overlay(window, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let overlay = self.overlay;
        let this = cx.entity();
        let transparent = matches!(
            overlay,
            Overlay::Model | Overlay::ModelSettings | Overlay::Effort | Overlay::Project
        );
        let backdrop = div()
            .id("overlay-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .when(!transparent, |d| d.bg(c.scrim))
            .on_click({
                let this = this.clone();
                move |_, window, cx| this.update(cx, |t, cx| t.close_overlay(window, cx))
            });

        // Keep the settings menu visible while a submenu opens next to it.
        let settings_bounds = self.submenu_parent_bounds(window, cx);
        let panel: gpui::AnyElement = match overlay {
            Overlay::Palette => self.render_palette(cx).into_any_element(),
            Overlay::ModelSettings => self.render_model_settings(true, cx).into_any_element(),
            Overlay::Model | Overlay::Effort => {
                self.render_model_settings(false, cx).into_any_element()
            }
            Overlay::Project => self.render_menu_overlay(window, cx).into_any_element(),
            Overlay::Rename(_) => self.render_rename(cx).into_any_element(),
            Overlay::Prefs => self.render_prefs(window, cx).into_any_element(),
            Overlay::LocalCli => self.render_local_cli(cx).into_any_element(),
            Overlay::CliApprovals => self.render_cli_approvals(window, cx).into_any_element(),
            Overlay::Session => self.render_session(window, cx).into_any_element(),
            Overlay::About => self.render_about(cx).into_any_element(),
            Overlay::Question => self.render_question(cx).into_any_element(),
            Overlay::None => div().into_any_element(),
        };
        let submenu = match overlay {
            Overlay::Model | Overlay::Effort => {
                let contents = if overlay == Overlay::Model {
                    self.render_menu_overlay(window, cx).into_any_element()
                } else {
                    self.render_effort_menu(window, cx).into_any_element()
                };
                let (upward, y, _) = self.submenu_vertical_layout(window, cx);
                let x = submenu_x(
                    f32::from(settings_bounds.origin.x),
                    f32::from(settings_bounds.size.width),
                    300.0 * t.scale.max(1.0),
                    f32::from(window.viewport_size().width),
                );
                Some(
                    gpui::deferred(
                        gpui::anchored()
                            .anchor(if upward {
                                gpui::Anchor::BottomLeft
                            } else {
                                gpui::Anchor::TopLeft
                            })
                            .position(gpui::point(px(x), px(y)))
                            .snap_to_window_with_margin(px(8.0))
                            .child(contents),
                    )
                    .with_priority(3)
                    .into_any_element(),
                )
            }
            _ => None,
        };
        let h = window.viewport_size().height;
        let positioned = match overlay {
            // Opens just above the model button: its bottom-left corner sits at the button's
            // top-left, in window coordinates, and is kept inside the window.
            Overlay::Model | Overlay::ModelSettings | Overlay::Effort => match self.model_button {
                Some(button) => gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::BottomLeft)
                        .position(gpui::point(button.origin.x, button.origin.y - px(6.0)))
                        .snap_to_window_with_margin(px(8.0))
                        .child(panel),
                )
                .with_priority(2)
                .into_any_element(),
                None => div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .items_center()
                    .pb(px(168.0))
                    .child(panel)
                    .into_any_element(),
            },
            Overlay::Project => div()
                .absolute()
                .left(px(8.0))
                .top(px(52.0))
                .child(panel)
                .into_any_element(),
            Overlay::Prefs => div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(panel)
                .into_any_element(),
            _ => div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .pt(px((f32::from(h) * 0.16).max(40.0)))
                .child(panel)
                .into_any_element(),
        };
        div()
            .absolute()
            .inset_0()
            .child(backdrop)
            .child(positioned)
            .children(submenu)
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let items = self.palette_items(cx);
        let sel = self.overlay_sel.min(items.len().saturating_sub(1));
        let this = cx.entity();
        let empty = items.is_empty();
        elevated(cx)
            .id("palette")
            .role(Role::Dialog)
            .aria_label("Command palette")
            .w(px(560.0 * t.scale.max(1.0)))
            .max_w_full()
            .flex()
            .flex_col()
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(14.0))
                    .h(px(46.0))
                    .border_b_1()
                    .border_color(c.border)
                    .child(icon("search", px(16.0), c.text_faint))
                    .child(div().flex_1().child(self.overlay_input.clone())),
            )
            .child(
                div()
                    .id("palette-list")
                    .role(Role::ListBox)
                    .flex()
                    .flex_col()
                    .p(px(6.0))
                    .max_h(px(380.0))
                    .overflow_y_scroll()
                    .children(items.into_iter().enumerate().map(|(i, cmd)| {
                        let this = this.clone();
                        let enabled = cmd.enabled;
                        let mut row = menu_row(("cmd", i), i == sel, cx)
                            .role(Role::ListBoxOption)
                            .aria_label(cmd.title.clone())
                            .aria_selected(i == sel)
                            .when(!enabled, |d| d.opacity(0.45))
                            .on_click(move |_, window, cx| {
                                this.update(cx, |t, cx| {
                                    t.overlay_sel = i;
                                    t.confirm_selection(window, cx);
                                })
                            })
                            .child(div().w(px(16.0)).child(if cmd.checked {
                                icon("check", px(14.0), c.accent).into_any_element()
                            } else {
                                div().into_any_element()
                            }))
                            .child(div().flex_1().min_w_0().truncate().child(cmd.title.clone()))
                            .child(
                                div()
                                    .text_size(t.small_size())
                                    .text_color(c.text_faint)
                                    .child(cmd.group),
                            );
                        if let Some(k) = cmd.shortcut {
                            row = row.child(kbd(k, cx));
                        }
                        row
                    }))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .px(px(12.0))
                                .py(px(16.0))
                                .text_color(c.text_muted)
                                .child("No matching commands"),
                        )
                    }),
            )
    }

    /// A question an extension asked: its choices, a yes/no, or a line of text.
    fn render_question(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let question = self.waiting_question(cx, true);
        let Some(q) = question else {
            return elevated(cx)
                .id("question")
                .child("This question is no longer open.");
        };
        if let Some(form) = q.form.clone().filter(|_| q.kind == UiRequestKind::Form) {
            return self.render_form(&q, &form, cx);
        }
        let (answering, error, now_ms) = {
            let s = self.state(cx);
            let conv = s.current();
            (
                conv.is_some_and(|c| c.ui_answering.contains(&q.id)),
                conv.and_then(|c| c.ui_error.clone()),
                s.now() * 1000,
            )
        };
        let sel = self.overlay_sel;
        let rows: Vec<(String, Option<String>)> = match q.kind {
            UiRequestKind::Select => q
                .items
                .iter()
                .map(|i| (i.label.clone(), i.description.clone()))
                .collect(),
            UiRequestKind::Confirm => vec![("Yes".into(), None), ("No".into(), None)],
            UiRequestKind::Input | UiRequestKind::Form => vec![],
        };
        let time_left = q.deadline.map(|d| {
            let secs = ((d - now_ms) / 1000).max(0);
            if secs >= 90 {
                format!("Cancels itself in about {} min", (secs + 59) / 60)
            } else {
                format!("Cancels itself in {secs} s")
            }
        });
        let decline = {
            let this = this.clone();
            let id = q.id.clone();
            Btn::new("question-decline")
                .label("Decline")
                .kind(BtnKind::Subtle)
                .compact()
                .on_click(move |window, cx| {
                    this.update(cx, |t, cx| {
                        t.dispatch(Command::CancelUiRequest(id.clone()), cx);
                        t.close_overlay(window, cx);
                    })
                })
        };
        elevated(cx)
            .id("question")
            .key_context("Overlay")
            .track_focus(&self.menu_focus)
            .role(Role::Dialog)
            .aria_label(format!("Question: {}", q.title))
            .w(px(520.0 * t.scale.max(1.0)))
            .max_w_full()
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .occlude()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(q.title.clone()),
            )
            .children(q.message.clone().map(|m| {
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(m)
            }))
            .when(q.kind == UiRequestKind::Input, |d| {
                d.child(
                    div()
                        .px(px(10.0))
                        .h(px(34.0))
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .bg(c.bg_input)
                        .border_1()
                        .border_color(c.border_strong)
                        .child(div().flex_1().child(self.overlay_input.clone())),
                )
            })
            .when(!rows.is_empty(), |d| {
                d.child(
                    div()
                        .id("question-list")
                        .role(Role::ListBox)
                        .flex()
                        .flex_col()
                        .max_h(px(280.0))
                        .overflow_y_scroll()
                        .children(rows.into_iter().enumerate().map(|(i, (label, note))| {
                            let this = this.clone();
                            menu_row(("choice", i), i == sel, cx)
                                .role(Role::ListBoxOption)
                                .aria_label(label.clone())
                                .aria_selected(i == sel)
                                .on_click(move |_, window, cx| {
                                    this.update(cx, |t, cx| {
                                        t.overlay_sel = i;
                                        t.confirm_selection(window, cx);
                                    })
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(div().truncate().child(label))
                                        .children(note.map(|n| {
                                            div()
                                                .truncate()
                                                .text_size(t.small_size())
                                                .text_color(c.text_faint)
                                                .child(n)
                                        })),
                                )
                        })),
                )
            })
            .children(error.map(|e| {
                div()
                    .text_size(t.small_size())
                    .text_color(c.danger)
                    .child(format!("Not accepted: {e}"))
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(t.small_size())
                            .text_color(c.text_faint)
                            .child(if answering {
                                "Sending your answer…".to_owned()
                            } else {
                                time_left.unwrap_or_default()
                            }),
                    )
                    .child(decline)
                    .when(q.kind == UiRequestKind::Input, |d| {
                        let this = this.clone();
                        d.child(
                            Btn::new("question-send")
                                .label("Answer")
                                .kind(BtnKind::Primary)
                                .compact()
                                .disabled(answering)
                                .on_click(move |window, cx| {
                                    this.update(cx, |t, cx| t.confirm_selection(window, cx))
                                }),
                        )
                    }),
            )
    }

    /// A questionnaire (ASK-2): every question with its options, text fields and notes, a
    /// bounded Markdown preview where the asker gave one, and one explicit submission. The
    /// selections live in the core's draft; nothing reaches the engine until Answer.
    fn render_form(
        &mut self,
        q: &UiRequest,
        form: &UiForm,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let sel = self.overlay_sel;
        let (answering, error, selections) = {
            let s = self.state(cx);
            let conv = s.current();
            (
                conv.is_some_and(|c| c.ui_answering.contains(&q.id)),
                conv.and_then(|c| c.ui_error.clone()),
                conv.and_then(|c| c.form_draft.clone()).unwrap_or_default(),
            )
        };
        let selected_of = |question: &str| -> Vec<String> {
            selections
                .selections
                .iter()
                .find(|(id, _)| id == question)
                .map(|(_, s)| s.clone())
                .unwrap_or_default()
        };
        let mut flat = 0usize;
        let mut body = div()
            .id("form-body")
            .flex()
            .flex_col()
            .gap(px(16.0))
            .max_h(px(460.0 * t.scale.max(1.0)))
            .overflow_y_scroll()
            .pr(px(4.0));
        for (qi, question) in form.questions.iter().enumerate() {
            let selected = selected_of(&question.id);
            let mut block = div()
                .id(("form-question", qi))
                .role(Role::Group)
                .aria_label(question.prompt.clone())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                    "{}. {}",
                    qi + 1,
                    question.prompt
                )));
            if let Some(d) = &question.description {
                block = block.child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child(d.clone()),
                );
            }
            if let Some(preview) = &question.preview {
                block = block.child(preview_blocks(preview, &t));
            }
            for option in &question.options {
                let i = flat;
                flat += 1;
                let on = selected.iter().any(|s| s == &option.id);
                let (qid, oid) = (question.id.clone(), option.id.clone());
                let this = this.clone();
                let multiple = question.kind == UiFormKind::Multiple;
                let marker = div()
                    .flex_none()
                    .size(px(14.0))
                    .border_1()
                    .border_color(if on { c.accent } else { c.border_strong })
                    .rounded(if multiple { px(3.0) } else { px(7.0) })
                    .when(on, |d| d.bg(c.accent_fill));
                let mut row = menu_row(("form-option", i), i == sel, cx)
                    .role(if multiple {
                        Role::CheckBox
                    } else {
                        Role::RadioButton
                    })
                    .aria_label(option.label.clone())
                    .aria_selected(on)
                    .items_start()
                    .py(px(6.0))
                    .on_click(move |_, _, cx| {
                        this.update(cx, |t, cx| {
                            t.overlay_sel = i;
                            t.dispatch(
                                Command::ToggleFormOption {
                                    question: qid.clone(),
                                    option: oid.clone(),
                                },
                                cx,
                            );
                        })
                    })
                    .child(div().pt(px(3.0)).child(marker));
                let mut text = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().child(option.label.clone()));
                if let Some(d) = &option.description {
                    text = text.child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_faint)
                            .child(d.clone()),
                    );
                }
                if let Some(preview) = &option.preview {
                    text = text.child(preview_blocks(preview, &t));
                }
                row = row.child(text);
                block = block.child(row);
            }
            if let Some(editor) = self.form_editor(&question.id, "text") {
                block = block.child(field_box(editor.clone(), cx));
            }
            if let Some(editor) = self.form_editor(&question.id, "note") {
                block = block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_none()
                                .text_size(t.small_size())
                                .text_color(c.text_muted)
                                .child("Note"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(field_box(editor.clone(), cx)),
                        ),
                );
            }
            body = body.child(block);
        }
        if let Some(editor) = self.form_editor("", "note") {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child("A note about these questions"),
                    )
                    .child(field_box(editor.clone(), cx)),
            );
        }
        let decline = {
            let this = this.clone();
            let id = q.id.clone();
            Btn::new("question-decline")
                .label("Decline")
                .kind(BtnKind::Subtle)
                .compact()
                .on_click(move |window, cx| {
                    this.update(cx, |t, cx| {
                        t.dispatch(Command::CancelUiRequest(id.clone()), cx);
                        t.close_overlay(window, cx);
                    })
                })
        };
        let submit = {
            let this = this.clone();
            Btn::new("question-send")
                .label("Answer")
                .kind(BtnKind::Primary)
                .compact()
                .disabled(answering)
                .aria("Send these answers")
                .on_click(move |window, cx| this.update(cx, |t, cx| t.submit_form(window, cx)))
        };
        elevated(cx)
            .id("question")
            .key_context("Overlay")
            .track_focus(&self.menu_focus)
            .role(Role::Dialog)
            .aria_label(format!("Questions: {}", q.title))
            .w(px(640.0 * t.scale.max(1.0)))
            .max_w_full()
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .occlude()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(q.title.clone()),
            )
            .children(q.message.clone().map(|m| {
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(m)
            }))
            .child(body)
            .children(error.map(|e| {
                div()
                    .text_size(t.small_size())
                    .text_color(c.danger)
                    .child(format!("Not accepted: {e}"))
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(t.small_size())
                            .text_color(c.text_faint)
                            .child(if answering {
                                "Sending your answers…"
                            } else {
                                "Nothing is sent until you answer; questions you leave blank stay unanswered."
                            }),
                    )
                    .child(decline)
                    .child(submit),
            )
    }

    fn render_model_settings(&mut self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let model_settings = self.state(cx).execution_capabilities().model_settings;
        let (model, effort, has_effort) = {
            let s = self.state(cx);
            let model = super::local_cli::restricted_model_label(s)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    s.models
                        .iter()
                        .find(|m| Some(&m.id) == s.prefs.model.as_ref())
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| "Choose model".into())
                });
            let effort = s
                .current()
                .and_then(|conv| conv.thinking_level.as_deref())
                .map(effort_label)
                .unwrap_or_else(|| "Unavailable".into());
            let has_effort = s
                .current()
                .is_some_and(|conv| !conv.thinking_levels.is_empty());
            (model, effort, has_effort)
        };
        let this = cx.entity();
        let measure = this.clone();
        let selected = match self.overlay {
            Overlay::Model => 0,
            Overlay::Effort => 1,
            _ => self.overlay_sel,
        };
        elevated(cx)
            .id("model-settings")
            .relative()
            .key_context("Overlay")
            .when(focused, |menu| menu.track_focus(&self.menu_focus))
            .role(Role::Menu)
            .aria_label("Model and effort")
            .w(px(300.0 * t.scale.max(1.0)))
            .p(px(6.0))
            .flex()
            .flex_col()
            .occlude()
            .children(
                [
                    (0, "Model", model, model_settings),
                    (1, "Effort", effort, has_effort && model_settings),
                ]
                .into_iter()
                .map(|(i, label, value, enabled)| {
                    let this = this.clone();
                    menu_row(("setting", i), i == selected, cx)
                        .role(Role::MenuItem)
                        .aria_label(format!("{label}: {value}"))
                        .when(!enabled, |r| r.opacity(0.45))
                        .on_click(move |_, window, cx| {
                            if enabled {
                                this.update(cx, |t, cx| {
                                    t.open_overlay(
                                        if i == 0 {
                                            Overlay::Model
                                        } else {
                                            Overlay::Effort
                                        },
                                        window,
                                        cx,
                                    );
                                });
                            }
                        })
                        .child(div().flex_1().child(label))
                        .child(
                            div()
                                .max_w(px(145.0))
                                .truncate()
                                .text_color(c.text_muted)
                                .child(value),
                        )
                        .child(icon("chevron-right", px(14.0), c.text_muted))
                }),
            )
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        measure.update(cx, |t, _| t.model_settings_bounds = Some(bounds))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }

    fn submenu_parent_bounds(&self, window: &Window, cx: &gpui::App) -> gpui::Bounds<gpui::Pixels> {
        self.model_settings_bounds.unwrap_or_else(|| {
            let t = cx.theme();
            let width = 300.0 * t.scale.max(1.0);
            let height = 2.0 * f32::from(t.control_height()) + 14.0;
            let button = self
                .model_button
                .map(|b| b.origin)
                .unwrap_or(gpui::point(px(8.0), px(height + 14.0)));
            gpui::Bounds::new(
                gpui::point(
                    px(f32::from(button.x).clamp(
                        8.0,
                        (f32::from(window.viewport_size().width) - width - 8.0).max(8.0),
                    )),
                    px((f32::from(button.y) - 6.0 - height).max(8.0)),
                ),
                gpui::size(px(width), px(height)),
            )
        })
    }

    fn submenu_vertical_layout(&self, window: &Window, cx: &gpui::App) -> (bool, f32, f32) {
        let t = cx.theme();
        let parent = self.submenu_parent_bounds(window, cx);
        let chrome = f32::from(t.control_height()) + 23.0;
        submenu_vertical(
            f32::from(parent.origin.y),
            f32::from(parent.origin.y + parent.size.height),
            f32::from(window.viewport_size().height),
            420.0 * t.scale.max(1.0) + chrome,
        )
    }

    fn submenu_list_limit(&self, window: &Window, cx: &gpui::App) -> f32 {
        let (_, _, room) = self.submenu_vertical_layout(window, cx);
        (room - f32::from(cx.theme().control_height()) - 23.0).max(0.0)
    }

    fn render_effort_menu(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let (levels, selected) = self.state(cx).current().map_or_else(
            || (Vec::new(), None),
            |conv| (conv.thinking_levels.clone(), conv.thinking_level.clone()),
        );
        let this = cx.entity();
        elevated(cx)
            .id("effort-menu")
            .key_context("Overlay")
            .track_focus(&self.menu_focus)
            .role(Role::Menu)
            .aria_label("Effort")
            .w(px(300.0 * t.scale.max(1.0)))
            .p(px(6.0))
            .flex()
            .flex_col()
            .occlude()
            .child(
                menu_row("effort-back", false, cx)
                    .role(Role::MenuItem)
                    .aria_label("Back to model and effort")
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            this.update(cx, |t, cx| {
                                t.open_overlay(Overlay::ModelSettings, window, cx)
                            })
                        }
                    })
                    .child(icon("chevron-left", px(14.0), c.text_muted))
                    .child("Model and effort"),
            )
            .child(div().h(px(1.0)).w_full().my(px(4.0)).bg(c.border))
            .child(
                div()
                    .id("effort-list")
                    .flex()
                    .flex_col()
                    .max_h(px(self.submenu_list_limit(window, cx)))
                    .overflow_y_scroll()
                    .children(levels.into_iter().enumerate().map(|(i, level)| {
                        let this = this.clone();
                        let checked = Some(&level) == selected.as_ref();
                        menu_row(("effort", i), i == self.overlay_sel, cx)
                            .role(Role::MenuItem)
                            .aria_label(format!("{} effort", effort_label(&level)))
                            .on_click(move |_, window, cx| {
                                this.update(cx, |t, cx| {
                                    t.overlay_sel = i;
                                    t.confirm_selection(window, cx);
                                });
                            })
                            .child(div().w(px(16.0)).child(if checked {
                                icon("check", px(14.0), c.accent).into_any_element()
                            } else {
                                div().into_any_element()
                            }))
                            .child(effort_label(&level))
                    })),
            )
    }

    fn render_menu_overlay(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let model_mode = self.overlay == Overlay::Model;
        let sel = self.overlay_sel;
        // Side menus must fit below the parent's top, rather than jumping above it.
        let list_cap = if model_mode {
            self.submenu_list_limit(window, cx)
        } else {
            420.0 * t.scale.max(1.0)
        };
        // The project menu ends with a row that opens the folder picker.
        let action_row = !model_mode && self.state(cx).can_create;
        let entries: Vec<(String, String, bool)> = {
            let s = self.state(cx);
            if model_mode {
                s.models
                    .iter()
                    .map(|m| {
                        (
                            m.name.clone(),
                            m.note.clone(),
                            Some(&m.id) == s.prefs.model.as_ref(),
                        )
                    })
                    .collect()
            } else {
                let cur = s.current_project().map(|p| p.id);
                let mut rows: Vec<(String, String, bool)> = s
                    .projects
                    .iter()
                    .map(|p| (p.name.clone(), p.path.clone(), Some(p.id) == cur))
                    .collect();
                if s.can_create {
                    rows.push(("Open project folder\u{2026}".into(), String::new(), false));
                }
                rows
            }
        };
        let count = entries.len();
        elevated(cx)
            .id("menu-overlay")
            .key_context("Overlay")
            .track_focus(&self.menu_focus)
            .role(Role::Menu)
            .aria_label(if model_mode { "Models" } else { "Projects" })
            .w(px(300.0 * t.scale.max(1.0)))
            .p(px(6.0))
            .flex()
            .flex_col()
            .occlude()
            .when(model_mode, |d| {
                let this = this.clone();
                d.child(
                    menu_row("model-back", false, cx)
                        .role(Role::MenuItem)
                        .aria_label("Back to model and effort")
                        .on_click(move |_, window, cx| {
                            this.update(cx, |t, cx| {
                                t.open_overlay(Overlay::ModelSettings, window, cx)
                            })
                        })
                        .child(icon("chevron-left", px(14.0), c.text_muted))
                        .child("Model and effort"),
                )
                .child(div().h(px(1.0)).w_full().my(px(4.0)).bg(c.border))
            })
            .child(
                // A long list (every model a provider offers) scrolls inside the menu instead
                // of running off the window.
                div()
                    .id("menu-list")
                    .flex()
                    .flex_col()
                    .max_h(px(list_cap))
                    .overflow_y_scroll()
                    .track_scroll(&self.menu_scroll)
                    .children(
                        entries
                            .into_iter()
                            .enumerate()
                            .map(|(i, (name, note, checked))| {
                                let this = this.clone();
                                let is_action = action_row && i + 1 == count;
                                let row = menu_row(("menu", i), i == sel, cx)
                                    .role(Role::MenuItem)
                                    .aria_label(name.clone())
                                    .on_click(move |_, window, cx| {
                                        this.update(cx, |t, cx| {
                                            t.overlay_sel = i;
                                            t.confirm_selection(window, cx);
                                        })
                                    })
                                    .child(div().w(px(16.0)).child(if is_action {
                                        icon("folder-open", px(14.0), c.text_muted)
                                            .into_any_element()
                                    } else if checked {
                                        icon("check", px(14.0), c.accent).into_any_element()
                                    } else {
                                        div().into_any_element()
                                    }))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(div().truncate().child(name))
                                            .when(!note.is_empty(), |d| {
                                                d.child(
                                                    div()
                                                        .truncate()
                                                        .text_size(t.small_size())
                                                        .text_color(c.text_faint)
                                                        .child(note),
                                                )
                                            }),
                                    );
                                // A rule sets the action apart from the projects above it.
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_none()
                                    .when(is_action, |d| {
                                        d.child(div().h(px(1.0)).w_full().my(px(4.0)).bg(c.border))
                                    })
                                    .child(row)
                            }),
                    ),
            )
    }

    fn render_rename(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let Overlay::Rename(id) = self.overlay else {
            return div().into_any_element();
        };
        let this2 = this.clone();
        elevated(cx)
            .id("rename")
            .role(Role::Dialog)
            .aria_label("Rename conversation")
            .w(px(420.0 * t.scale.max(1.0)))
            .max_w_full()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .occlude()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Rename conversation"),
            )
            .child(
                div()
                    .px(px(10.0))
                    .h(t.control_height())
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(c.bg_input)
                    .border_1()
                    .border_color(c.accent)
                    .child(div().flex_1().child(self.overlay_input.clone())),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        Btn::new("rename-cancel")
                            .label("Cancel")
                            .on_click(move |window, cx| {
                                this.update(cx, |t, cx| t.close_overlay(window, cx))
                            }),
                    )
                    .child(
                        Btn::new("rename-save")
                            .label("Rename")
                            .kind(BtnKind::Primary)
                            .on_click(move |window, cx| {
                                this2.update(cx, |t, cx| {
                                    let title = t.overlay_input.read(cx).text();
                                    t.dispatch(Command::RenameConversation(id, title), cx);
                                    t.close_overlay(window, cx);
                                })
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_session(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let local_cli = self
            .state(cx)
            .current()
            .is_some_and(|cv| cv.profile.is_local_cli());
        // Native usage rows are not CLI accounting. In particular an empty native
        // bucket must not present a fabricated zero-dollar CLI bill.
        let usage = self
            .state(cx)
            .current()
            .and_then(|cv| native_usage(cv.profile, cv.usage.clone()));
        let context = self.state(cx).current().and_then(|cv| cv.context.clone());
        let demo = self.state(cx).mode == Mode::Demo;
        let this = cx.entity();
        let row = |label: String, value: String| {
            div()
                .flex()
                .justify_between()
                .gap(px(16.0))
                .child(div().min_w_0().text_color(c.text_muted).child(label))
                .child(div().font_family(t.mono_font()).child(value))
        };
        let totals = usage.as_ref().map(SessionUsage::total);
        let accessible = totals.as_ref().map_or_else(
            || {
                if local_cli {
                    "Local CLI usage and billing are unverified.".to_string()
                } else {
                    "Session usage unavailable.".to_string()
                }
            },
            |total| {
                format!(
                    "Session usage. {} total tokens. Reported cost {}.",
                    format_tokens(total.total_tokens),
                    format_cost(total.cost_usd)
                )
            },
        );
        elevated(cx)
            .id("session-usage")
            .key_context("Overlay")
            .track_focus(&self.menu_focus)
            .role(Role::Dialog)
            .aria_label(accessible)
            .w(px(480.0 * t.scale.max(1.0)))
            .max_w_full()
            .max_h(window.viewport_size().height - px(24.0))
            .overflow_y_scroll()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Session usage"),
                    )
                    .child(
                        Btn::new("session-close")
                            .icon("x")
                            .aria("Close session usage")
                            .compact()
                            .on_click(move |window, cx| {
                                this.update(cx, |t, cx| t.close_overlay(window, cx))
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(usage.as_ref().map_or_else(
                        || {
                            if local_cli {
                                "CLI usage is not integrated here. Check Claude Code for account usage; subscription billing is unverified.".to_string()
                            } else if demo {
                                "Demo · no model usage".to_string()
                            } else {
                                "The engine has not reported usage for this session.".to_string()
                            }
                        },
                        |u| format!("Session {} · committed totals", u.session_id),
                    )),
            )
            .when_some(totals, |d, total| {
                d.child(div().h(px(1.0)).bg(c.border))
                    .child(row("Input".into(), format_tokens(total.input)))
                    .child(row("Output".into(), format_tokens(total.output)))
                    .child(row("Cache read".into(), format_tokens(total.cache_read)))
                    .child(row("Cache write".into(), format_tokens(total.cache_write)))
                    .when_some(total.cache_write_1h, |d, tokens| {
                        d.child(row("Of cache write: 1 hour".into(), format_tokens(tokens)))
                    })
                    .when_some(total.reasoning, |d, tokens| {
                        d.child(row("Of output: reasoning".into(), format_tokens(tokens)))
                    })
                    .child(row(
                        "Total tokens".into(),
                        format_tokens(total.total_tokens),
                    ))
                    .child(row("Reported cost".into(), format_cost(total.cost_usd)))
            })
            .when_some(context.filter(|x| x.known()), |d, context| {
                let tokens = context.tokens.unwrap_or(0);
                d.child(div().h(px(1.0)).bg(c.border))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Context for the next request"),
                    )
                    .child(row(
                        format!("In use ({})", context.source.label()),
                        match (context.capacity, context.percent()) {
                            (Some(capacity), Some(percent)) => format!(
                                "{} of {} · {percent}%",
                                format_tokens(tokens),
                                format_tokens(capacity)
                            ),
                            _ => format!("{} · window unknown", format_tokens(tokens)),
                        },
                    ))
                    .when_some(context.reserve, |d, reserve| {
                        d.child(row(
                            "Kept free for compaction".into(),
                            format_tokens(reserve),
                        ))
                    })
                    .when_some(context.remaining, |d, remaining| {
                        d.child(row("Remaining".into(), format_tokens(remaining)))
                    })
                    .child(row(
                        "Compaction".into(),
                        match context.compaction.as_str() {
                            "background" => "summarizing alongside work".into(),
                            "blocking" => "before the next request".into(),
                            "off" => "off".into(),
                            _ => "idle".into(),
                        },
                    ))
                    .when(context.stale, |d| {
                        d.child(div().text_size(t.small_size()).text_color(c.warning).child(
                            "Measured with a different model; the next response will refresh it.",
                        ))
                    })
            })
            .when_some(usage, |d, usage| {
                let buckets = usage.models.into_iter().chain(
                    usage
                        .tools
                        .into_iter()
                        .map(|(name, amount)| (format!("Tool · {name}"), amount)),
                );
                d.child(div().h(px(1.0)).bg(c.border))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("By model and tool"),
                    )
                    .child(
                        div()
                            .id("session-usage-models")
                            .max_h(px(200.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(7.0))
                            .children(buckets.map(|(name, amount)| {
                                row(
                                    name,
                                    format!(
                                        "{} · {}",
                                        format_tokens(amount.total_tokens),
                                        format_cost(amount.cost_usd)
                                    ),
                                )
                            })),
                    )
            })
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_faint)
                    .child("Cost is the engine’s reported estimate, not a billing balance."),
            )
            .child(div().h(px(1.0)).bg(c.border))
            .child(self.render_diagnostics_section(cx))
    }

    /// The engine's own measurements for this session (OBS-1): a summary and the newest
    /// records, fetched when the dialog opens or on Refresh; an export writes them to a file
    /// in the engine's data directory. Engine-measured model and tool latency only; the
    /// desktop's input-to-frame figures are a separate measurement.
    fn render_diagnostics_section(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        use pipkin_core::diagnostics::format_ms;
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let demo = self.state(cx).mode == Mode::Demo;
        let real = self.state(cx).mode == Mode::Real && self.state(cx).connection.is_ready();
        let d = self
            .state(cx)
            .current()
            .map(|cv| cv.diagnostics.clone())
            .unwrap_or_default();
        let row = |label: &str, value: String| {
            div()
                .flex()
                .justify_between()
                .gap(px(16.0))
                .child(
                    div()
                        .min_w_0()
                        .text_color(c.text_muted)
                        .child(label.to_owned()),
                )
                .child(div().font_family(t.mono_font()).child(value))
        };
        let opt = |v: Option<u64>| v.map_or_else(|| "—".to_owned(), format_ms);
        let refresh = {
            let this = this.clone();
            Btn::new("diagnostics-refresh")
                .label("Refresh")
                .kind(BtnKind::Subtle)
                .compact()
                .disabled(!real || d.loading)
                .aria("Refresh engine diagnostics")
                .on_click(move |_, cx| {
                    this.update(cx, |t, cx| t.dispatch(Command::RefreshDiagnostics, cx))
                })
        };
        let export = {
            let this = this.clone();
            Btn::new("diagnostics-export")
                .label("Export")
                .kind(BtnKind::Subtle)
                .compact()
                .disabled(!real || d.exporting || !d.available)
                .aria("Export engine diagnostics to a file")
                .on_click(move |_, cx| {
                    this.update(cx, |t, cx| t.dispatch(Command::ExportDiagnostics, cx))
                })
        };
        let mut section = div()
            .id("session-diagnostics")
            .role(Role::Region)
            .aria_label("Engine diagnostics")
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Engine diagnostics"),
                    )
                    .child(div().flex().gap(px(6.0)).child(refresh).child(export)),
            );
        if demo {
            return section.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child("Demo · simulated agent: no engine diagnostics."),
            );
        }
        if let Some(error) = &d.error {
            section = section.child(
                div()
                    .id("diagnostics-error")
                    .role(Role::Alert)
                    .text_size(t.small_size())
                    .text_color(c.warning)
                    .child(error.clone()),
            );
        }
        match &d.summary {
            None => {
                section = section.child(
                    div()
                        .id("diagnostics-state")
                        .role(Role::Status)
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child(if d.loading {
                            "Reading the engine’s records…"
                        } else {
                            "No diagnostics yet."
                        }),
                );
            }
            Some(s) => {
                section = section
                    .child(row(
                        "Runs · cancellations",
                        format!("{} · {}", s.runs, s.cancellations),
                    ))
                    .child(row(
                        "Model requests · errors · retries",
                        format!(
                            "{} · {} · {}",
                            s.provider_requests, s.provider_errors, s.retries
                        ),
                    ))
                    .child(row(
                        "Model request p50 · p95",
                        format!("{} · {}", opt(s.provider_p50_ms), opt(s.provider_p95_ms)),
                    ))
                    .child(row("First token p50", opt(s.first_token_p50_ms)))
                    .child(row(
                        "Tool calls · errors",
                        format!("{} · {}", s.tool_calls, s.tool_errors),
                    ))
                    .child(row(
                        "Tool call p50 · p95",
                        format!("{} · {}", opt(s.tool_p50_ms), opt(s.tool_p95_ms)),
                    ))
                    .child(row("Longest queue wait", opt(s.queue_wait_max_ms)))
                    .child(row("Longest Stop", opt(s.stop_latency_max_ms)))
                    .child(row("Records kept", s.records.to_string()));
                if !d.records.is_empty() {
                    section = section.child(
                        div()
                            .id("diagnostics-records")
                            .role(Role::List)
                            .aria_label("Newest diagnostic records")
                            .max_h(px(160.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .text_size(t.small_size())
                            .font_family(t.mono_font())
                            .text_color(c.text_muted)
                            .children(d.records.iter().map(|r| {
                                div()
                                    .id(("diagnostic-record", r.seq as usize))
                                    .role(Role::ListItem)
                                    .min_w_0()
                                    .child(format!("#{} {}", r.seq, r.line))
                            })),
                    );
                }
            }
        }
        if let Some(health) = self.state(cx).export_health.clone() {
            section = section.child(row("Export", health.summary()));
        }
        if let Some(exported) = &d.exported {
            section = section.child(
                div()
                    .id("diagnostics-exported")
                    .role(Role::Status)
                    .text_size(t.small_size())
                    .child(format!("Exported to {exported}")),
            );
        }
        section.child(
            div()
                .text_size(t.small_size())
                .text_color(c.text_faint)
                .child("Measured by the engine around model and tool calls. The window’s own input-to-frame latency is logged separately (Log performance stats)."),
        )
    }

    fn render_about(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        elevated(cx)
            .id("about")
            .key_context("Overlay")
            .track_focus(&self.menu_focus)
            .role(Role::Dialog)
            .aria_label("About Pipkin")
            .w(px(360.0 * t.scale.max(1.0)))
            .max_w_full()
            .p(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .occlude()
            .child(
                gpui::img(crate::assets::MASCOT)
                    .w(px(126.0 * t.scale.max(1.0)))
                    .h(px(131.0 * t.scale.max(1.0)))
                    .object_fit(gpui::ObjectFit::Contain),
            )
            .child(
                div()
                    .text_size(px(22.0 * t.scale))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Pipkin"),
            )
            .child(div().text_color(c.text_muted).child(format!(
                "Pipkin desktop · Version {}",
                env!("CARGO_PKG_VERSION")
            )))
            .child(
                Btn::new("about-website")
                    .label("Visit pipkinai.com")
                    .kind(BtnKind::Subtle)
                    .on_click(|_, cx| cx.open_url("https://pipkinai.com")),
            )
            .child(
                div().w_full().flex().justify_end().pt(px(8.0)).child(
                    Btn::new("about-back")
                        .label("Back to settings")
                        .kind(BtnKind::Primary)
                        .on_click(move |window, cx| {
                            this.update(cx, |t, cx| t.close_overlay(window, cx))
                        }),
                ),
            )
    }
}

/// The questionnaire's options in display order, as `(question id, option id)`, for the
/// keyboard's single highlighted row.
pub(super) fn form_options(q: &UiRequest) -> Vec<(String, String)> {
    q.form
        .as_ref()
        .map(|form| {
            form.questions
                .iter()
                .flat_map(|question| {
                    question
                        .options
                        .iter()
                        .map(move |o| (question.id.clone(), o.id.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A bordered text field inside a dialog.
fn field_box(editor: gpui::Entity<crate::text::ComposerEditor>, cx: &App) -> Div {
    let c = &cx.theme().colors;
    div()
        .px(px(10.0))
        .py(px(6.0))
        .min_h(px(34.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .bg(c.bg_input)
        .border_1()
        .border_color(c.border_strong)
        .child(div().flex_1().min_w_0().child(editor))
}

/// A bounded Markdown preview as blocks: headings, paragraphs, list items and code, in the
/// transcript's parser. Inline styles are not drawn; the text is shown as given.
fn preview_blocks(markdown: &str, t: &crate::theme::Theme) -> Div {
    use crate::transcript::markdown::{BlockKind, parse};
    let c = &t.colors;
    let mut out = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .py(px(4.0))
        .text_size(t.small_size());
    for block in parse(markdown) {
        out = out.child(match block.kind {
            BlockKind::Rule => div().my(px(4.0)).h(px(1.0)).w_full().bg(c.border_strong),
            BlockKind::Code { .. } => div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(c.bg_input)
                .font_family(t.mono_font())
                .child(block.text),
            BlockKind::Heading(_) => div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(block.text),
            BlockKind::ListItem => div()
                .pl(px(10.0 * (block.indent as f32 + 1.0)))
                .child(block.text),
            BlockKind::Paragraph => div()
                .when(block.quote, |d| d.text_color(c.text_muted))
                .child(block.text),
        });
    }
    out
}

#[cfg(test)]
mod form_tests {
    use super::*;

    #[test]
    fn the_keyboard_row_order_follows_the_questionnaire() {
        let q = UiRequest {
            id: "q1".into(),
            kind: UiRequestKind::Form,
            title: "t".into(),
            message: None,
            items: vec![],
            placeholder: None,
            default_value: None,
            deadline: None,
            source: None,
            form: Some(UiForm {
                allow_note: false,
                questions: vec![
                    UiFormQuestion {
                        id: "a".into(),
                        kind: UiFormKind::Single,
                        prompt: "A?".into(),
                        description: None,
                        preview: None,
                        options: vec![
                            UiFormOption {
                                id: "x".into(),
                                label: "X".into(),
                                description: None,
                                preview: None,
                            },
                            UiFormOption {
                                id: "y".into(),
                                label: "Y".into(),
                                description: None,
                                preview: None,
                            },
                        ],
                        allow_other: false,
                        allow_note: false,
                        default: vec![],
                        placeholder: None,
                    },
                    UiFormQuestion {
                        id: "b".into(),
                        kind: UiFormKind::Text,
                        prompt: "B?".into(),
                        description: None,
                        preview: None,
                        options: vec![],
                        allow_other: false,
                        allow_note: false,
                        default: vec![],
                        placeholder: None,
                    },
                    UiFormQuestion {
                        id: "c".into(),
                        kind: UiFormKind::Multiple,
                        prompt: "C?".into(),
                        description: None,
                        preview: None,
                        options: vec![UiFormOption {
                            id: "z".into(),
                            label: "Z".into(),
                            description: None,
                            preview: None,
                        }],
                        allow_other: false,
                        allow_note: false,
                        default: vec![],
                        placeholder: None,
                    },
                ],
            }),
        };
        let rows = form_options(&q);
        assert_eq!(
            rows,
            [
                ("a".to_owned(), "x".to_owned()),
                ("a".to_owned(), "y".to_owned()),
                ("c".to_owned(), "z".to_owned())
            ]
        );
        let mut plain = q.clone();
        plain.form = None;
        assert!(form_options(&plain).is_empty());
    }
}

#[cfg(test)]
mod session_format_tests {
    use super::*;

    #[test]
    fn external_profiles_never_display_native_zero_usage_as_cli_accounting() {
        let empty = Some(SessionUsage::default());
        assert!(native_usage(ExecutionProfile::NativeAgent, empty.clone()).is_some());
        assert!(native_usage(ExecutionProfile::LocalCliChatOnly, empty.clone()).is_none());
        assert!(native_usage(ExecutionProfile::LocalCliRestrictedWrite, empty.clone()).is_none());
        assert!(native_usage(ExecutionProfile::Unsupported, empty).is_none());
    }

    #[test]
    fn counts_and_unreported_costs_are_distinct() {
        assert_eq!(format_tokens(1_234_567), "1,234,567");
        assert_eq!(format_cost(Some(0.0)), "$0.0000");
        assert_eq!(format_cost(Some(0.00001)), "<$0.0001");
        assert_eq!(format_cost(None), "Not reported");
    }
}

#[cfg(test)]
mod submenu_tests {
    use super::{submenu_vertical, submenu_x};

    #[test]
    fn submenu_uses_upward_space_near_bottom_instead_of_a_squat_list() {
        assert_eq!(
            submenu_vertical(650.0, 720.0, 800.0, 473.0),
            (true, 720.0, 473.0)
        );
        assert_eq!(
            submenu_vertical(50.0, 120.0, 800.0, 473.0),
            (false, 50.0, 473.0)
        );
        assert_eq!(
            submenu_vertical(230.0, 300.0, 500.0, 473.0),
            (true, 300.0, 292.0)
        );
    }

    #[test]
    fn submenu_fits_inside_landscape_and_portrait_viewports() {
        for height in [360.0, 800.0, 1440.0] {
            for top in [8.0, height / 2.0, height - 80.0] {
                let (up, edge, room) = submenu_vertical(top, top + 70.0, height, 473.0);
                let (start, end) = if up {
                    (edge - room, edge)
                } else {
                    (edge, edge + room)
                };
                assert!(start >= 8.0);
                assert!(end <= height - 8.0);
            }
        }
    }

    #[test]
    fn submenu_opens_beside_parent_and_stays_visible_on_small_windows() {
        assert_eq!(submenu_x(240.0, 300.0, 300.0, 1440.0), 546.0);
        assert_eq!(submenu_x(570.0, 300.0, 300.0, 900.0), 264.0);
        assert_eq!(submenu_x(8.0, 300.0, 300.0, 500.0), 192.0);
    }
}

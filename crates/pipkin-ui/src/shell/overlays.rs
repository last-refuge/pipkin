use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px,
};
use pipkin_core::*;

use super::commands::{self, Cmd};
use super::controls::*;
use super::workspace::{Overlay, Workspace};
use crate::model::DemoControls;
use crate::theme::ActiveTheme;

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
            .filter_map(|c| {
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
            UiRequestKind::Input => vec![],
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

    fn render_model_settings(&mut self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let (model, effort, has_effort) = {
            let s = self.state(cx);
            let model = s
                .models
                .iter()
                .find(|m| Some(&m.id) == s.prefs.model.as_ref())
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "Choose model".into());
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
                [(0, "Model", model, true), (1, "Effort", effort, has_effort)]
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
        let usage = self.state(cx).current().and_then(|cv| cv.usage.clone());
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
            || "Session usage unavailable.".to_string(),
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
                            if demo {
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

#[cfg(test)]
mod session_format_tests {
    use super::*;

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

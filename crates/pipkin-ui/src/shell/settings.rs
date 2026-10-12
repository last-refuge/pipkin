//! A navigable Settings surface. Sections own their content, so new settings do not turn
//! the dialog into a long mixed list. Only the currently selected section is mounted.

use gpui::{
    Context, Div, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px,
};
use pipkin_core::{Command, Mode, TextSize, Theme as ThemeChoice};

use super::controls::{Btn, BtnKind, elevated, icon, menu_row, separator};
use super::workspace::{Overlay, Workspace};
use crate::theme::{ActiveTheme, Theme};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SettingsSection {
    #[default]
    Appearance,
    Session,
    Connections,
    Servers,
}

impl SettingsSection {
    const ALL: [Self; 4] = [
        Self::Appearance,
        Self::Session,
        Self::Connections,
        Self::Servers,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Session => "Session",
            Self::Connections => "Connections",
            Self::Servers => "MCP servers",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Appearance => "Make the workspace comfortable to read and use.",
            Self::Session => "Choose what Pi may do in this session.",
            Self::Connections => "Manage the accounts Pipkin uses for models.",
            Self::Servers => "Connect tool servers the model can search and call.",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "sun",
            Self::Session => "zap",
            Self::Connections => "wrench",
            Self::Servers => "terminal",
        }
    }
}

/// Keep every edge inside the viewport, including at the minimum supported window size.
fn settings_bounds(width: f32, height: f32, scale: f32) -> (f32, f32, bool) {
    let scale = scale.max(1.0);
    (
        (760.0 * scale).min((width - 24.0).max(0.0)),
        (540.0 * scale).min((height - 32.0).max(0.0)),
        width < 820.0 * scale,
    )
}

fn option(
    id: &'static str,
    group: &'static str,
    label: &'static str,
    selected: bool,
    command: Command,
    this: gpui::Entity<Workspace>,
) -> Btn {
    Btn::new(id)
        .label(label)
        .aria(format!(
            "{group}: {label}{}",
            if selected { ", current choice" } else { "" }
        ))
        .kind(if selected {
            BtnKind::Subtle
        } else {
            BtnKind::Ghost
        })
        .selected(selected)
        .on_click(move |_, cx| this.update(cx, |w, cx| w.dispatch(command.clone(), cx)))
}

/// One setting has one label, one consequence, and one group of controls. The compact layout
/// stacks the control below the explanation rather than squeezing it into a narrow right column.
fn setting_row(
    label: &'static str,
    detail: &'static str,
    controls: Vec<Btn>,
    compact: bool,
    t: &Theme,
) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .when(compact, |d| d.flex_col().gap(px(10.0)))
        .when(!compact, |d| {
            d.items_center().justify_between().gap(px(18.0))
        })
        .px(px(16.0))
        .py(px(14.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(div().font_weight(gpui::FontWeight::MEDIUM).child(label))
                .child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child(detail),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .gap(px(4.0))
                .children(controls),
        )
}

impl Workspace {
    fn settings_section_button(
        &self,
        section: SettingsSection,
        index: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let active = self.settings_section == section;
        let this = cx.entity();
        let keyboard_this = this.clone();
        menu_row(("settings-section", index), active, cx)
            .role(Role::Button)
            .aria_label(format!(
                "{} settings{}",
                section.title(),
                if active { ", selected" } else { "" }
            ))
            .on_click(move |_, _, cx| {
                this.update(cx, |w, cx| {
                    w.show_settings_section(section, cx);
                });
            })
            .on_key_down(move |event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    keyboard_this.update(cx, |w, cx| {
                        w.show_settings_section(section, cx);
                    });
                    cx.stop_propagation();
                }
            })
            .child(icon(
                section.icon(),
                px(16.0 * t.scale.max(1.0)),
                if active { c.accent } else { c.text_muted },
            ))
            .child(
                div()
                    .font_weight(if active {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if active { c.text } else { c.text_muted })
                    .child(section.title()),
            )
    }

    /// Selecting the servers section reads the engine's servers afresh.
    pub(super) fn show_settings_section(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = section;
        if section == SettingsSection::Servers {
            self.dispatch(Command::McpRefresh, cx);
        }
        cx.notify();
    }

    fn settings_page(&self, compact: bool, cx: &Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let this = cx.entity();
        let prefs = self.state(cx).prefs.clone();
        let row_compact = compact || t.scale > 1.0;
        let content = match self.settings_section {
            SettingsSection::Appearance => div()
                .rounded(px(8.0))
                .border_1()
                .border_color(c.border)
                .bg(c.bg_surface)
                .child(setting_row(
                    "Theme",
                    "Choose the contrast that works for your workspace.",
                    vec![
                        option(
                            "theme-dark",
                            "Theme",
                            "Dark",
                            prefs.theme == ThemeChoice::Dark,
                            Command::SetTheme(ThemeChoice::Dark),
                            this.clone(),
                        ),
                        option(
                            "theme-light",
                            "Theme",
                            "Light",
                            prefs.theme == ThemeChoice::Light,
                            Command::SetTheme(ThemeChoice::Light),
                            this.clone(),
                        ),
                    ],
                    row_compact,
                    &t,
                ))
                .child(separator(cx))
                .child(setting_row(
                    "Text size",
                    "Scale the interface and conversation together.",
                    vec![
                        option(
                            "size-small",
                            "Text size",
                            "Small",
                            prefs.text_size == TextSize::Small,
                            Command::SetTextSize(TextSize::Small),
                            this.clone(),
                        ),
                        option(
                            "size-normal",
                            "Text size",
                            "Normal",
                            prefs.text_size == TextSize::Normal,
                            Command::SetTextSize(TextSize::Normal),
                            this.clone(),
                        ),
                        option(
                            "size-large",
                            "Text size",
                            "Large",
                            prefs.text_size == TextSize::Large,
                            Command::SetTextSize(TextSize::Large),
                            this.clone(),
                        ),
                    ],
                    row_compact,
                    &t,
                ))
                .child(separator(cx))
                .child(setting_row(
                    "Reduced motion",
                    "Keep transitions and the caret calm.",
                    vec![
                        option(
                            "motion-off",
                            "Reduced motion",
                            "Off",
                            !prefs.reduced_motion,
                            Command::SetReducedMotion(false),
                            this.clone(),
                        ),
                        option(
                            "motion-on",
                            "Reduced motion",
                            "On",
                            prefs.reduced_motion,
                            Command::SetReducedMotion(true),
                            this.clone(),
                        ),
                    ],
                    row_compact,
                    &t,
                ))
                .into_any_element(),
            SettingsSection::Session => {
                let s = self.state(cx);
                let current = s.current();
                let enabled = current.and_then(|conv| conv.subagents.enabled);
                let ready = s.connection.is_ready();
                let detail = if s.mode == Mode::Demo {
                    "Not available in the simulated demo."
                } else if !ready {
                    "Reconnect to Pi to change this session's setting."
                } else if current.is_none() {
                    "Open a Pi session to choose whether it can delegate work."
                } else if enabled.is_none() {
                    "This session's subagent service is not available."
                } else {
                    "Off blocks new calls; children already running continue."
                };
                let disabled = s.mode == Mode::Demo || enabled.is_none() || !ready;
                let off = this.clone();
                let on = this.clone();
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.bg_surface)
                    .child(setting_row(
                        "Allow subagents",
                        detail,
                        vec![
                            Btn::new("subagents-off").label("Off")
                                .aria(format!("Block new subagent calls in this session{}", if enabled == Some(false) { ", current choice" } else { "" }))
                                .kind(if enabled == Some(false) { BtnKind::Subtle } else { BtnKind::Ghost })
                                .selected(enabled == Some(false)).disabled(disabled)
                                .on_click(move |_, cx| off.update(cx, |w, cx| w.dispatch(Command::SetSubagentsEnabled(false), cx))),
                            Btn::new("subagents-on").label("On")
                                .aria(format!("Allow new subagent calls in this session{}", if enabled == Some(true) { ", current choice" } else { "" }))
                                .kind(if enabled == Some(true) { BtnKind::Subtle } else { BtnKind::Ghost })
                                .selected(enabled == Some(true)).disabled(disabled)
                                .on_click(move |_, cx| on.update(cx, |w, cx| w.dispatch(Command::SetSubagentsEnabled(true), cx))),
                        ],
                        row_compact,
                        &t,
                    ))
                    .child(separator(cx))
                    .child(
                        div().flex().items_center().gap(px(8.0)).px(px(16.0)).py(px(12.0))
                            .child(icon("circle-help", px(14.0), c.text_muted))
                            .child(div().text_size(t.small_size()).text_color(c.text_muted)
                                .child("This choice belongs to the current Pi session, not all projects.")),
                    )
                    .into_any_element()
            }
            SettingsSection::Connections => {
                let this = this.clone();
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.bg_surface)
                    .child(setting_row(
                        "Model connections",
                        "Connect Claude or ChatGPT subscriptions through Pi.",
                        vec![Btn::new("prefs-connections")
                            .label("Manage…")
                            .trailing_icon("chevron-right")
                            .aria("Manage model connections in account setup")
                            .kind(BtnKind::Subtle)
                            .on_click(move |window, cx| {
                                this.update(cx, |w, cx| {
                                    w.close_overlay(window, cx);
                                    w.model.update(cx, |m, cx| m.manage_connections(cx));
                                });
                            })],
                        row_compact,
                        &t,
                    ))
                    .child(separator(cx))
                    .child(div().px(px(16.0)).py(px(12.0)).text_size(t.small_size())
                        .text_color(c.text_muted)
                        .child("Managing connections opens account setup. Return to the workspace when you’re done."))
                    .into_any_element()
            }
            SettingsSection::Servers => self.render_mcp_section(cx),
        };
        div()
            .id("settings-content")
            .role(Role::Region)
            .aria_label(format!("{} settings", self.settings_section.title()))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(if compact { 16.0 } else { 24.0 }))
            .py(px(22.0))
            .child(
                div()
                    .id("settings-page-title")
                    .role(Role::Heading)
                    .aria_label(self.settings_section.title())
                    .text_size(px(19.0 * t.scale))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.settings_section.title()),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .mb(px(20.0))
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(self.settings_section.subtitle()),
            )
            .child(content)
            .into_any_element()
    }

    pub(super) fn render_prefs(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let width = f32::from(window.viewport_size().width);
        let height = f32::from(window.viewport_size().height);
        let (dialog_width, dialog_height, compact) = settings_bounds(width, height, t.scale);
        let this = cx.entity();
        let close = this.clone();
        let about = this.clone();
        let nav = div()
            .id("settings-navigation")
            .role(Role::Navigation)
            .aria_label("Settings categories")
            .flex()
            .when(compact, |d| {
                d.flex_row()
                    .flex_none()
                    .gap(px(4.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(c.border)
            })
            .when(!compact, |d| {
                d.flex_col()
                    .flex_none()
                    .gap(px(4.0))
                    .w(px(182.0 * t.scale.max(1.0)))
                    .p(px(12.0))
                    .border_r_1()
                    .border_color(c.border)
            })
            .bg(c.bg_surface)
            .children(
                SettingsSection::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, section)| self.settings_section_button(section, index, cx)),
            );
        elevated(cx)
            .id("prefs")
            .key_context("Preferences")
            .track_focus(&self.menu_focus)
            .role(Role::Dialog)
            .aria_label("Settings")
            .w(px(dialog_width))
            .h(px(dialog_height))
            .max_w_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(24.0))
                    .py(px(17.0))
                    .border_b_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(21.0 * t.scale))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Settings"),
                            )
                            .child(
                                div()
                                    .text_size(t.small_size())
                                    .text_color(c.text_muted)
                                    .child("Shape your workspace and coding sessions."),
                            ),
                    )
                    .child(
                        Btn::new("prefs-close-top")
                            .icon("x")
                            .aria("Close settings")
                            .on_click(move |window, cx| {
                                close.update(cx, |w, cx| w.close_overlay(window, cx))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(compact, |d| d.flex_col())
                    .child(nav)
                    .child(self.settings_page(compact, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(20.0))
                    .py(px(12.0))
                    .border_t_1()
                    .border_color(c.border)
                    .child(
                        Btn::new("prefs-about")
                            .label("About Pipkin")
                            .kind(BtnKind::Ghost)
                            .on_click(move |window, cx| {
                                about.update(cx, |w, cx| w.open_overlay(Overlay::About, window, cx))
                            }),
                    )
                    .child(
                        Btn::new("prefs-close")
                            .label("Done")
                            .kind(BtnKind::Primary)
                            .on_click(move |window, cx| {
                                this.update(cx, |w, cx| w.close_overlay(window, cx))
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{SettingsSection, settings_bounds};

    #[test]
    fn settings_dialog_fits_small_and_scaled_viewports() {
        for (width, height, scale) in [
            (480.0, 480.0, 1.0),
            (720.0, 600.0, 1.2),
            (1440.0, 960.0, 1.0),
        ] {
            let (dialog_width, dialog_height, compact) = settings_bounds(width, height, scale);
            assert!(dialog_width <= width - 24.0);
            assert!(dialog_height <= height - 32.0);
            assert_eq!(compact, width < 820.0 * scale);
        }
        assert!(settings_bounds(480.0, 480.0, 1.0).2);
        assert!(!settings_bounds(1440.0, 960.0, 1.0).2);
    }

    #[test]
    fn settings_categories_have_unique_navigation_labels() {
        let labels: Vec<_> = SettingsSection::ALL
            .into_iter()
            .map(SettingsSection::title)
            .collect();
        assert_eq!(
            labels,
            ["Appearance", "Session", "Connections", "MCP servers"]
        );
    }
}

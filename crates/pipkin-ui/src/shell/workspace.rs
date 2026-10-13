use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, MouseUpEvent, ParentElement, PathPromptOptions,
    Render, ScrollHandle, Styled, Subscription, Task, UniformListScrollHandle, Window, deferred,
    div, prelude::*, px,
};
use pipkin_core::*;

use super::actions::*;
use super::clipboard_image;
use super::commands::Run;
use crate::model::{DemoControls, Model};
use crate::text::{ComposerEditor, ComposerEvent};
use crate::theme::{ActiveTheme, Theme};
use crate::transcript::TranscriptView;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    None,
    Palette,
    Model,
    ModelSettings,
    Effort,
    Project,
    Rename(ConversationId),
    Prefs,
    Session,
    About,
    /// A question an extension asked, for the first one waiting.
    Question,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Nav,
    Inspector,
}

#[derive(Clone, Copy)]
pub enum Split {
    Nav,
    Inspector,
}

struct DragGhost;
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub const MIN_CENTER: f32 = 480.0;
pub const DOCK_NAV_MIN_WIDTH: f32 = 900.0;
const DIVIDER_WIDTH: f32 = 5.0;
const MIN_INSPECTOR: f32 = 280.0;

// Keep the centre readable when a saved width comes from a larger monitor. These are
// display widths only: shrinking a window must not overwrite the user's saved preference.
fn docked_nav_width(total: f32, preferred: f32, inspector_open: bool) -> f32 {
    let nav = preferred.min(total - DIVIDER_WIDTH - MIN_CENTER);
    // If both panes can fit at their minimums, shrink the navigation first rather
    // than letting it push the enabled inspector's resize handle offscreen.
    if inspector_open && total >= 180.0 + MIN_INSPECTOR + MIN_CENTER + 2.0 * DIVIDER_WIDTH {
        nav.min(total - 2.0 * DIVIDER_WIDTH - MIN_CENTER - MIN_INSPECTOR)
    } else {
        nav
    }
}

fn docked_inspector_width(total: f32, nav: f32, preferred: f32) -> Option<f32> {
    let room = total - nav - 2.0 * DIVIDER_WIDTH - MIN_CENTER;
    (room >= MIN_INSPECTOR).then(|| preferred.clamp(MIN_INSPECTOR, room))
}

// The inspector can use all available space except the navigation pane and a readable centre.
fn inspector_drag_width(x: f32, total: f32, nav: f32) -> f32 {
    let available = total - nav - 2.0 * DIVIDER_WIDTH - MIN_CENTER;
    (total - x).clamp(MIN_INSPECTOR, available.max(MIN_INSPECTOR))
}

#[cfg(test)]
mod resize_tests {
    use super::{docked_inspector_width, docked_nav_width, inspector_drag_width};

    #[test]
    fn inspector_uses_available_room_instead_of_a_fixed_ceiling() {
        assert_eq!(inspector_drag_width(500.0, 1920.0, 240.0), 1190.0);
        assert_eq!(inspector_drag_width(1800.0, 1920.0, 240.0), 280.0);
        assert_eq!(inspector_drag_width(1100.0, 1200.0, 240.0), 280.0);
        assert_eq!(inspector_drag_width(0.0, 1200.0, 240.0), 470.0);
    }

    #[test]
    fn saved_large_monitor_width_shrinks_on_laptop_and_restores_when_widened() {
        assert_eq!(docked_inspector_width(1920.0, 240.0, 1100.0), Some(1100.0));
        assert_eq!(docked_inspector_width(1200.0, 240.0, 1100.0), Some(470.0));
        assert_eq!(docked_inspector_width(1010.0, 240.0, 1100.0), Some(280.0));
        assert_eq!(docked_nav_width(1009.0, 240.0, true), 239.0);
        assert_eq!(docked_inspector_width(1009.0, 239.0, 1100.0), Some(280.0));
        assert_eq!(docked_inspector_width(1920.0, 240.0, 1100.0), Some(1100.0));
    }

    #[test]
    fn both_dividers_and_a_dragged_navigation_leave_room_for_the_centre() {
        assert_eq!(docked_nav_width(900.0, 420.0, true), 415.0);
        assert_eq!(docked_inspector_width(1200.0, 420.0, 400.0), Some(290.0));
        assert_eq!(docked_nav_width(1050.0, 420.0, true), 280.0);
        assert_eq!(docked_inspector_width(1050.0, 280.0, 400.0), Some(280.0));
        assert_eq!(docked_nav_width(950.0, 420.0, true), 180.0);
        assert_eq!(docked_nav_width(949.0, 420.0, true), 420.0);
        assert_eq!(docked_inspector_width(949.0, 420.0, 400.0), None);
    }

    #[test]
    fn docked_splitters_stay_inside_the_viewport_at_every_width() {
        for total in 900..=2000 {
            let total = total as f32;
            for preferred_nav in [180.0, 240.0, 420.0] {
                let nav = docked_nav_width(total, preferred_nav, true);
                assert!(nav >= 180.0);
                if let Some(inspector) = docked_inspector_width(total, nav, 8192.0) {
                    assert!(inspector >= 280.0);
                    assert!(
                        nav + 2.0 * super::DIVIDER_WIDTH + inspector + super::MIN_CENTER <= total
                    );
                } else {
                    assert!(nav + super::DIVIDER_WIDTH + super::MIN_CENTER <= total);
                }
            }
        }
    }
}

pub struct Workspace {
    pub(super) model: Entity<Model>,
    pasted_image_dir: PathBuf,
    pub(super) transcript: Entity<TranscriptView>,
    pub(super) composer: Entity<ComposerEditor>,
    pub(super) nav_search: Entity<ComposerEditor>,
    pub(super) overlay_input: Entity<ComposerEditor>,
    pub(super) setup_input: Entity<ComposerEditor>,
    /// The MCP servers settings section's inputs and form choices.
    pub(super) mcp_name: Entity<ComposerEditor>,
    pub(super) mcp_target: Entity<ComposerEditor>,
    pub(super) mcp_values: Entity<ComposerEditor>,
    pub(super) mcp_import: Entity<ComposerEditor>,
    pub(super) mcp_answer: Entity<ComposerEditor>,
    pub(super) mcp_kind: pipkin_core::McpKind,
    pub(super) mcp_scope: pipkin_core::McpScope,
    pub(super) setup_challenge: Option<String>,
    pub(super) setup_project_edit: bool,
    pub(super) last_entry: onboarding::EntrySurface,
    pub(super) overlay: Overlay,
    pub(super) overlay_sel: usize,
    pub(super) settings_section: super::settings::SettingsSection,
    /// Questions the person put aside; they are not offered again on their own.
    pub(super) question_dismissed: Vec<String>,
    pub(super) restore_focus: Option<FocusHandle>,
    pub(super) temp_panel: Option<Panel>,
    pub(super) panel_restore: Option<FocusHandle>,
    pub(super) root_focus: FocusHandle,
    pub(super) menu_focus: FocusHandle,
    pub(super) panel_focus: FocusHandle,
    synced: (Option<ConversationId>, u64),
    flush_task: Option<Task<()>>,
    /// Saves the window size once resizing has settled.
    size_task: Option<Task<()>>,
    /// The conversation search box is showing (it also shows while it holds a query).
    pub(super) search_open: bool,
    pub(super) toast: Option<String>,
    toast_task: Option<Task<()>>,
    pub(super) diff_scroll: UniformListScrollHandle,
    pub(super) nav_scroll: ScrollHandle,
    /// The model/project menu's list, so the highlighted row is kept in view.
    pub(super) menu_scroll: ScrollHandle,
    /// Where the model button was last painted, so its menu opens right above it.
    pub(super) model_button: Option<gpui::Bounds<gpui::Pixels>>,
    pub(super) model_settings_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(super) changes_scroll: ScrollHandle,
    pub(super) diff_rows: DiffRows,
    pub(super) inspector_tab: super::subagents::InspectorTab,
    pub(super) subagent_rows: super::subagents::SubagentRows,
    pub(super) plan_rows: super::plan::PlanRows,
    pub(super) job_rows: super::jobs::JobRows,
    live_nav: Option<f32>,
    live_insp: Option<f32>,
    last_change_sel: Option<(ConversationId, Option<usize>)>,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
pub struct DiffRows {
    pub key: Option<(ConversationId, usize, u64)>,
    pub rows: Vec<(u32, u32)>,
    pub widest: usize,
    pub change: Option<std::rc::Rc<FileChange>>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.root_focus.clone()
    }
}

impl Workspace {
    pub fn new(
        model: Entity<Model>,
        data_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let transcript = cx.new(|cx| TranscriptView::new(model.clone(), window, cx));
        let composer = cx.new(|cx| ComposerEditor::new(window, cx));
        let nav_search = cx.new(|cx| ComposerEditor::single_line(window, cx));
        let overlay_input = cx.new(|cx| ComposerEditor::single_line(window, cx));
        let setup_input = cx.new(|cx| ComposerEditor::single_line(window, cx));
        setup_input.update(cx, |e, cx| {
            e.set_label("Sign-in response", cx);
            e.set_placeholder("Paste your code or redirect here");
        });
        let mcp_name = cx.new(|cx| ComposerEditor::single_line(window, cx));
        mcp_name.update(cx, |e, cx| {
            e.set_label("Server name", cx);
            e.set_placeholder("Server name (letters, digits, - _ .)");
        });
        let mcp_target = cx.new(|cx| ComposerEditor::single_line(window, cx));
        mcp_target.update(cx, |e, cx| {
            e.set_label("Command or URL", cx);
            e.set_placeholder("npx -y @modelcontextprotocol/server-filesystem /path");
        });
        let mcp_values = cx.new(|cx| ComposerEditor::new(window, cx));
        mcp_values.update(cx, |e, cx| {
            e.set_label("Environment or headers", cx);
            e.set_placeholder(
                "NAME=value, one per line (environment for commands, headers for URLs)",
            );
        });
        let mcp_import = cx.new(|cx| ComposerEditor::new(window, cx));
        mcp_import.update(cx, |e, cx| {
            e.set_label("Configuration to import", cx);
            e.set_placeholder("{ \"mcpServers\": { ... } }");
        });
        let mcp_answer = cx.new(|cx| ComposerEditor::single_line(window, cx));
        mcp_answer.update(cx, |e, cx| {
            e.set_label("MCP sign-in response", cx);
            e.set_placeholder("Paste the code or redirect URL here");
        });
        nav_search.update(cx, |e, cx| {
            e.set_placeholder("Search conversations");
            e.set_label("Search conversations", cx);
        });
        overlay_input.update(cx, |e, cx| {
            e.set_label("Command palette or dialog input", cx)
        });
        composer.update(cx, |e, _| e.set_placeholder("Ask anything\u{2026}"));
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&composer, window, Self::on_composer_event));
        subs.push(cx.subscribe_in(&nav_search, window, Self::on_search_event));
        subs.push(cx.subscribe_in(&overlay_input, window, Self::on_overlay_input_event));
        subs.push(cx.subscribe_in(&setup_input, window, |this, _, ev, _, cx| {
            if matches!(ev, ComposerEvent::Submit) {
                this.submit_sign_in_answer(cx);
            }
        }));
        subs.push(cx.subscribe_in(&mcp_answer, window, |this, _, ev, _, cx| {
            if matches!(ev, ComposerEvent::Submit) {
                this.submit_mcp_answer(cx);
            }
        }));
        subs.push(
            cx.observe_window_bounds(window, |this, window, cx| this.on_window_bounds(window, cx)),
        );
        subs.push(cx.observe_in(&model, window, |this, _, window, cx| {
            this.on_model(window, cx)
        }));
        let initial_entry = model.read(cx).entry;
        let mut ws = Workspace {
            model,
            pasted_image_dir: data_dir,
            transcript,
            composer,
            nav_search,
            overlay_input,
            setup_input,
            mcp_name,
            mcp_target,
            mcp_values,
            mcp_import,
            mcp_answer,
            mcp_kind: pipkin_core::McpKind::Stdio,
            mcp_scope: pipkin_core::McpScope::User,
            setup_challenge: None,
            setup_project_edit: false,
            last_entry: initial_entry,
            overlay: Overlay::None,
            overlay_sel: 0,
            settings_section: super::settings::SettingsSection::default(),
            question_dismissed: Vec::new(),
            restore_focus: None,
            temp_panel: None,
            panel_restore: None,
            root_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            panel_focus: cx.focus_handle(),
            synced: (None, u64::MAX),
            flush_task: None,
            size_task: None,
            search_open: false,
            toast: None,
            toast_task: None,
            diff_scroll: UniformListScrollHandle::new(),
            nav_scroll: ScrollHandle::new(),
            menu_scroll: ScrollHandle::new(),
            model_button: None,
            model_settings_bounds: None,
            changes_scroll: ScrollHandle::new(),
            diff_rows: DiffRows::default(),
            inspector_tab: super::subagents::InspectorTab::default(),
            subagent_rows: super::subagents::SubagentRows::default(),
            plan_rows: super::plan::PlanRows::default(),
            job_rows: super::jobs::JobRows::default(),
            live_nav: None,
            live_insp: None,
            last_change_sel: None,
            _subs: subs,
        };
        ws.on_model(window, cx);
        ws.focus_composer(window, cx);
        ws
    }

    pub(super) fn state<'a>(&self, cx: &'a App) -> &'a AppState {
        &self.model.read(cx).state
    }

    pub(super) fn dispatch(&self, command: Command, cx: &mut App) {
        // Opening a hit ends the search, so the box that held it is emptied too.
        let ends_search = matches!(command, Command::OpenSearchHit(_));
        self.model.update(cx, |m, cx| m.dispatch(command, cx));
        if ends_search {
            self.nav_search.update(cx, |e, cx| e.set_text("", cx));
        }
    }

    pub(super) fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        // The composer is absent on welcome and when no conversation is selected.
        // Focusing an unmounted editor disconnects all workspace keyboard actions.
        let h = if self.model.read(cx).entry == onboarding::EntrySurface::Workspace
            && self.state(cx).current().is_some()
        {
            self.composer.read(cx).focus_handle(cx)
        } else {
            self.root_focus.clone()
        };
        window.focus(&h, cx);
    }

    pub(super) fn show_toast(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.toast = Some(text.into());
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ------------------------------------------------------------ model → view

    fn on_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_subagent_rows(cx);
        self.refresh_plan_rows(cx);
        self.refresh_job_rows(cx);
        let entry = self.model.read(cx).entry;
        let challenge = self
            .model
            .read(cx)
            .sign_in
            .challenge
            .as_ref()
            .map(|q| q.id.clone());
        if self.setup_challenge != challenge {
            self.setup_challenge = challenge.clone();
            self.setup_input.update(cx, |e, cx| e.set_text("", cx));
            if challenge.is_some() && entry == onboarding::EntrySurface::Welcome {
                let focus = self.setup_input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
        }
        if entry != self.last_entry {
            self.last_entry = entry;
            self.focus_composer(window, cx);
        }
        let (prefs, current, title, run, change_sel) = {
            let s = self.state(cx);
            let c = s.current();
            (
                s.prefs.clone(),
                c.map(|c| (c.id, c.draft.sync_epoch, c.draft.text.clone())),
                c.map(|c| c.title.clone()),
                c.map(|c| c.run.clone()),
                c.map(|c| (c.id, c.selected_change)),
            )
        };
        let project_root = self
            .state(cx)
            .current_project()
            .map(|p| PathBuf::from(&p.path));
        // Real conversation creation is asynchronous. Keep the root usable while it is
        // pending, then focus the newly mounted composer without stealing an overlay's focus.
        if self.synced.0.is_none() && current.is_some() && self.root_focus.is_focused(window) {
            self.focus_composer(window, cx);
        }
        if current.is_none()
            && self.synced.0.is_some()
            && (self.composer.read(cx).focus_handle(cx).is_focused(window)
                || self.transcript.read(cx).focus_handle(cx).is_focused(window))
        {
            window.focus(&self.root_focus, cx);
        }
        self.composer
            .update(cx, |e, cx| e.set_project_root(project_root, cx));
        let wanted = Theme::new(prefs.theme, prefs.text_size, prefs.reduced_motion);
        let differs = cx
            .try_global::<Theme>()
            .map(|t| {
                t.choice != wanted.choice
                    || t.scale != wanted.scale
                    || t.reduced_motion != wanted.reduced_motion
            })
            .unwrap_or(true);
        if differs {
            cx.set_global(wanted);
            cx.refresh_windows();
        }
        match current {
            Some((id, epoch, text)) => {
                if self.synced != (Some(id), epoch) {
                    self.synced = (Some(id), epoch);
                    self.composer.update(cx, |e, cx| e.set_text(&text, cx));
                }
            }
            None => {
                if self.synced.0.is_some() || self.synced.1 == u64::MAX {
                    self.synced = (None, 0);
                    self.composer.update(cx, |e, cx| e.set_text("", cx));
                }
            }
        }
        let history_enabled = self
            .state(cx)
            .current()
            .is_some_and(|c| c.prompt_history.has_entries());
        let history_browsing = self
            .state(cx)
            .current()
            .is_some_and(|c| c.prompt_history.is_browsing());
        self.composer.update(cx, |e, _| {
            e.set_prompt_history_enabled(history_enabled);
            e.set_prompt_history_browsing(history_browsing);
        });
        // A conversation another engine owns is read here, not written: no draft to type.
        let owned = self
            .state(cx)
            .current()
            .is_some_and(|c| self.state(cx).owned_elsewhere(c.id));
        let locked = owned
            || matches!(
                run,
                Some(RunState::Submitting { .. } | RunState::OutcomeUnknown { .. }) | None
            );
        self.composer.update(cx, |e, cx| e.set_disabled(locked, cx));
        window.set_window_title(&match title {
            Some(t) => format!("{t} — Pipkin"),
            None => "Pipkin".to_string(),
        });
        // A change selected from the transcript must be visible even when the inspector is not
        // docked; this opens the temporary panel without touching transcript scroll or draft.
        if change_sel != self.last_change_sel {
            let was = self.last_change_sel;
            self.last_change_sel = change_sel;
            if let (Some((id, Some(_))), Some((pid, _))) = (change_sel, was)
                && id == pid
            {
                self.inspector_tab = super::subagents::InspectorTab::Changes;
            }
            if let (Some((id, Some(_))), Some((pid, _))) = (change_sel, was)
                && id == pid
                && !self.can_dock_inspector(window, cx)
                && self.temp_panel != Some(Panel::Inspector)
            {
                self.open_panel(Panel::Inspector, window, cx);
            }
        }
        cx.notify();
    }

    fn on_composer_event(
        &mut self,
        _: &Entity<ComposerEditor>,
        ev: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ComposerEvent::Changed => {
                let text = self.composer.read(cx).text();
                self.dispatch(Command::EditDraft(text), cx);
                self.schedule_flush(cx);
            }
            ComposerEvent::Up | ComposerEvent::Down => {
                self.dispatch(
                    if matches!(ev, ComposerEvent::Up) {
                        Command::PreviousPrompt
                    } else {
                        Command::NextPrompt
                    },
                    cx,
                );
                self.schedule_flush(cx);
            }
            ComposerEvent::Submit => self.submit_primary(window, cx),
            ComposerEvent::PasteImage(image) => self.paste_image(image.clone(), cx),
            ComposerEvent::Escape => {}
        }
    }

    fn paste_image(&mut self, image: gpui::Image, cx: &mut Context<Self>) {
        let Some(conversation) = self.state(cx).selected else {
            self.show_toast("Open a conversation before pasting an image", cx);
            return;
        };
        let dir = self.pasted_image_dir.clone();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { clipboard_image::save(&image, &dir) })
                .await;
            match saved {
                Ok(path) => {
                    let attached = this
                        .update(cx, |this, cx| {
                            if this.state(cx).selected != Some(conversation) {
                                this.show_toast(
                                    "Image not attached: conversation changed during paste",
                                    cx,
                                );
                                return false;
                            }
                            this.dispatch(
                                Command::AddAttachments(vec![describe_attachment(&path)]),
                                cx,
                            );
                            this.schedule_flush(cx);
                            this.show_toast("Image attached to draft", cx);
                            true
                        })
                        .unwrap_or(false);
                    if !attached {
                        cx.background_spawn(async move {
                            let _ = std::fs::remove_file(path);
                        })
                        .detach();
                    }
                }
                Err(message) => {
                    this.update(cx, |this, cx| this.show_toast(message, cx))
                        .ok();
                }
            }
        })
        .detach();
    }

    /// Enter: send when idle, steer when a run is active.
    pub(super) fn submit_primary(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        if text.trim() == "/session" {
            self.dispatch(Command::EditDraft(String::new()), cx);
            self.open_overlay(Overlay::Session, _window, cx);
            return;
        }
        if text.trim() == "/goal" {
            self.show_toast(
                "Usage: /goal <goal>, /goal pause, /goal resume or /goal clear",
                cx,
            );
            return;
        }
        if let Some(value) = text.trim().strip_prefix("/goal ") {
            if self.state(cx).selected.is_none() {
                self.show_toast("Open a conversation before setting a goal", cx);
                return;
            }
            let value = value.trim();
            if value.is_empty() {
                self.show_toast(
                    "Usage: /goal <goal>, /goal pause, /goal resume or /goal clear",
                    cx,
                );
                return;
            }
            let command = match value {
                "clear" => Command::ClearGoal,
                "pause" => Command::PauseGoal,
                "resume" => Command::ResumeGoal,
                "audit on" => Command::SetGoalAudit(true),
                "audit off" => Command::SetGoalAudit(false),
                "skip-audit" => Command::SkipGoalAudit,
                _ if value.starts_with("turns ") || value == "turns" => {
                    let n = value.trim_start_matches("turns").trim();
                    match n.parse::<u64>() {
                        Ok(n) => Command::SetGoalLimits {
                            max_turns: Some(n),
                            max_tokens: self
                                .state(cx)
                                .current()
                                .and_then(|c| c.goal.as_ref())
                                .and_then(|g| g.max_tokens),
                        },
                        Err(_) if n == "off" => Command::SetGoalLimits {
                            max_turns: None,
                            max_tokens: self
                                .state(cx)
                                .current()
                                .and_then(|c| c.goal.as_ref())
                                .and_then(|g| g.max_tokens),
                        },
                        Err(_) => {
                            self.show_toast("Usage: /goal turns <n> or /goal turns off", cx);
                            return;
                        }
                    }
                }
                _ if value.starts_with("tokens ") || value == "tokens" => {
                    let n = value.trim_start_matches("tokens").trim();
                    match n.parse::<u64>() {
                        Ok(n) => Command::SetGoalLimits {
                            max_turns: self
                                .state(cx)
                                .current()
                                .and_then(|c| c.goal.as_ref())
                                .and_then(|g| g.max_turns),
                            max_tokens: Some(n),
                        },
                        Err(_) if n == "off" => Command::SetGoalLimits {
                            max_turns: self
                                .state(cx)
                                .current()
                                .and_then(|c| c.goal.as_ref())
                                .and_then(|g| g.max_turns),
                            max_tokens: None,
                        },
                        Err(_) => {
                            self.show_toast("Usage: /goal tokens <n> or /goal tokens off", cx);
                            return;
                        }
                    }
                }
                _ if value.starts_with("criteria ") => {
                    let items: Vec<String> = value
                        .trim_start_matches("criteria")
                        .split('|')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if items.is_empty() {
                        self.show_toast("Usage: /goal criteria <one> | <two> | …", cx);
                        return;
                    }
                    Command::SetGoalCriteria(items)
                }
                _ => Command::SetGoal(value.to_string()),
            };
            self.dispatch(Command::EditDraft(String::new()), cx);
            self.dispatch(command, cx);
            return;
        }
        let a = self.state(cx).availability();
        if a.submit {
            self.dispatch(Command::Submit, cx);
        } else if a.steer {
            self.dispatch(Command::Steer, cx);
        }
    }

    /// Remember the window's size, once resizing has paused. The viewport is the real size:
    /// on Wayland `window_bounds()` calls an ordinary resize "maximized" and keeps the old
    /// size, so it cannot be trusted here. Fullscreen is the screen's size, not the person's
    /// choice, so it is skipped.
    fn on_window_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            return;
        }
        let viewport = window.viewport_size();
        let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
        let model = self.model.clone();
        self.size_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            model.update(cx, |m, cx| m.dispatch(Command::SetWindowSize(w, h), cx));
        }));
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state(cx).selected else {
            return;
        };
        let model = self.model.clone();
        self.flush_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            model.update(cx, |m, cx| m.dispatch(Command::FlushDraft(id), cx));
        }));
    }

    fn on_search_event(
        &mut self,
        _: &Entity<ComposerEditor>,
        ev: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ComposerEvent::Changed => {
                let q = self.nav_search.read(cx).text();
                self.dispatch(Command::SetSearch(q), cx);
            }
            ComposerEvent::Escape => {
                self.search_open = false;
                self.nav_search.update(cx, |e, cx| e.set_text("", cx));
                self.dispatch(Command::SetSearch(String::new()), cx);
                self.focus_composer(window, cx);
            }
            ComposerEvent::Submit | ComposerEvent::Down => {
                let first = self.state(cx).visible_conversations().first().map(|c| c.id);
                if let Some(id) = first {
                    self.dispatch(Command::SelectConversation(id), cx);
                    self.focus_composer(window, cx);
                }
            }
            _ => {}
        }
    }

    fn on_overlay_input_event(
        &mut self,
        _: &Entity<ComposerEditor>,
        ev: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ComposerEvent::Changed => {
                self.overlay_sel = 0;
                cx.notify();
            }
            ComposerEvent::Up => self.move_selection(-1, cx),
            ComposerEvent::Down => self.move_selection(1, cx),
            ComposerEvent::PasteImage(_) => {}
            ComposerEvent::Escape => self.close_overlay(window, cx),
            ComposerEvent::Submit => match self.overlay {
                Overlay::Rename(id) => {
                    let title = self.overlay_input.read(cx).text();
                    self.dispatch(Command::RenameConversation(id, title), cx);
                    self.close_overlay(window, cx);
                }
                Overlay::Question => self.confirm_selection(window, cx),
                _ => self.confirm_selection(window, cx),
            },
        }
    }

    // ------------------------------------------------------------- layout rules

    pub(super) fn nav_width(&self, cx: &App) -> f32 {
        self.live_nav.unwrap_or(self.state(cx).prefs.nav_width)
    }

    pub(super) fn inspector_width(&self, cx: &App) -> f32 {
        self.live_insp
            .unwrap_or(self.state(cx).prefs.inspector_width)
    }

    pub(super) fn nav_docked(&self, window: &Window) -> bool {
        f32::from(window.viewport_size().width) >= DOCK_NAV_MIN_WIDTH
    }

    fn can_dock_inspector(&self, window: &Window, cx: &App) -> bool {
        let w = f32::from(window.viewport_size().width);
        self.nav_docked(window)
            && docked_inspector_width(
                w,
                docked_nav_width(w, self.nav_width(cx), true),
                self.inspector_width(cx),
            )
            .is_some()
    }

    pub(super) fn inspector_docked(&self, window: &Window, cx: &App) -> bool {
        self.state(cx).prefs.inspector_open && self.can_dock_inspector(window, cx)
    }

    // ----------------------------------------------------------------- overlays

    pub(super) fn open_overlay(&mut self, o: Overlay, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::None {
            self.restore_focus = window.focused(cx);
        }
        let previous = self.overlay;
        if o == Overlay::Prefs && previous == Overlay::None {
            self.settings_section = super::settings::SettingsSection::default();
        }
        if o == Overlay::ModelSettings && !matches!(previous, Overlay::Model | Overlay::Effort) {
            self.model_settings_bounds = None;
        }
        self.overlay = o;
        self.overlay_sel = usize::from(o == Overlay::ModelSettings && previous == Overlay::Effort);
        match o {
            Overlay::Palette => {
                self.overlay_input.update(cx, |e, cx| {
                    e.set_text("", cx);
                    e.set_placeholder("Type a command…");
                });
                let h = self.overlay_input.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            }
            Overlay::Rename(id) => {
                let title = self
                    .state(cx)
                    .conversation(id)
                    .map(|c| c.title.clone())
                    .unwrap_or_default();
                self.overlay_input.update(cx, |e, cx| {
                    e.set_text(&title, cx);
                    e.set_placeholder("Conversation title");
                    e.select_all(cx);
                });
                let h = self.overlay_input.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            }
            Overlay::Model => {
                let cur = self.state(cx).prefs.model.clone();
                self.overlay_sel = self
                    .state(cx)
                    .models
                    .iter()
                    .position(|m| Some(&m.id) == cur.as_ref())
                    .unwrap_or(0);
                self.menu_scroll.scroll_to_item(self.overlay_sel);
                window.focus(&self.menu_focus, cx);
            }
            Overlay::Effort => {
                self.overlay_sel = self.state(cx).current().map_or(0, |c| {
                    c.thinking_levels
                        .iter()
                        .position(|l| Some(l) == c.thinking_level.as_ref())
                        .unwrap_or(0)
                });
                window.focus(&self.menu_focus, cx);
            }
            Overlay::ModelSettings
            | Overlay::Project
            | Overlay::Prefs
            | Overlay::Session
            | Overlay::About => window.focus(&self.menu_focus, cx),
            Overlay::Question => {
                let question = self.waiting_question(cx, true);
                match question {
                    Some(q) if q.kind == UiRequestKind::Input => {
                        self.overlay_input.update(cx, |e, cx| {
                            e.set_text(q.default_value.as_deref().unwrap_or(""), cx);
                            e.set_placeholder(q.placeholder.as_deref().unwrap_or("Your answer"));
                            e.select_all(cx);
                        });
                        let h = self.overlay_input.read(cx).focus_handle(cx);
                        window.focus(&h, cx);
                    }
                    _ => window.focus(&self.menu_focus, cx),
                }
            }
            Overlay::None => {}
        }
        cx.notify();
    }

    /// The first question an extension is waiting on in the open conversation. Questions put
    /// aside are skipped unless `including_dismissed`.
    pub(super) fn waiting_question(
        &self,
        cx: &App,
        including_dismissed: bool,
    ) -> Option<UiRequest> {
        self.state(cx).current().and_then(|c| {
            c.ui_requests
                .iter()
                .find(|q| including_dismissed || !self.question_dismissed.contains(&q.id))
                .cloned()
        })
    }

    /// Offer a waiting question without being asked, once.
    fn offer_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::None || self.waiting_question(cx, false).is_none() {
            return;
        }
        let this = cx.entity();
        window.defer(cx, move |window, cx| {
            this.update(cx, |t, cx| {
                if t.overlay == Overlay::None && t.waiting_question(cx, false).is_some() {
                    t.open_overlay(Overlay::Question, window, cx);
                }
            })
        });
    }

    pub(super) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::None {
            return;
        }
        if self.overlay == Overlay::Question
            && let Some(q) = self.waiting_question(cx, true)
            && !self.question_dismissed.contains(&q.id)
        {
            self.question_dismissed.push(q.id);
        }
        if self.overlay == Overlay::About {
            self.overlay = Overlay::Prefs;
            window.focus(&self.menu_focus, cx);
            cx.notify();
            return;
        }
        self.overlay = Overlay::None;
        match self.restore_focus.take() {
            Some(h) => window.focus(&h, cx),
            None => self.focus_composer(window, cx),
        }
        cx.notify();
    }

    pub(super) fn open_panel(&mut self, p: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if self.temp_panel.is_none() {
            self.panel_restore = window.focused(cx);
        }
        self.temp_panel = Some(p);
        window.focus(&self.panel_focus, cx);
        cx.notify();
    }

    pub(super) fn close_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.temp_panel.take().is_some() {
            match self.panel_restore.take() {
                Some(h) => window.focus(&h, cx),
                None => self.focus_composer(window, cx),
            }
            cx.notify();
        }
    }

    pub(super) fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let n = self.overlay_len(cx).max(1) as i32;
        self.overlay_sel = (self.overlay_sel as i32 + delta).rem_euclid(n) as usize;
        self.menu_scroll.scroll_to_item(self.overlay_sel);
        cx.notify();
    }

    // ------------------------------------------------------------------ actions

    pub(super) fn run_command(&mut self, run: &Run, window: &mut Window, cx: &mut Context<Self>) {
        match run {
            Run::Action(a) => {
                let a = a.boxed_clone();
                window.dispatch_action(a, cx);
            }
            Run::Dispatch(c) => self.dispatch(c.clone(), cx),
            Run::InspectSubagent(child) => {
                self.dispatch(Command::SelectSubagent(*child), cx);
                window.dispatch_action(Box::new(ShowSubagents), cx);
            }
            Run::CopyDiagnostics => {
                let report = self.model.read(cx).support_report().map(str::to_owned);
                if let Some(report) = report {
                    cx.write_to_clipboard(ClipboardItem::new_string(report));
                    self.show_toast(
                        "Copied support metadata only — no logs, paths or conversation text. Review before sharing.",
                        cx,
                    );
                } else {
                    self.show_toast("Support metadata is unavailable in this session", cx);
                }
            }
            Run::DemoScenario(name) => {
                if let Some(d) = cx.try_global::<DemoControls>().cloned() {
                    (d.set_scenario)(name);
                    self.show_toast(format!("Demo scenario: {name}"), cx);
                }
            }
            Run::ToggleSaveFailure => {
                if let Some(d) = cx.try_global::<DemoControls>().cloned() {
                    let on = !(d.save_failure)();
                    (d.set_save_failure)(on);
                    self.show_toast(
                        if on {
                            "Draft save failures injected"
                        } else {
                            "Draft saves restored"
                        },
                        cx,
                    );
                }
            }
        }
    }

    fn on_open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.open_overlay(Overlay::Palette, window, cx);
    }
    fn on_new_conversation(
        &mut self,
        _: &NewConversation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_panel(window, cx);
        self.dispatch(Command::NewConversation, cx);
        self.focus_composer(window, cx);
    }
    fn on_toggle_nav(&mut self, _: &ToggleNav, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_docked(window) {
            let h = self.nav_search.read(cx).focus_handle(cx);
            window.focus(&h, cx);
        } else if self.temp_panel == Some(Panel::Nav) {
            self.close_panel(window, cx);
        } else {
            self.open_panel(Panel::Nav, window, cx);
        }
    }
    fn on_toggle_inspector(
        &mut self,
        _: &ToggleInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_dock_inspector(window, cx) {
            let open = !self.state(cx).prefs.inspector_open;
            self.dispatch(Command::SetInspectorOpen(open), cx);
        } else if self.temp_panel == Some(Panel::Inspector) {
            self.close_panel(window, cx);
        } else {
            self.open_panel(Panel::Inspector, window, cx);
        }
    }
    fn on_show_subagents(
        &mut self,
        _: &ShowSubagents,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inspector_tab = super::subagents::InspectorTab::Subagents;
        if self.can_dock_inspector(window, cx) {
            self.dispatch(Command::SetInspectorOpen(true), cx);
        } else {
            self.open_panel(Panel::Inspector, window, cx);
        }
        cx.notify();
    }
    fn on_show_plan(&mut self, _: &ShowPlan, window: &mut Window, cx: &mut Context<Self>) {
        self.inspector_tab = super::subagents::InspectorTab::Plan;
        if self.can_dock_inspector(window, cx) {
            self.dispatch(Command::SetInspectorOpen(true), cx);
        } else {
            self.open_panel(Panel::Inspector, window, cx);
        }
        cx.notify();
    }
    fn on_focus_composer(
        &mut self,
        _: &FocusComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_composer(window, cx);
    }
    fn on_focus_transcript(
        &mut self,
        _: &FocusTranscript,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let h = if self.state(cx).current().is_some() {
            self.transcript.read(cx).focus_handle(cx)
        } else {
            self.root_focus.clone()
        };
        window.focus(&h, cx);
    }
    fn on_prefs(&mut self, _: &OpenPreferences, window: &mut Window, cx: &mut Context<Self>) {
        self.open_overlay(Overlay::Prefs, window, cx);
    }

    fn on_session(&mut self, _: &OpenSession, window: &mut Window, cx: &mut Context<Self>) {
        if self.state(cx).current().is_some() {
            self.open_overlay(Overlay::Session, window, cx);
        }
    }
    fn on_rename(&mut self, _: &RenameConversation, window: &mut Window, cx: &mut Context<Self>) {
        if self.state(cx).mode == Mode::Real {
            self.show_toast(
                "Renaming conversations is not supported by this engine.",
                cx,
            );
            return;
        }
        if let Some(id) = self.state(cx).selected {
            self.open_overlay(Overlay::Rename(id), window, cx);
        }
    }
    fn on_cancel(&mut self, _: &CancelRun, _: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Command::Cancel, cx);
    }
    fn on_queue(&mut self, _: &QueueFollowUp, _: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Command::QueueFollowUp, cx);
    }
    fn on_steer(&mut self, _: &SteerRun, _: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Command::Steer, cx);
    }
    fn on_jump(&mut self, _: &JumpToLatest, window: &mut Window, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |t, cx| t.jump_to_latest(window, cx));
    }
    fn on_model_menu(&mut self, _: &OpenModelMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.open_overlay(Overlay::Model, window, cx);
    }
    fn on_answer_question(
        &mut self,
        _: &AnswerQuestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.waiting_question(cx, true).is_some() {
            self.question_dismissed.clear();
            self.open_overlay(Overlay::Question, window, cx);
        }
    }
    fn on_open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        self.open_project(cx);
    }

    /// Choose a project folder and add it. The engine resolves a session's directory through
    /// symlinks, so the folder is resolved the same way here, which keeps one folder one project.
    pub(super) fn open_project(&mut self, cx: &mut Context<Self>) {
        if !self.state(cx).can_create {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = match rx.await {
                Ok(Ok(Some(p))) => p,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    this.update(cx, |this, cx| {
                        this.show_toast(format!("Folder picker unavailable: {e}"), cx)
                    })
                    .ok();
                    return;
                }
            };
            let Some(first) = paths.into_iter().next() else {
                return;
            };
            let resolved = cx
                .background_spawn(async move { std::fs::canonicalize(&first).map(|p| (first, p)) })
                .await;
            this.update(cx, |this, cx| match resolved {
                Ok((_, real)) if real.is_dir() => {
                    this.dispatch(Command::AddProject(real.display().to_string()), cx);
                    if this.model.read(cx).entry == onboarding::EntrySurface::Welcome
                        && this.model.read(cx).setup.started()
                        && this.model.read(cx).accepted_provider.is_some()
                    {
                        this.setup_project_edit = false;
                        this.dispatch(Command::NewConversation, cx);
                    }
                }
                Ok((chosen, _)) => {
                    this.show_toast(format!("{} is not a folder", chosen.display()), cx)
                }
                Err(e) => this.show_toast(format!("Cannot open that folder: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    fn on_attach(&mut self, _: &AttachFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.attach_files(cx);
    }
    fn on_next(&mut self, _: &NextConversation, _: &mut Window, cx: &mut Context<Self>) {
        self.step_conversation(1, cx);
    }
    fn on_prev(&mut self, _: &PrevConversation, _: &mut Window, cx: &mut Context<Self>) {
        self.step_conversation(-1, cx);
    }
    fn on_close_overlay(&mut self, _: &CloseOverlay, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::None {
            self.close_overlay(window, cx);
        } else {
            self.close_panel(window, cx);
        }
    }
    fn on_menu_up(&mut self, _: &MenuUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }
    fn on_menu_down(&mut self, _: &MenuDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }
    fn on_menu_confirm(&mut self, _: &MenuConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_selection(window, cx);
    }
    fn on_log_stats(&mut self, _: &LogStats, _: &mut Window, cx: &mut Context<Self>) {
        let s = self.transcript.read(cx).stats(cx);
        let rss_kb = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find(|l| l.starts_with("VmRSS"))
                    .and_then(|l| l.split_whitespace().nth(1).map(str::to_string))
            })
            .unwrap_or_default();
        if let Some(l) = crate::text::latency::stats() {
            log::warn!(
                "perf: input-to-frame (handler call to composer paint end; CPU, not display presentation) n={} p50={:?} p95={:?} max={:?}",
                l.count,
                l.p50,
                l.p95,
                l.max
            );
        }
        let msg = format!(
            "perf: transcript frames={} p50={:?} p95={:?} (CPU prepaint+paint, not display presentation) mounted_rows={} cached_blocks={} following={} rss_kb={}",
            s.frames,
            s.frame_p50,
            s.frame_p95,
            s.mounted_rows,
            s.cached_blocks,
            s.following,
            rss_kb
        );
        log::warn!("{msg}");
        self.show_toast("Performance stats written to the log", cx);
    }
    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn step_conversation(&mut self, delta: i32, cx: &mut Context<Self>) {
        let (ids, cur) = {
            let s = self.state(cx);
            (
                s.visible_conversations()
                    .iter()
                    .map(|c| c.id)
                    .collect::<Vec<_>>(),
                s.selected,
            )
        };
        if ids.is_empty() {
            return;
        }
        let pos = cur
            .and_then(|c| ids.iter().position(|i| *i == c))
            .unwrap_or(0) as i32;
        let next = ids[(pos + delta).rem_euclid(ids.len() as i32) as usize];
        self.dispatch(Command::SelectConversation(next), cx);
    }

    pub(super) fn attach_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach file references".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let paths = match result {
                Ok(Ok(Some(p))) => p,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    this.update(cx, |this, cx| {
                        this.show_toast(format!("File picker unavailable: {e}"), cx)
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |this, cx| this.attach_paths(paths, cx))
                .ok();
        })
        .detach();
    }

    /// Attach files by path: from the picker, or dropped onto the window. Folders cannot be
    /// attached; they are skipped with a word about it. The files are read off the UI thread.
    pub(super) fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        let (files, folders): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| !p.is_dir());
        if !folders.is_empty() {
            self.show_toast(
                if files.is_empty() {
                    "Folders cannot be attached; drop files instead".to_string()
                } else {
                    format!("Attached {} file(s); folders were skipped", files.len())
                },
                cx,
            );
        }
        if files.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let described = cx
                .background_spawn(async move {
                    files
                        .iter()
                        .map(|p| describe_attachment(p))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.dispatch(Command::AddAttachments(described), cx)
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn copy_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.show_toast("Draft copied to clipboard", cx);
    }

    // ------------------------------------------------------------------ resizing

    fn on_drag_move(&mut self, split: Split, x: f32, total: f32, cx: &mut Context<Self>) {
        match split {
            Split::Nav => {
                self.live_nav = Some(docked_nav_width(
                    total,
                    x.clamp(180.0, 420.0),
                    self.state(cx).prefs.inspector_open,
                ));
            }
            Split::Inspector => {
                self.live_insp = Some(inspector_drag_width(
                    x,
                    total,
                    docked_nav_width(total, self.nav_width(cx), true),
                ))
            }
        }
        cx.notify();
    }

    fn end_drag(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(w) = self.live_nav.take() {
            self.dispatch(Command::SetNavWidth(w), cx);
        }
        if let Some(w) = self.live_insp.take() {
            self.dispatch(Command::SetInspectorWidth(w), cx);
        }
    }

    pub(super) fn divider(
        &self,
        id: &'static str,
        split: Split,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().colors.border;
        let accent = cx.theme().colors.accent;
        let marker = match split {
            Split::Nav => 0usize,
            Split::Inspector => 1,
        };
        div()
            .id(id)
            .w(px(DIVIDER_WIDTH))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .flex()
            .justify_center()
            .hover(move |s| s.bg(accent.opacity(0.25)))
            .on_drag(DragMarker(marker), |_, _, _, cx| cx.new(|_| DragGhost))
            .child(div().w(px(1.0)).h_full().bg(border))
    }
}

#[cfg(test)]
mod diagnostics_tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    #[gpui::test]
    fn empty_workspace_and_welcome_exploration_keep_keyboard_actions(cx: &mut TestAppContext) {
        cx.update(crate::text::init);
        cx.update(super::super::actions::bind_keys);
        for (welcome, real) in [(false, false), (true, false), (false, true), (true, true)] {
            let window = cx.add_window(|window, cx| {
                let mut state = AppState::new(
                    Bootstrap {
                        projects: vec![Project {
                            id: ProjectId(1),
                            name: "test".into(),
                            path: "/synthetic-project".into(),
                        }],
                        models: vec![],
                        conversations: vec![],
                        now: 0,
                    },
                    Prefs {
                        selected_project: Some(ProjectId(1)),
                        ..Prefs::default()
                    },
                );
                if real {
                    state.mode = Mode::Real;
                    state.can_create = true;
                }
                let model = cx.new(|_| {
                    let mut model = Model::new(state);
                    if welcome {
                        model.entry = onboarding::EntrySurface::Welcome;
                    }
                    model
                });
                Workspace::new(model, PathBuf::new(), window, cx)
            });
            let root = window.root(cx).unwrap();
            let visual = VisualTestContext::from_window(window.into(), cx);
            let visual = &mut { visual };
            visual.run_until_parked();
            if welcome {
                root.update(visual, |workspace, cx| {
                    workspace
                        .model
                        .update(cx, |model, cx| model.explore_workspace(cx));
                });
                visual.run_until_parked();
            }
            // Neither the composer nor the transcript is mounted in an empty workspace.
            // Its own shortcuts and explicit focus shortcuts must all retain a live target.
            for focus_key in [None, Some("ctrl-l"), Some("ctrl-j")] {
                if let Some(key) = focus_key {
                    visual.simulate_keystrokes(key);
                    visual.run_until_parked();
                }
                visual.simulate_keystrokes("ctrl-k");
                visual.run_until_parked();
                assert_eq!(
                    root.read_with(visual, |workspace, _| workspace.overlay),
                    Overlay::Palette
                );
                visual.simulate_keystrokes("escape");
                visual.run_until_parked();
                assert_eq!(
                    root.read_with(visual, |workspace, _| workspace.overlay),
                    Overlay::None
                );
            }
            visual.simulate_keystrokes("ctrl-n");
            visual.run_until_parked();
            if real {
                assert!(root.read_with(visual, |workspace, cx| {
                    workspace.state(cx).current().is_none()
                }));
                root.update(visual, |workspace, cx| {
                    workspace.model.update(cx, |model, cx| {
                        model.mutate(
                            |state| {
                                state.apply_catalog(Bootstrap {
                                    projects: state.projects.clone(),
                                    models: vec![],
                                    conversations: vec![(
                                        ConversationId(1),
                                        ProjectId(1),
                                        "created".into(),
                                        1,
                                    )],
                                    now: 1,
                                })
                            },
                            cx,
                        );
                    });
                });
                visual.run_until_parked();
            }
            assert!(root.read_with(visual, |workspace, cx| {
                workspace.state(cx).current().is_some()
            }));
            visual.simulate_input("keyboard works");
            visual.run_until_parked();
            assert_eq!(
                root.read_with(visual, |workspace, cx| workspace.composer.read(cx).text()),
                "keyboard works"
            );
            root.update(visual, |workspace, cx| {
                workspace.model.update(cx, |model, cx| {
                    model.mutate(
                        |state| {
                            state.apply_catalog(Bootstrap {
                                projects: state.projects.clone(),
                                models: vec![],
                                conversations: vec![],
                                now: 2,
                            })
                        },
                        cx,
                    );
                });
            });
            visual.run_until_parked();
            visual.simulate_keystrokes("ctrl-k");
            visual.run_until_parked();
            assert_eq!(
                root.read_with(visual, |workspace, _| workspace.overlay),
                Overlay::Palette
            );
        }
    }

    #[gpui::test]
    fn composer_recalls_only_the_selected_sessions_prompts_and_restores_the_draft(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::text::init);
        let window = cx.add_window(|window, cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![Project {
                        id: ProjectId(1),
                        name: "test".into(),
                        path: "/synthetic-project".into(),
                    }],
                    models: vec![],
                    conversations: vec![
                        (ConversationId(1), ProjectId(1), "one".into(), 1),
                        (ConversationId(2), ProjectId(1), "two".into(), 2),
                    ],
                    now: 0,
                },
                Prefs::default(),
            );
            state.restore_prompt_history(ConversationId(1), vec!["alpha".into(), "beta".into()]);
            state.restore_prompt_history(ConversationId(2), vec!["gamma".into()]);
            state.dispatch(Command::SelectConversation(ConversationId(1)));
            let model = cx.new(|_| Model::new(state));
            Workspace::new(model, PathBuf::new(), window, cx)
        });
        let root = window.root(cx).unwrap();
        let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
        cx.run_until_parked();
        cx.simulate_input("unsent");
        for (key, expected) in [
            ("up", "beta"),
            ("up", "alpha"),
            ("down", "beta"),
            ("down", "unsent"),
        ] {
            cx.simulate_keystrokes(key);
            cx.run_until_parked();
            assert_eq!(
                root.read_with(cx, |w, cx| w.composer.read(cx).text()),
                expected
            );
        }
        root.update(cx, |w, cx| {
            w.dispatch(Command::SelectConversation(ConversationId(2)), cx)
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(
            root.read_with(cx, |w, cx| w.composer.read(cx).text()),
            "gamma"
        );
        root.update(cx, |w, cx| {
            w.dispatch(Command::SelectConversation(ConversationId(1)), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            root.read_with(cx, |w, cx| w.composer.read(cx).text()),
            "unsent"
        );
    }

    #[gpui::test]
    fn copies_controller_metadata_only_and_reports_unavailable_without_overwriting_clipboard(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::text::init);
        let window = cx.add_window(|window, cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![],
                    models: vec![],
                    conversations: vec![],
                    now: 0,
                },
                Prefs::default(),
            );
            state.set_storage_issue("sk-secret private error /private/project".into());
            let model = cx.new(|_| Model::new(state));
            Workspace::new(model, PathBuf::new(), window, cx)
        });
        let root = window.root(cx).unwrap();
        let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
        cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into())));
        root.update_in(cx, |workspace, window, cx| {
            workspace.run_command(&Run::CopyDiagnostics, window, cx);
            assert!(workspace.toast.as_deref().unwrap().contains("unavailable"));
        });
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("sentinel")
            )
        });
        root.update_in(cx, |workspace, window, cx| {
            workspace.model.update(cx, |model, _| {
                model.set_support_report("public metadata only".into())
            });
            workspace.run_command(&Run::CopyDiagnostics, window, cx);
            assert!(workspace.toast.as_deref().unwrap().contains("no logs"));
        });
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("public metadata only")
            )
        });
    }
}

#[derive(Clone)]
struct DragMarker(usize);

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.model.read(cx).entry == onboarding::EntrySurface::Welcome {
            return self.render_setup(window, cx).into_any_element();
        }
        self.offer_question(window, cx);
        let t = cx.theme().clone();
        let size = window.viewport_size();
        let width = f32::from(size.width);
        let nav_docked = self.nav_docked(window);
        let insp_docked = self.inspector_docked(window, cx);
        let nav_w = if nav_docked {
            docked_nav_width(
                width,
                self.nav_width(cx),
                self.state(cx).prefs.inspector_open,
            )
        } else {
            self.nav_width(cx)
        };
        let insp_w = self.inspector_width(cx);
        let docked_insp_w = docked_inspector_width(width, nav_w, insp_w);
        let temp = self.temp_panel;

        let mut row = div().flex().flex_row().size_full().min_h_0();
        if nav_docked {
            row = row.child(
                div()
                    .w(px(nav_w))
                    .h_full()
                    .flex_none()
                    .child(self.render_nav(window, cx)),
            );
            row = row.child(self.divider("nav-divider", Split::Nav, cx));
        }
        row = row.child(self.render_center(window, cx));
        if insp_docked {
            row = row.child(self.divider("inspector-divider", Split::Inspector, cx));
            row = row.child(
                div()
                    .w(px(docked_insp_w.expect("docked inspector has room")))
                    .h_full()
                    .flex_none()
                    .child(self.render_inspector(window, cx)),
            );
        }

        let scrim = t.colors.scrim;
        let mut root = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.root_focus)
            .relative()
            .size_full()
            .bg(t.colors.bg_app)
            .text_color(t.colors.text)
            .text_size(t.ui_size())
            .font_family(t.ui_font())
            .on_action(cx.listener(Self::on_open_palette))
            .on_action(cx.listener(Self::on_new_conversation))
            .on_action(cx.listener(Self::on_toggle_nav))
            .on_action(cx.listener(Self::on_toggle_inspector))
            .on_action(cx.listener(Self::on_show_subagents))
            .on_action(cx.listener(Self::on_show_plan))
            .on_action(cx.listener(Self::on_focus_composer))
            .on_action(cx.listener(Self::on_focus_transcript))
            .on_action(cx.listener(Self::on_prefs))
            .on_action(cx.listener(Self::on_session))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_queue))
            .on_action(cx.listener(Self::on_steer))
            .on_action(cx.listener(Self::on_jump))
            .on_action(cx.listener(Self::on_model_menu))
            .on_action(cx.listener(Self::on_attach))
            .on_action(cx.listener(Self::on_open_project))
            .on_action(cx.listener(Self::on_answer_question))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_close_overlay))
            .on_action(cx.listener(Self::on_menu_up))
            .on_action(cx.listener(Self::on_menu_down))
            .on_action(cx.listener(Self::on_menu_confirm))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_log_stats))
            .on_drag_move::<DragMarker>(cx.listener(
                move |this, ev: &gpui::DragMoveEvent<DragMarker>, _, cx| {
                    let x = f32::from(ev.event.position.x);
                    match ev.drag(cx).0 {
                        0 => this.on_drag_move(Split::Nav, x, width, cx),
                        _ => this.on_drag_move(Split::Inspector, x, width, cx),
                    }
                },
            ))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_drag))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_drag))
            .child(row);

        if let Some(panel) = temp {
            let content = match panel {
                Panel::Nav => self.render_nav(window, cx).into_any_element(),
                Panel::Inspector => self.render_inspector(window, cx).into_any_element(),
            };
            let pw = (if panel == Panel::Nav { nav_w } else { insp_w })
                .max(260.0)
                .min((width - 48.0).max(0.0));
            let drawer = div()
                .id("temp-panel")
                .key_context("Panel")
                .track_focus(&self.panel_focus)
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(pw))
                .bg(t.colors.bg_pane)
                .border_color(t.colors.border_strong)
                .when(panel == Panel::Nav, |d| d.left_0().border_r_1())
                .when(panel == Panel::Inspector, |d| d.right_0().border_l_1())
                .shadow_lg()
                .occlude()
                .child(content);
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .child(
                            div()
                                .id("panel-scrim")
                                .absolute()
                                .inset_0()
                                .bg(scrim)
                                .occlude()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.close_panel(window, cx)),
                                ),
                        )
                        .child(drawer),
                )
                .with_priority(5),
            );
        }

        if self.overlay != Overlay::None {
            root = root.child(deferred(self.render_overlay(window, cx)).with_priority(10));
        }
        if let Some(toast) = self.toast.clone() {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .bottom(px(24.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            super::controls::elevated(cx)
                                .px(px(14.0))
                                .py(px(8.0))
                                .child(toast),
                        ),
                )
                .with_priority(20),
            );
        }
        root.into_any_element()
    }
}

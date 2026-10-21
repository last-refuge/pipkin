//! Dashboard-local creation picker; execution reuses the correlated CLI setup flow.
use super::*;
use crate::shell::controls::{Btn, BtnKind, elevated};
use gpui::{IntoElement, ParentElement, PathPromptOptions, Role, Styled, div, prelude::*, px};
use pipkin_core::{CliMode, CliModel, LocalCliCommand, LocalCliDraft, LocalCliStage};

impl Workspace {
    pub(in crate::shell) fn open_dashboard_new(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let s = self.state(cx);
        if !s.dashboard.open {
            return;
        }
        // Reopen unfinished creation instead of replacing a pending check or receipt.
        if s.local_cli.dashboard_creation && s.local_cli.stage != LocalCliStage::Created {
            self.local_cli_advanced = false;
            self.open_overlay(Overlay::LocalCli, window, cx);
            return;
        }
        self.dashboard_new_draft = LocalCliDraft {
            project: s
                .dashboard
                .selected_row()
                .map(|r| r.worker.cwd.clone())
                .filter(|p| p.starts_with('/'))
                .or_else(|| s.current_project().map(|p| p.path.clone()))
                .or_else(|| s.projects.first().map(|p| p.path.clone()))
                .unwrap_or_default(),
            model: s.prefs.cli_model,
            mode: if s.prefs.cli_mode == CliMode::Orchestration {
                CliMode::Orchestration
            } else {
                CliMode::Coding
            },
            ..Default::default()
        };
        self.dashboard_new_generation += 1;
        self.open_overlay(Overlay::DashboardNew, window, cx);
    }

    fn start_dashboard_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::DashboardNew
            || !self.state(cx).dashboard.open
            || !self.state(cx).local_cli_availability().begin
            || !self.dashboard_new_draft.project.starts_with('/')
        {
            return;
        }
        self.dispatch(
            Command::LocalCli(LocalCliCommand::BeginDashboardNew(
                self.dashboard_new_draft.clone(),
            )),
            cx,
        );
        self.local_cli_advanced = false;
        self.open_overlay(Overlay::LocalCli, window, cx);
    }

    fn browse_dashboard_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.dashboard_new_draft.clone();
        let generation = self.dashboard_new_generation;
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = result.await else {
                return;
            };
            let Some(path) = paths.first().and_then(|p| p.to_str()).map(str::to_owned) else {
                return;
            };
            let _ = this.update_in(cx, |w, _, cx| {
                if w.overlay == Overlay::DashboardNew
                    && w.dashboard_new_generation == generation
                    && w.state(cx).dashboard.open
                    && w.dashboard_new_draft == original
                {
                    w.dashboard_new_draft.project = path;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::shell) fn reconcile_dashboard_new(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.dashboard_new_pending.clone() else {
            return;
        };
        if !self.state(cx).dashboard.open {
            self.dashboard_new_pending = None;
            return;
        }
        if self.overlay != Overlay::None {
            return;
        }
        if self
            .state(cx)
            .dashboard
            .rows
            .iter()
            .any(|r| r.worker.session == session)
        {
            self.dashboard_new_pending = None;
            self.dashboard
                .update(cx, |d, cx| d.open_flyup(session, window, cx));
        }
    }

    pub(in crate::shell) fn render_dashboard_new(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let s = self.state(cx);
        let available = s.local_cli_availability().begin;
        let draft = &self.dashboard_new_draft;
        let mut projects = div()
            .id("new-thread-project-list")
            .max_h(px(180.))
            .overflow_y_scroll()
            .overflow_x_scroll()
            .flex()
            .flex_col()
            .gap(px(6.));
        for project in &s.projects {
            let path = project.path.clone();
            projects = projects.child(
                Btn::new(format!("new-cli-project-{}", project.id.0))
                    .label(format!("{} · {}", project.name, project.path))
                    .selected(draft.project == path)
                    .tab_stop(true)
                    .on_click({
                        let this = cx.entity();
                        move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.dashboard_new_draft.project = path.clone();
                                cx.notify();
                            })
                        }
                    }),
            );
        }
        let mut models = div().flex().gap(px(8.));
        for (id, label, model) in [
            ("new-cli-haiku", "Haiku", CliModel::Haiku),
            ("new-cli-sonnet", "Sonnet", CliModel::Sonnet),
            ("new-cli-opus", "Opus", CliModel::Opus),
        ] {
            models = models.child(
                Btn::new(id)
                    .label(label)
                    .selected(draft.model == model)
                    .tab_stop(true)
                    .on_click({
                        let this = cx.entity();
                        move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.dashboard_new_draft.model = model;
                                cx.notify();
                            })
                        }
                    }),
            );
        }
        elevated(cx).id("dashboard-new-thread").occlude()
            .role(Role::Dialog).aria_label("New thread")
            .track_focus(&self.menu_focus.clone().tab_index(0).tab_stop(false))
            .tab_group().tab_stop(false).capture_key_down(cx.listener(Self::cli_dialog_key))
            .track_scroll(&self.cli_dialog_scroll)
            .w(px(620_f32.min((f32::from(window.viewport_size().width) - 32.).max(1.))))
            .max_h(px(650_f32.min((f32::from(window.viewport_size().height) - 32.).max(1.)))).overflow_y_scroll()
            .p(px(24.)).flex().flex_col().gap(px(16.))
            .child(div().flex().justify_between().child("New thread")
                .child(Btn::new("new-thread-close").label("Cancel").tab_stop(true)
                    .on_click({ let this = cx.entity(); move |window, cx| this.update(cx, |w, cx| w.close_overlay(window, cx)) })))
            .child("Choose a project and model. Your workspace map stays open.")
            .child(div().flex().gap(px(8.)).child("Provider")
                .child(Btn::new("new-thread-claude").label("Claude Code CLI").selected(true)))
            .child("Project").child(projects)
            .child(Btn::new("new-thread-browse").label("Choose another folder…").tab_stop(true)
                .on_click({ let this = cx.entity(); move |window, cx| this.update(cx, |w, cx| w.browse_dashboard_project(window, cx)) }))
            .child(if draft.project.is_empty() { "Choose a project folder to continue.".to_owned() } else { draft.project.clone() })
            .child("Model").child(models)
            .when(!available, |d| d.child(if s.local_cli.creation.is_some() && s.local_cli.stage != LocalCliStage::Created {
                "Finish or recover the pending Claude creation before starting another thread."
            } else { crate::shell::local_cli::availability_label(s) }))
            .child("Connection setup uses Claude Code’s login. No prompt is sent until you write one in the new thread.")
            .child(div().flex().justify_between()
                .child(Btn::new("new-thread-other-provider").label("Standard creation…").tab_stop(true)
                    .on_click({ let this = cx.entity(); move |window, cx| this.update(cx, |w, cx| {
                        w.close_overlay(window, cx);
                        w.dispatch(Command::Dashboard(d::Command::Close), cx);
                        w.begin_conversation(window, cx);
                    }) }))
                .child(Btn::new("new-thread-create").label("Create Claude thread").kind(BtnKind::Primary)
                    .disabled(!available || !draft.project.starts_with('/')).tab_stop(true)
                    .on_click({ let this = cx.entity(); move |window, cx| this.update(cx, |w, cx| w.start_dashboard_new(window, cx)) })))
    }
}

//! Dashboard-local provider/project/model picker with correlated native and CLI creation.
use super::*;
use crate::shell::controls::{Btn, BtnKind, elevated};
use gpui::{IntoElement, ParentElement, PathPromptOptions, Role, Styled, div, prelude::*, px};
use pipkin_core::{
    CliMode, CliModel, LocalCliCommand, LocalCliDraft, LocalCliStage, ProviderModel,
};

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
        if s.dashboard.creation.is_some() {
            self.open_overlay(Overlay::DashboardNew, window, cx);
            self.reconcile_dashboard_new(window, cx);
            return;
        }
        let choices = s.provider_models();
        self.dashboard_new_model = s
            .next_provider_model()
            .filter(|selection| choices.iter().any(|choice| &choice.selection == selection))
            .or_else(|| {
                s.models
                    .is_empty()
                    .then_some(ProviderModel::ClaudeCli(s.prefs.cli_model))
                    .filter(|selection| choices.iter().any(|choice| &choice.selection == selection))
            })
            .or_else(|| choices.first().map(|choice| choice.selection.clone()));
        let s = self.state(cx);
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
            || !self.dashboard_new_draft.project.starts_with('/')
        {
            return;
        }
        match self.dashboard_new_model.clone() {
            Some(ProviderModel::ClaudeCli(model))
                if self.state(cx).local_cli_availability().begin =>
            {
                self.dashboard_new_draft.model = model;
                self.dispatch(Command::Dashboard(d::Command::DismissCreation), cx);
                self.dispatch(
                    Command::LocalCli(LocalCliCommand::BeginDashboardNew(
                        self.dashboard_new_draft.clone(),
                    )),
                    cx,
                );
                self.local_cli_advanced = false;
                self.open_overlay(Overlay::LocalCli, window, cx);
            }
            Some(ProviderModel::Native(model)) => {
                self.dispatch(
                    Command::Dashboard(d::Command::CreateNative {
                        project: self.dashboard_new_draft.project.clone(),
                        model,
                    }),
                    cx,
                );
            }
            _ => {}
        }
    }

    fn browse_dashboard_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.dashboard_new_draft.clone();
        let original_model = self.dashboard_new_model.clone();
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
                    && w.dashboard_new_model == original_model
                    && !matches!(
                        w.state(cx).dashboard.creation,
                        Some(d::Creation::Pending(_))
                    )
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
        if let Some(d::Creation::Created(id)) = self.state(cx).dashboard.creation {
            if !self.state(cx).dashboard.open {
                self.dispatch(Command::Dashboard(d::Command::DismissCreation), cx);
            } else if self.overlay == Overlay::DashboardNew {
                let session = self
                    .state(cx)
                    .dashboard
                    .rows
                    .iter()
                    .find(|row| row.conversation == Some(id))
                    .map(|row| row.worker.session.clone());
                if let Some(session) = session {
                    self.dashboard_new_pending = Some(session);
                    self.dispatch(Command::Dashboard(d::Command::DismissCreation), cx);
                    self.close_overlay(window, cx);
                }
            }
        }
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
        let choices = s.provider_models();
        let cli = matches!(self.dashboard_new_model, Some(ProviderModel::ClaudeCli(_)));
        let busy = matches!(
            s.dashboard.creation,
            Some(d::Creation::Pending(_) | d::Creation::Created(_))
        );
        let available = !busy
            && if cli {
                s.local_cli_availability().begin
            } else {
                s.dashboard_native_creation_available()
            };
        let known_model = choices
            .iter()
            .any(|choice| Some(&choice.selection) == self.dashboard_new_model.as_ref());
        let draft = &self.dashboard_new_draft;
        let mut providers = div().flex().flex_wrap().gap(px(8.));
        let mut groups = Vec::new();
        for choice in &choices {
            let group = provider_group(&choice.selection);
            if groups.contains(&group) {
                continue;
            }
            groups.push(group.clone());
            let selection = choice.selection.clone();
            providers = providers.child(
                Btn::new(format!("new-provider-{}", groups.len() - 1))
                    .label(group.1.clone())
                    .selected(
                        self.dashboard_new_model
                            .as_ref()
                            .is_some_and(|m| provider_group(m) == group),
                    )
                    .disabled(busy)
                    .tab_stop(true)
                    .on_click({
                        let this = cx.entity();
                        move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.dashboard_new_model = Some(selection.clone());
                                cx.notify();
                            })
                        }
                    }),
            );
        }
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
                    .disabled(busy)
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
        let mut models = div()
            .id("new-thread-model-list")
            .max_h(px(160.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (index, choice) in choices.iter().enumerate().filter(|(_, choice)| {
            self.dashboard_new_model
                .as_ref()
                .is_some_and(|m| provider_group(m) == provider_group(&choice.selection))
        }) {
            let id = match choice.selection {
                ProviderModel::ClaudeCli(CliModel::Haiku) => "new-cli-haiku".to_owned(),
                ProviderModel::ClaudeCli(CliModel::Sonnet) => "new-cli-sonnet".to_owned(),
                ProviderModel::ClaudeCli(CliModel::Opus) => "new-cli-opus".to_owned(),
                _ => format!("new-native-model-{index}"),
            };
            let model = choice.selection.clone();
            models = models.child(
                Btn::new(id)
                    .label(choice.name.clone())
                    .hint(choice.detail.clone())
                    .selected(self.dashboard_new_model.as_ref() == Some(&model))
                    .disabled(busy)
                    .tab_stop(true)
                    .on_click({
                        let this = cx.entity();
                        move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.dashboard_new_model = Some(model.clone());
                                cx.notify();
                            })
                        }
                    }),
            );
        }
        let status = match &s.dashboard.creation {
            Some(d::Creation::Pending(_)) => "Creating your thread… You can close this dialog and return while it finishes.".to_owned(),
            Some(d::Creation::Created(_)) => "Thread created. Waiting for it to appear in the workspace map…".to_owned(),
            Some(d::Creation::Failed(message)) => message.clone(),
            None if choices.is_empty() => "No models are available. Connect a provider in Settings → Connections.".into(),
            None if !known_model => "This model is no longer available. Choose another model.".into(),
            None if !available && cli => crate::shell::local_cli::availability_label(s).into(),
            None if !available => "Creation is unavailable. Check your connection, workspace access, or any pending thread setup.".into(),
            _ if cli => "Connection setup uses Claude Code’s login. No prompt is sent until you write one in the new thread.".into(),
            _ => "Uses your connected provider. No prompt is sent until you write one in the new thread.".into(),
        };
        elevated(cx)
            .id("dashboard-new-thread")
            .occlude()
            .role(Role::Dialog)
            .aria_label("New thread")
            .track_focus(&self.menu_focus.clone().tab_index(0).tab_stop(false))
            .tab_group()
            .tab_stop(false)
            .capture_key_down(cx.listener(Self::cli_dialog_key))
            .track_scroll(&self.cli_dialog_scroll)
            .w(px(620_f32.min(
                (f32::from(window.viewport_size().width) - 32.).max(1.),
            )))
            .max_h(px(
                650_f32.min((f32::from(window.viewport_size().height) - 32.).max(1.))
            ))
            .overflow_y_scroll()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div().flex().justify_between().child("New thread").child(
                    Btn::new("new-thread-close")
                        .label(if busy { "Close" } else { "Cancel" })
                        .tab_stop(true)
                        .on_click({
                            let this = cx.entity();
                            move |window, cx| this.update(cx, |w, cx| w.close_overlay(window, cx))
                        }),
                ),
            )
            .child("Choose a project and model. Your workspace map stays open.")
            .child("Provider")
            .child(providers)
            .child("Project")
            .child(projects)
            .child(
                Btn::new("new-thread-browse")
                    .label("Choose another folder…")
                    .disabled(busy)
                    .tab_stop(true)
                    .on_click({
                        let this = cx.entity();
                        move |window, cx| {
                            this.update(cx, |w, cx| w.browse_dashboard_project(window, cx))
                        }
                    }),
            )
            .child(if draft.project.is_empty() {
                "Choose a project folder to continue.".to_owned()
            } else {
                draft.project.clone()
            })
            .child("Model")
            .child(models)
            .child(status)
            .child(
                div().flex().justify_end().child(
                    Btn::new("new-thread-create")
                        .label(if busy { "Creating…" } else { "Create thread" })
                        .kind(BtnKind::Primary)
                        .disabled(!available || !known_model || !draft.project.starts_with('/'))
                        .tab_stop(true)
                        .on_click({
                            let this = cx.entity();
                            move |window, cx| {
                                this.update(cx, |w, cx| w.start_dashboard_new(window, cx))
                            }
                        }),
                ),
            )
    }
}

/// Provider keys are presentation groups only; dispatch always uses the typed selection.
fn provider_group(model: &ProviderModel) -> (bool, String) {
    match model {
        ProviderModel::Native(id) => (
            false,
            id.split_once('/')
                .map_or("Native", |(provider, _)| provider)
                .to_owned(),
        ),
        ProviderModel::ClaudeCli(_) => (true, "Claude Code CLI".into()),
        ProviderModel::Unsupported => (false, "Unavailable".into()),
    }
}

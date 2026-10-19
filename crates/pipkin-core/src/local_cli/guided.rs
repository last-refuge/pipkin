//! Convenience orchestration is core state, never a render-time effect or saved grant.
use super::*;

impl AppState {
    /// Reopening Connections must not replace a model-picker intent with provider-only setup.
    pub fn pending_cli_conversation_setup(&self) -> bool {
        self.mode == Mode::Real
            && self.local_cli.guided
            && !self.local_cli.provider_only
            && self.local_cli.target == Some(LocalCliTarget::New)
            && self.local_cli.stage != LocalCliStage::Created
            && self.provider_origin_current(&self.local_cli.provider_origin)
    }

    pub(super) fn dispatch_guided_cli(&mut self, command: &LocalCliCommand) -> Option<Outcome> {
        use LocalCliCommand as C;
        let a = self.local_cli_availability();
        let mut out = Outcome::default();
        match command {
            C::BeginGuided | C::BeginProviderSetup if a.begin => {
                let provider_only = matches!(command, C::BeginProviderSetup);
                if provider_only && self.pending_cli_conversation_setup() {
                    return Some(out);
                }
                let origin = if provider_only {
                    None
                } else {
                    let model = crate::ProviderModel::ClaudeCli(self.prefs.cli_model);
                    self.prefs.provider_model = Some(model.clone());
                    self.mark_provider_origin(model, &mut out)
                };
                let draft = LocalCliDraft {
                    project: self
                        .current_project()
                        .map(|p| p.path.clone())
                        .unwrap_or_default(),
                    model: self.prefs.cli_model,
                    mode: if self.prefs.cli_mode == CliMode::Orchestration {
                        CliMode::Orchestration
                    } else {
                        CliMode::Coding
                    },
                    ..Default::default()
                };
                if self.local_cli.target.is_some() && self.local_cli.stage != LocalCliStage::Created
                {
                    self.local_cli_cancel(&mut out);
                }
                out.merge(self.dispatch_local_cli(C::BeginNew(draft)));
                self.local_cli.guided = true;
                self.local_cli.provider_only = provider_only;
                self.local_cli.provider_origin = origin;
                self.local_cli.candidates.clear();
                out.merge(self.dispatch_local_cli(C::Discover));
            }
            C::BeginGuidedExisting {
                conversation,
                session,
            } if a.begin => {
                if self.local_cli.target.is_some() && self.local_cli.stage != LocalCliStage::Created
                {
                    self.local_cli_cancel(&mut out);
                }
                out.merge(self.dispatch_local_cli(C::BeginExisting {
                    conversation: *conversation,
                    session: session.clone(),
                }));
                if matches!(&self.local_cli.target, Some(LocalCliTarget::Existing { conversation: c, session: s, .. }) if c == conversation && s == session)
                {
                    self.local_cli.guided = true;
                    self.local_cli.provider_only = false;
                    self.local_cli.provider_origin = None;
                    self.local_cli.candidates.clear();
                    out.merge(self.dispatch_local_cli(C::SessionStatus));
                }
            }
            C::Discover
                if self.local_cli.guided
                    && a.inspect
                    && matches!(self.local_cli.target, Some(LocalCliTarget::New)) =>
            {
                let mut draft = self.local_cli.draft.clone();
                draft.executable.clear();
                // Rediscovery retires the previous ticket/consent and stale picker results,
                // but preserves whether this is provider configuration or conversation setup.
                out.merge(self.dispatch_local_cli(C::Edit(draft)));
                self.local_cli.discovering = true;
                self.local_cli.candidates.clear();
                self.local_cli_send(
                    LocalCliRequest::Discover {
                        saved: self.prefs.cli_executable.clone(),
                        project: self.local_cli.draft.project.clone(),
                    },
                    &mut out,
                );
            }
            C::Configure(draft)
                if self.local_cli.guided
                    && a.edit
                    && matches!(self.local_cli.target, Some(LocalCliTarget::New)) =>
            {
                out.merge(self.dispatch_local_cli(C::Edit(draft.clone())));
                if draft.executable.is_empty() {
                    out.merge(self.dispatch_local_cli(C::Discover));
                } else {
                    out.merge(self.dispatch_local_cli(C::Inspect));
                }
            }
            C::ConfirmAndConnect { setup }
                if self.local_cli.guided
                    && a.trust
                    && self
                        .local_cli
                        .ticket
                        .as_ref()
                        .is_some_and(|t| &t.setup == setup) =>
            {
                self.local_cli.trust_executable = true;
                self.local_cli.trust_project = true;
                self.local_cli.connect_after_check = true;
                self.prefs.cli_executable =
                    self.local_cli.ticket.as_ref().map(|t| t.executable.clone());
                self.prefs.cli_model = self.local_cli.draft.model;
                self.prefs.cli_mode = self.local_cli.draft.mode;
                out.effects.push(Effect::SavePrefs(self.prefs.clone()));
                out.merge(self.dispatch_local_cli(C::Check));
            }
            C::BeginGuided
            | C::BeginProviderSetup
            | C::BeginGuidedExisting { .. }
            | C::Discover
            | C::Configure(_)
            | C::ConfirmAndConnect { .. } => return Some(out),
            _ => return None,
        }
        out.notes.push(Note::Other);
        Some(out)
    }

    pub(super) fn advance_guided_cli(
        &mut self,
        discovered: bool,
        inspected: bool,
        hints: bool,
        checked: bool,
        out: &mut Outcome,
    ) {
        if !self.local_cli.guided {
            return;
        }
        if discovered
            && (self.local_cli.candidates.len() == 1
                || (self.prefs.cli_provider_configured
                    && self.prefs.cli_executable.as_deref()
                        == Some(self.local_cli.draft.executable.as_str())
                    && self
                        .local_cli
                        .candidates
                        .contains(&self.local_cli.draft.executable)))
        {
            out.merge(self.dispatch_local_cli(LocalCliCommand::Inspect));
        }
        if hints
            && let Some(view) = self.local_cli.session_status.clone()
            && let Some(model) = view.model
            && view.mode != CliMode::Unsupported
        {
            out.merge(
                self.dispatch_local_cli(LocalCliCommand::Edit(LocalCliDraft {
                    thinking: None,
                    executable: view.executable,
                    project: view.project,
                    mode: view.mode,
                    model,
                })),
            );
            out.merge(self.dispatch_local_cli(LocalCliCommand::Inspect));
        }
        if hints
            && self
                .local_cli
                .session_status
                .as_ref()
                .is_some_and(|view| view.model.is_none() || view.mode == CliMode::Unsupported)
        {
            self.local_cli.stage = LocalCliStage::Failed;
            self.local_cli.error = Some(LocalCliFailure::Rejected("This saved Claude configuration is unsupported; it cannot be reconnected or migrated automatically.".into()));
        }
        // Selecting a configured provider is the user's request to connect it, as for
        // native providers. Discovery alone, changed paths and status/unknown replies
        // never execute candidates or resume an interrupted setup.
        if inspected
            && !self.local_cli.provider_only
            && self.prefs.cli_provider_configured
            && self.local_cli.draft.mode == CliMode::Coding
            && self.local_cli.stage == LocalCliStage::AwaitingTrust
            && let Some(ticket) = self.local_cli.ticket.clone()
            && self.prefs.cli_executable.as_deref() == Some(ticket.executable.as_str())
        {
            out.merge(self.dispatch_local_cli(LocalCliCommand::ConfirmAndConnect {
                setup: ticket.setup,
            }));
        }
        // Status lookup is never permission to continue an interrupted workflow.
        if checked && self.local_cli.connect_after_check {
            self.local_cli.connect_after_check = false;
            if self.local_cli.stage == LocalCliStage::Verified {
                self.prefs.cli_provider_configured = true;
                if self.local_cli.provider_only {
                    out.effects.push(Effect::SavePrefs(self.prefs.clone()));
                    return;
                }
                let next = match self.local_cli.target {
                    Some(LocalCliTarget::New) => LocalCliCommand::Create,
                    Some(LocalCliTarget::Existing { .. }) => LocalCliCommand::ReturnToConversation,
                    None => return,
                };
                let returning = matches!(next, LocalCliCommand::ReturnToConversation);
                out.merge(self.dispatch_local_cli(next));
                if returning {
                    self.local_cli.stage = LocalCliStage::Created;
                }
                out.effects.push(Effect::SavePrefs(self.prefs.clone()));
            }
        }
    }
}

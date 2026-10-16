//! Shared provider/model catalog and conversation-scoped selection, independent of transport.
use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderModel {
    Native(String),
    ClaudeCli(CliModel),
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderModelChoice {
    pub selection: ProviderModel,
    pub name: String,
    pub detail: String,
}

impl AppState {
    /// Native model IDs and CLI aliases occupy different typed namespaces. A native provider
    /// with a CLI-looking name is still native; no prefix/string-based routing exists.
    pub fn provider_models(&self) -> Vec<ProviderModelChoice> {
        let mut rows: Vec<_> = self
            .models
            .iter()
            .map(|m| ProviderModelChoice {
                selection: ProviderModel::Native(m.id.clone()),
                name: m.name.clone(),
                detail: m.note.clone(),
            })
            .collect();
        if self.mode == Mode::Real
            && (self.local_cli.available
                || self.prefs.cli_provider_configured
                || matches!(self.prefs.provider_model, Some(ProviderModel::ClaudeCli(_))))
        {
            for (model, name) in [
                (CliModel::Haiku, "Haiku"),
                (CliModel::Sonnet, "Sonnet"),
                (CliModel::Opus, "Opus"),
            ] {
                rows.push(ProviderModelChoice {
                    selection: ProviderModel::ClaudeCli(model),
                    name: format!("Claude Code CLI · {name}"),
                    detail: if self.local_cli.available { "CLI login · fixed model · chat or approved Write only; connection review in Settings" } else { "CLI provider unavailable on this engine; no fallback" }.into(),
                });
            }
        }
        rows
    }

    pub fn provider_picker_available(&self) -> bool {
        !self.provider_models().is_empty() && self.connection.is_ready() && self.read_only.is_none()
    }

    pub fn selected_provider_model(&self) -> Option<ProviderModel> {
        self.current()
            .and_then(|c| c.provider_model.clone())
            .or_else(|| {
                if self.current_execution_profile() == Some(ExecutionProfile::NativeAgent)
                    || self.mode == Mode::Demo
                {
                    self.prefs.model.clone().map(ProviderModel::Native)
                } else {
                    None
                }
            })
    }

    /// New follows this conversation's model (or its explicit pending change), not another
    /// conversation's last choice. Only a blank workspace uses the saved last choice.
    pub fn next_provider_model(&self) -> Option<ProviderModel> {
        if self.prefs.pending_provider_models.contains_key(&0) {
            return Some(ProviderModel::Unsupported);
        }
        if let Some(c) = self.current() {
            if let Some(model) = self.prefs.pending_provider_models.get(&c.id.0) {
                return Some(model.clone());
            }
            if let Some(model) = self.selected_provider_model() {
                return Some(model);
            }
            if c.profile.is_local_cli() {
                return Some(ProviderModel::Unsupported);
            }
            if c.execution_profile(self.mode) == ExecutionProfile::NativeAgent {
                return None; // Native default, never another conversation's saved CLI choice.
            }
        }
        self.prefs.provider_model.clone()
    }

    pub fn provider_selection_pending(&self, id: ConversationId) -> bool {
        self.prefs.pending_provider_models.contains_key(&id.0)
            || self.prefs.pending_provider_models.contains_key(&0)
    }

    pub fn provider_selection_notice(&self) -> Option<&'static str> {
        self.current().filter(|c| self.provider_selection_pending(c.id)).map(|_| {
            "Provider change is not connected yet. This conversation keeps its original provider and draft; nothing will be sent as a fallback. Finish setup in Connections or explicitly keep this conversation's provider. Other conversations are unaffected."
        })
    }

    pub(crate) fn mark_provider_origin(
        &mut self,
        model: ProviderModel,
        out: &mut Outcome,
    ) -> Option<(ConversationId, ProviderModel)> {
        let id = self.selected?;
        // Ordinary New with the same model must not stop an already submitted source prompt.
        if self.selected_provider_model().as_ref() == Some(&model)
            && !self.provider_selection_pending(id)
        {
            return None;
        }
        if self.selected_provider_model().as_ref() != Some(&model)
            && let Some(intent) = self
                .conversations
                .iter_mut()
                .find(|c| c.id == id)
                .and_then(|c| c.pending_intent.as_mut())
        {
            intent.provider_changed = true;
        }
        self.prefs
            .pending_provider_models
            .insert(id.0, model.clone());
        out.effects.push(Effect::SavePrefs(self.prefs.clone()));
        out.notes.push(Note::Other);
        Some((id, model))
    }

    pub(crate) fn provider_origin_current(
        &self,
        origin: &Option<(ConversationId, ProviderModel)>,
    ) -> bool {
        origin
            .as_ref()
            .is_none_or(|(id, model)| self.prefs.pending_provider_models.get(&id.0) == Some(model))
    }

    pub(crate) fn finish_provider_origin(
        &mut self,
        origin: Option<(ConversationId, ProviderModel)>,
        out: &mut Outcome,
    ) {
        if let Some((id, model)) = origin
            && self.prefs.pending_provider_models.get(&id.0) == Some(&model)
        {
            self.prefs.pending_provider_models.remove(&id.0);
            out.effects.push(Effect::SavePrefs(self.prefs.clone()));
            out.notes.push(Note::Other);
        }
    }

    /// Persist one final preference snapshot, never an intermediate cleared source guard.
    pub(crate) fn coalesce_provider_prefs(&self, out: &mut Outcome) {
        if out
            .effects
            .iter()
            .any(|e| matches!(e, Effect::SavePrefs(_)))
        {
            out.effects.retain(|e| !matches!(e, Effect::SavePrefs(_)));
            out.effects.push(Effect::SavePrefs(self.prefs.clone()));
        }
    }

    pub(crate) fn select_provider_model(&mut self, model: ProviderModel) -> Outcome {
        let mut out = self.select_provider_model_inner(model);
        self.coalesce_provider_prefs(&mut out);
        out
    }

    fn select_provider_model_inner(&mut self, model: ProviderModel) -> Outcome {
        let mut out = Outcome::default();
        if self.mode == Mode::Demo && !matches!(model, ProviderModel::Native(_)) {
            return out;
        }
        // A provider selection may create a new session, never overlap an unresolved creation.
        if self.pending_native_creation()
            || !self.local_cli_availability().edit
                && self.local_cli.creation.is_some()
                && self.local_cli.stage != LocalCliStage::Created
        {
            self.notice = Some(
                "Resolve the pending conversation creation before choosing another provider."
                    .into(),
            );
            out.notes.push(Note::Other);
            return out;
        }
        self.prefs.provider_model = Some(model.clone());
        out.effects.push(Effect::SavePrefs(self.prefs.clone()));
        if !self.provider_picker_available()
            || !self.provider_models().iter().any(|m| m.selection == model)
        {
            self.mark_provider_origin(model, &mut out);
            self.notice = Some(
                "The selected provider/model is unavailable. No other model was substituted."
                    .into(),
            );
            out.notes.push(Note::Other);
            return out;
        }
        if self.selected_provider_model().as_ref() == Some(&model) {
            out.merge(self.dispatch(Command::KeepConversationProvider));
            return out;
        }
        match model {
            ProviderModel::Native(id)
                if self.mode == Mode::Demo
                    || self.current_execution_profile() == Some(ExecutionProfile::NativeAgent) =>
            {
                out.merge(self.dispatch(Command::KeepConversationProvider));
                if self.mode == Mode::Real {
                    self.mark_provider_origin(ProviderModel::Native(id.clone()), &mut out);
                }
                out.merge(self.dispatch(Command::SetModel(id)));
            }
            ProviderModel::Native(_) => {
                self.mark_provider_origin(model, &mut out);
                out.merge(self.dispatch(Command::NewConversation));
            }
            ProviderModel::ClaudeCli(alias) => {
                self.prefs.cli_model = alias;
                self.mark_provider_origin(ProviderModel::ClaudeCli(alias), &mut out);
                // BeginGuided captures the pending source and selected alias. It never probes.
                out.merge(self.dispatch(Command::LocalCli(LocalCliCommand::BeginGuided)));
            }
            ProviderModel::Unsupported => {}
        }
        out.notes.push(Note::Other);
        out
    }
}

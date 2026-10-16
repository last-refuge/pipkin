//! The GPUI entity that owns `AppState` and connects it to the controller's effect handler.

use gpui::{Context, EventEmitter};
use pipkin_core::*;

pub type EffectHandler = Box<dyn FnMut(Effect, &mut Context<Model>)>;

pub struct Model {
    pub state: AppState,
    /// Ephemeral per-connection login prompts and links; never sent to SavePrefs or diagnostics.
    pub sign_in: onboarding::SignInSnapshot,
    pub entry: onboarding::EntrySurface,
    pub setup: onboarding::SetupFlow,
    ever_started_setup: bool,
    pub managing_connections: bool,
    pub pending_removal: Option<onboarding::SignInProvider>,
    pub accepted_provider: Option<onboarding::SignInProvider>,
    pub engine_selected_model: Option<String>,
    completing_setup: bool,
    handler: Option<EffectHandler>,
    support_report: Option<String>,
    /// Exact engine IDs from the controller's authoritative catalog, never desktop IDs.
    local_cli_sessions: std::collections::HashMap<ConversationId, EngineSessionId>,
}

impl EventEmitter<Note> for Model {}

impl Model {
    pub fn new(state: AppState) -> Self {
        Model {
            state,
            sign_in: onboarding::SignInSnapshot::default(),
            entry: onboarding::EntrySurface::Workspace,
            setup: onboarding::SetupFlow::default(),
            ever_started_setup: false,
            managing_connections: false,
            pending_removal: None,
            accepted_provider: None,
            engine_selected_model: None,
            completing_setup: false,
            handler: None,
            support_report: None,
            local_cli_sessions: Default::default(),
        }
    }

    /// Installed by the controller (pipkin-app). Effects are executed outside render.
    pub fn set_effect_handler(&mut self, handler: EffectHandler) {
        self.handler = Some(handler);
    }

    /// Controller integration hook: advertise only the adapter's actual setup capability.
    /// Session pairs must come from captured authoritative source backend IDs. Replacing the
    /// snapshot (including with an empty one on disconnect) prevents stale reapproval targets.
    pub fn set_local_cli_catalog(
        &mut self,
        available: bool,
        sessions: impl IntoIterator<Item = (ConversationId, EngineSessionId)>,
        cx: &mut Context<Self>,
    ) {
        let mut outcome = self.state.set_local_cli_available(available);
        let sessions = sessions.into_iter().collect();
        if self.local_cli_sessions != sessions {
            self.local_cli_sessions = sessions;
            outcome.notes.push(pipkin_core::Note::Other);
        }
        self.finish(outcome, cx);
    }

    pub fn set_cli_approvals_available(&mut self, available: bool, cx: &mut Context<Self>) {
        let out = self.state.set_cli_approvals_available(available);
        self.finish(out, cx);
    }

    pub fn local_cli_session(&self, conversation: ConversationId) -> Option<&EngineSessionId> {
        self.local_cli_sessions.get(&conversation)
    }

    /// Prepared by the controller outside render; never composed from conversation/error text.
    pub fn set_support_report(&mut self, report: String) {
        self.support_report = Some(report);
    }

    pub fn support_report(&self) -> Option<&str> {
        self.support_report.as_deref()
    }

    pub fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {
        let outcome = self.state.dispatch(command);
        self.finish(outcome, cx);
    }

    pub fn apply_event(&mut self, event: BackendEvent, cx: &mut Context<Self>) {
        let outcome = self.state.apply_event(event);
        self.finish(outcome, cx);
    }

    /// Run a state change that produces an `Outcome` (effects and notes) outside a command.
    pub fn mutate(&mut self, f: impl FnOnce(&mut AppState) -> Outcome, cx: &mut Context<Self>) {
        let outcome = f(&mut self.state);
        self.finish(outcome, cx);
    }

    pub fn set_connection(&mut self, connection: Connection, cx: &mut Context<Self>) {
        if !connection.is_ready() {
            self.local_cli_sessions.clear();
        }
        if !connection.is_ready() && self.state.connection.is_ready() {
            self.accepted_provider = None;
            self.engine_selected_model = None;
            self.setup.retry();
        }
        let outcome = self.state.set_connection(connection);
        self.finish(outcome, cx);
    }

    pub fn set_sign_in(&mut self, state: onboarding::SignInSnapshot, cx: &mut Context<Self>) {
        // A profile with any Pi credential is a returning user, even when Pipkin has no local
        // conversations yet. Once the tour was deliberately started, do not silently skip it.
        if self.entry == onboarding::EntrySurface::Welcome
            && !self.ever_started_setup
            && state.credentials_known
            && state.has_existing_credentials
        {
            self.entry = onboarding::EntrySurface::Workspace;
        }
        if state.status == onboarding::SignInStatus::Done {
            self.accepted_provider = state.provider;
        }
        self.sign_in = state;
        self.reconcile_setup();
        cx.notify();
    }

    pub fn begin_setup(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.state.connection,
            Connection::Connecting | Connection::Reconnecting
        ) || (self.state.connection.is_ready()
            && self.sign_in.available
            && !self.sign_in.credentials_known)
        {
            return;
        }
        self.ever_started_setup = true;
        self.setup.begin();
        self.reconcile_setup();
        cx.notify();
    }

    pub fn back_to_welcome(&mut self, cx: &mut Context<Self>) {
        if let Some(attempt) = self.sign_in.attempt.clone() {
            self.dispatch(Command::CancelSignIn { attempt }, cx);
        }
        self.setup.back_to_welcome();
        self.reconcile_setup();
        cx.notify();
    }

    pub fn explore_workspace(&mut self, cx: &mut Context<Self>) {
        if let Some(attempt) = self.sign_in.attempt.clone() {
            self.dispatch(Command::CancelSignIn { attempt }, cx);
        }
        self.managing_connections = false;
        self.pending_removal = None;
        self.entry = onboarding::EntrySurface::Workspace;
        cx.notify();
    }

    /// Let a returning user with no usable model connect a subscription in the GUI. Keep all
    /// existing projects, conversations and drafts; opening setup does not mark it complete.
    pub fn manage_connections(&mut self, cx: &mut Context<Self>) {
        self.open_account_setup(cx);
        self.managing_connections = true;
        self.pending_removal = None;
        cx.notify();
    }

    pub fn open_account_setup(&mut self, cx: &mut Context<Self>) {
        self.managing_connections = false;
        self.entry = onboarding::EntrySurface::Welcome;
        self.ever_started_setup = true;
        self.setup.begin();
        self.reconcile_setup();
        cx.notify();
    }

    pub fn accept_existing_provider(
        &mut self,
        provider: onboarding::SignInProvider,
        cx: &mut Context<Self>,
    ) {
        if self.sign_in.available
            && self.state.connection.is_ready()
            && self.sign_in.existing_providers.contains(&provider)
        {
            // Discovery metadata alone is not proof: Pi must acknowledge reuse after it checks
            // the credential and its available model catalogue.
            self.accepted_provider = None;
            self.engine_selected_model = None;
            self.dispatch(Command::ReuseSignIn(provider), cx);
        }
    }

    pub fn start_sign_in(&mut self, provider: onboarding::SignInProvider, cx: &mut Context<Self>) {
        if !self.sign_in.available || !self.state.connection.is_ready() {
            return;
        }
        self.accepted_provider = None;
        self.engine_selected_model = None;
        self.dispatch(Command::StartSignIn(provider), cx);
        self.reconcile_setup();
    }

    pub fn back_to_provider(&mut self, cx: &mut Context<Self>) {
        self.accepted_provider = None;
        self.engine_selected_model = None;
        self.reconcile_setup();
        cx.notify();
    }

    pub fn native_conversation_created(
        &mut self,
        request: &pipkin_core::RequestId,
        conversation: pipkin_core::ConversationId,
        cx: &mut Context<Self>,
    ) {
        let out = self
            .state
            .native_conversation_created(request, conversation);
        self.finish(out, cx);
    }

    pub fn engine_model_selected(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        self.engine_selected_model = model.clone();
        self.mutate(|state| state.set_engine_model(model), cx);
        self.reconcile_setup();
    }

    pub fn reconcile_setup(&mut self) {
        use onboarding::{EngineReadiness, ProviderReadiness, SetupFacts};
        let engine = match self.state.connection {
            Connection::Ready => EngineReadiness::Ready,
            Connection::Connecting | Connection::Reconnecting => EngineReadiness::Preparing,
            _ => EngineReadiness::Unavailable,
        };
        let provider = if self.accepted_provider.is_some() {
            ProviderReadiness::Configured
        } else {
            match self.sign_in.status {
                onboarding::SignInStatus::Connecting
                | onboarding::SignInStatus::Waiting
                | onboarding::SignInStatus::Prompt => ProviderReadiness::Connecting,
                onboarding::SignInStatus::Failed => ProviderReadiness::Failed,
                _ if !self.sign_in.existing_providers.is_empty() => {
                    ProviderReadiness::ExistingConnectionAvailable
                }
                _ => ProviderReadiness::Unknown,
            }
        };
        let model_ready = self
            .engine_selected_model
            .as_deref()
            .is_some_and(|selected| {
                self.state.models.iter().any(|m| m.id == selected)
                    && self.accepted_provider.is_some_and(|p| {
                        selected
                            .split_once('/')
                            .is_some_and(|(id, _)| id == p.pi_id())
                    })
            });
        let facts = SetupFacts {
            engine,
            provider,
            model_ready,
            // A folder bookmark alone is not a usable conversation: wait for Pi to create
            // and list the project session before advancing to its model catalogue.
            project_ready: self.state.current_project().is_some() && self.state.current().is_some(),
        };
        self.setup.reconcile(self.setup.epoch(), facts);
    }

    pub fn enter_workspace(&mut self, cx: &mut Context<Self>) {
        if self.completing_setup || !self.setup.can_complete() || self.state.current().is_none() {
            return;
        }
        self.completing_setup = true;
        let mut prefs = self.state.prefs.clone();
        prefs.setup_completed = true;
        if let Some(mut handler) = self.handler.take() {
            handler(Effect::CompleteSetup(prefs), cx);
            self.handler = Some(handler);
        }
        cx.notify();
    }

    pub fn setup_saved(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.completing_setup = false;
        match result {
            Ok(()) if self.setup.can_complete() => {
                self.state.prefs.setup_completed = true;
                self.entry = onboarding::EntrySurface::Workspace;
            }
            Ok(()) => {
                // The write succeeded, but the engine changed before the acknowledgement.
                // Preserve the saved flag for a safe return, without implying setup is ready.
                self.state.prefs.setup_completed = true;
            }
            Err(error) => {
                self.mutate(
                    |state| {
                        state
                            .set_storage_issue(format!("Could not save setup: {error}. Try again."))
                    },
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub fn draft_saved(
        &mut self,
        conversation: ConversationId,
        rev: u64,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.state.draft_saved(conversation, rev, result);
        cx.notify();
    }

    fn finish(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        let Outcome { effects, notes } = outcome;
        if let Some(mut handler) = self.handler.take() {
            for effect in effects {
                handler(effect, cx);
            }
            self.handler = Some(handler);
        }
        for note in notes {
            cx.emit(note);
        }
        self.reconcile_setup();
        cx.notify();
    }
}

/// Developer controls for the simulated backend. Installed as a GPUI global by the controller;
/// the command palette lists these under "Developer".
#[derive(Clone)]
pub struct DemoControls {
    /// (name, one-line description) of each scripted scenario.
    pub scenarios: Vec<(&'static str, &'static str)>,
    pub current_scenario: std::rc::Rc<dyn Fn() -> String>,
    pub set_scenario: std::rc::Rc<dyn Fn(&str)>,
    pub save_failure: std::rc::Rc<dyn Fn() -> bool>,
    pub set_save_failure: std::rc::Rc<dyn Fn(bool)>,
}

impl gpui::Global for DemoControls {}

#[cfg(test)]
mod account_setup_tests {
    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    #[gpui::test]
    fn a_returning_user_can_open_setup_without_losing_saved_work_or_restarting_on_discovery(
        cx: &mut TestAppContext,
    ) {
        let model = cx.update(|cx| {
            let mut state = AppState::new(
                Bootstrap {
                    projects: vec![],
                    models: vec![],
                    conversations: vec![],
                    now: 0,
                },
                Prefs::default(),
            );
            state.restore_project("/tmp/existing-project");
            state.connection = Connection::Ready;
            cx.new(|_| Model::new(state))
        });
        model.update(cx, |m, cx| {
            assert_eq!(m.entry, onboarding::EntrySurface::Workspace);
            m.set_sign_in(
                onboarding::SignInSnapshot {
                    available: true,
                    credentials_known: true,
                    has_existing_credentials: true,
                    ..Default::default()
                },
                cx,
            );
            m.open_account_setup(cx);
            assert_eq!(m.entry, onboarding::EntrySurface::Welcome);
            assert_eq!(m.setup.stage(), onboarding::SetupStage::ConnectProvider);
            assert!(!m.state.prefs.setup_completed);
            assert_eq!(m.state.projects.len(), 1);
            // A late credential discovery must not undo an explicit return to setup.
            m.set_sign_in(
                onboarding::SignInSnapshot {
                    available: true,
                    credentials_known: true,
                    has_existing_credentials: true,
                    ..Default::default()
                },
                cx,
            );
            assert_eq!(m.entry, onboarding::EntrySurface::Welcome);
            m.explore_workspace(cx);
            assert_eq!(m.entry, onboarding::EntrySurface::Workspace);
            assert!(!m.state.prefs.setup_completed);
            assert_eq!(m.state.projects.len(), 1);

            m.state.prefs.setup_completed = true;
            m.accepted_provider = Some(onboarding::SignInProvider::Claude);
            m.manage_connections(cx);
            assert!(m.managing_connections);
            assert!(m.state.prefs.setup_completed);
            m.set_sign_in(
                onboarding::SignInSnapshot {
                    available: true,
                    status: onboarding::SignInStatus::Done,
                    provider: Some(onboarding::SignInProvider::ChatGpt),
                    ..Default::default()
                },
                cx,
            );
            assert!(m.managing_connections);
            assert_eq!(m.entry, onboarding::EntrySurface::Welcome);
            assert_eq!(
                m.accepted_provider,
                Some(onboarding::SignInProvider::ChatGpt)
            );
            m.pending_removal = Some(onboarding::SignInProvider::Claude);
            m.explore_workspace(cx);
            assert!(!m.managing_connections);
            assert!(m.pending_removal.is_none());
            assert!(m.state.prefs.setup_completed);
            assert_eq!(m.state.projects.len(), 1);
        });
    }
}

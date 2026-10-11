//! First-run policy, independent of rendering, transport and credential values.
//! Only Pi's allowlisted OAuth service may accept a sign-in response. No response is held in
//! setup facts, serialized to storage, or printed by Debug.

/// The only account connections offered by the first-run wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInProvider {
    Claude,
    ChatGpt,
}

impl SignInProvider {
    pub fn pi_id(self) -> &'static str {
        match self {
            Self::Claude => "anthropic",
            Self::ChatGpt => "openai-codex",
        }
    }
}

/// An entered answer is never printed by backend request diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub struct SignInAnswer(String);

impl SignInAnswer {
    pub fn new(answer: String) -> Self {
        Self(answer)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SignInAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[sign-in answer redacted]")
    }
}

/// Volatile data from Pi's per-client login service. Not saved to Pipkin's database.
#[derive(Clone, PartialEq, Eq)]
pub struct SignInSnapshot {
    pub available: bool,
    pub credentials_known: bool,
    pub credential_lookup_failed: bool,
    pub has_existing_credentials: bool,
    /// Only pre-existing, allowlisted subscription OAuth providers may be reused in setup.
    pub existing_providers: Vec<SignInProvider>,
    pub attempt: Option<String>,
    pub provider: Option<SignInProvider>,
    pub status: SignInStatus,
    pub message: String,
    pub url: Option<String>,
    pub device_code: Option<String>,
    pub challenge: Option<SignInChallenge>,
    /// Allowlisted providers the engine cannot sign in to, with the engine's reason (a reserved
    /// integration, say). Shown as unavailable; never started, reused or substituted.
    pub unavailable: Vec<(SignInProvider, String)>,
}

impl SignInSnapshot {
    /// Why `provider` cannot be signed in to here, if it cannot.
    pub fn unavailable_reason(&self, provider: SignInProvider) -> Option<&str> {
        self.unavailable
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, reason)| reason.as_str())
    }

    /// Whether a sign-in to `provider` may be started (or an existing one reused).
    pub fn can_sign_in(&self, provider: SignInProvider) -> bool {
        self.available && self.unavailable_reason(provider).is_none()
    }
}

impl Default for SignInSnapshot {
    fn default() -> Self {
        Self {
            available: false,
            credentials_known: false,
            credential_lookup_failed: false,
            has_existing_credentials: false,
            existing_providers: vec![],
            attempt: None,
            provider: None,
            status: SignInStatus::Idle,
            message: String::new(),
            url: None,
            device_code: None,
            challenge: None,
            unavailable: vec![],
        }
    }
}

impl std::fmt::Debug for SignInSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInSnapshot")
            .field("available", &self.available)
            .field("credentials_known", &self.credentials_known)
            .field("credential_lookup_failed", &self.credential_lookup_failed)
            .field("provider", &self.provider)
            .field("status", &self.status)
            .field("unavailable", &self.unavailable)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignInStatus {
    #[default]
    Idle,
    Connecting,
    Waiting,
    Prompt,
    Done,
    Failed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SignInChallenge {
    pub id: String,
    pub kind: String,
    pub message: String,
    pub placeholder: Option<String>,
    pub options: Vec<(String, String)>,
}

impl std::fmt::Debug for SignInChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInChallenge")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntrySurface {
    Welcome,
    Workspace,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryFacts {
    pub setup_completed: bool,
    pub has_saved_work: bool,
    pub demo: bool,
    pub explicit_external_server: bool,
}

impl EntryFacts {
    pub fn surface(self) -> EntrySurface {
        if self.setup_completed || self.has_saved_work || self.demo || self.explicit_external_server
        {
            EntrySurface::Workspace
        } else {
            EntrySurface::Welcome
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineReadiness {
    #[default]
    Preparing,
    Unavailable,
    Ready,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderReadiness {
    #[default]
    Unknown,
    /// Discovery only: never imply consent or successful authentication.
    ExistingConnectionAvailable,
    Connecting,
    Failed,
    /// The engine acknowledged configuration; not proof of a successful paid request.
    Configured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupStage {
    Welcome,
    PreparingEngine,
    EngineUnavailable,
    ConnectProvider,
    ConnectingProvider,
    ProviderFailed,
    ChooseModel,
    ChooseProject,
    Ready,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetupFacts {
    pub engine: EngineReadiness,
    pub provider: ProviderReadiness,
    /// An available model selection acknowledged by the engine, not a local dropdown value.
    pub model_ready: bool,
    /// A selected usable project acknowledged by the application, not arbitrary picker text.
    pub project_ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupFlow {
    started: bool,
    epoch: u64,
    facts: SetupFacts,
}

impl SetupFlow {
    pub fn started(&self) -> bool {
        self.started
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn begin(&mut self) {
        self.started = true;
    }

    /// Going back changes presentation, not engine acknowledgements or project choice.
    pub fn back_to_welcome(&mut self) {
        self.started = false;
    }

    /// A new engine/connection attempt invalidates old observations but preserves project choice.
    /// Overflow fails rather than reusing an epoch and accepting a previous attempt's result.
    pub fn retry(&mut self) -> Option<u64> {
        self.epoch = self.epoch.checked_add(1)?;
        self.facts.engine = EngineReadiness::Preparing;
        self.facts.provider = ProviderReadiness::Unknown;
        self.facts.model_ready = false;
        Some(self.epoch)
    }

    /// Apply an authoritative, secret-free snapshot only for the current setup attempt.
    pub fn reconcile(&mut self, epoch: u64, facts: SetupFacts) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.facts = facts;
        true
    }

    pub fn stage(&self) -> SetupStage {
        if !self.started {
            return SetupStage::Welcome;
        }
        match self.facts.engine {
            EngineReadiness::Preparing => return SetupStage::PreparingEngine,
            EngineReadiness::Unavailable => return SetupStage::EngineUnavailable,
            EngineReadiness::Ready => {}
        }
        match self.facts.provider {
            ProviderReadiness::Unknown | ProviderReadiness::ExistingConnectionAvailable => {
                SetupStage::ConnectProvider
            }
            ProviderReadiness::Connecting => SetupStage::ConnectingProvider,
            ProviderReadiness::Failed => SetupStage::ProviderFailed,
            // Pi models belong to a project session. Open the folder before selecting one.
            ProviderReadiness::Configured if !self.facts.project_ready => SetupStage::ChooseProject,
            ProviderReadiness::Configured if !self.facts.model_ready => SetupStage::ChooseModel,
            ProviderReadiness::Configured => SetupStage::Ready,
        }
    }

    /// Eligibility only. The controller must persist completion after deliberate workspace entry,
    /// and acknowledge that write. Exploring/skipping or showing Welcome never implies completion.
    pub fn can_complete(&self) -> bool {
        self.stage() == SetupStage::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entered_code_is_not_rendered_in_request_debug() {
        let request = crate::protocol::BackendRequest::AnswerSignIn {
            attempt: "new-attempt".into(),
            challenge: "challenge".into(),
            response: SignInAnswer::new("private-redirect-code".into()),
        };
        assert!(!format!("{request:?}").contains("private-redirect-code"));
        assert!(format!("{request:?}").contains("[sign-in answer redacted]"));
    }

    #[test]
    fn an_unavailable_provider_cannot_be_started_while_others_can() {
        let pi = SignInSnapshot {
            available: true,
            ..SignInSnapshot::default()
        };
        assert!(pi.can_sign_in(SignInProvider::Claude));
        assert!(pi.can_sign_in(SignInProvider::ChatGpt));
        assert_eq!(pi.unavailable_reason(SignInProvider::Claude), None);
        let native = SignInSnapshot {
            available: true,
            unavailable: vec![(
                SignInProvider::Claude,
                "Reserved for a later release.".into(),
            )],
            ..SignInSnapshot::default()
        };
        assert!(!native.can_sign_in(SignInProvider::Claude));
        assert!(native.can_sign_in(SignInProvider::ChatGpt));
        assert_eq!(
            native.unavailable_reason(SignInProvider::Claude),
            Some("Reserved for a later release.")
        );
        assert!(!SignInSnapshot::default().can_sign_in(SignInProvider::ChatGpt));
    }

    #[test]
    fn only_fresh_ordinary_real_launch_enters_welcome() {
        assert_eq!(EntryFacts::default().surface(), EntrySurface::Welcome);
        for facts in [
            EntryFacts {
                setup_completed: true,
                ..EntryFacts::default()
            },
            EntryFacts {
                has_saved_work: true,
                ..EntryFacts::default()
            },
            EntryFacts {
                demo: true,
                ..EntryFacts::default()
            },
            EntryFacts {
                explicit_external_server: true,
                ..EntryFacts::default()
            },
        ] {
            assert_eq!(facts.surface(), EntrySurface::Workspace);
        }
    }

    #[test]
    fn back_to_welcome_does_not_erase_acknowledged_choices() {
        let mut flow = SetupFlow::default();
        flow.begin();
        let ready = SetupFacts {
            engine: EngineReadiness::Ready,
            provider: ProviderReadiness::Configured,
            model_ready: true,
            project_ready: true,
        };
        flow.reconcile(0, ready);
        flow.back_to_welcome();
        assert_eq!(flow.stage(), SetupStage::Welcome);
        flow.begin();
        assert!(flow.can_complete());
    }

    #[test]
    fn preparation_runs_backstage_without_skipping_welcome() {
        let mut flow = SetupFlow::default();
        flow.reconcile(
            0,
            SetupFacts {
                engine: EngineReadiness::Ready,
                ..SetupFacts::default()
            },
        );
        assert_eq!(flow.stage(), SetupStage::Welcome);
        assert!(!flow.can_complete());
        flow.begin();
        assert_eq!(flow.stage(), SetupStage::ConnectProvider);
    }

    #[test]
    fn readiness_requires_engine_provider_and_project_acknowledgements() {
        for engine in [
            EngineReadiness::Preparing,
            EngineReadiness::Unavailable,
            EngineReadiness::Ready,
        ] {
            for provider in [
                ProviderReadiness::Unknown,
                ProviderReadiness::ExistingConnectionAvailable,
                ProviderReadiness::Connecting,
                ProviderReadiness::Failed,
                ProviderReadiness::Configured,
            ] {
                for project_ready in [false, true] {
                    for model_ready in [false, true] {
                        let mut flow = SetupFlow::default();
                        flow.begin();
                        flow.reconcile(
                            0,
                            SetupFacts {
                                engine,
                                provider,
                                model_ready,
                                project_ready,
                            },
                        );
                        assert_eq!(
                            flow.can_complete(),
                            engine == EngineReadiness::Ready
                                && provider == ProviderReadiness::Configured
                                && model_ready
                                && project_ready
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn existing_credentials_are_a_choice_not_implicit_consent() {
        let mut flow = SetupFlow::default();
        flow.begin();
        flow.reconcile(
            0,
            SetupFacts {
                engine: EngineReadiness::Ready,
                provider: ProviderReadiness::ExistingConnectionAvailable,
                model_ready: true,
                project_ready: true,
            },
        );
        assert_eq!(flow.stage(), SetupStage::ConnectProvider);
        assert!(!flow.can_complete());
    }

    #[test]
    fn retry_rejects_late_success_without_losing_project_choice() {
        let mut flow = SetupFlow::default();
        flow.begin();
        let ready = SetupFacts {
            engine: EngineReadiness::Ready,
            provider: ProviderReadiness::Configured,
            model_ready: true,
            project_ready: true,
        };
        flow.reconcile(0, ready);
        assert!(flow.can_complete());
        let epoch = flow.retry().unwrap();
        assert_eq!(epoch, 1);
        assert!(!flow.reconcile(0, ready));
        assert_eq!(flow.stage(), SetupStage::PreparingEngine);
        assert!(flow.facts.project_ready);
        flow.reconcile(epoch, ready);
        assert!(flow.can_complete());
    }

    #[test]
    fn disconnected_engine_takes_precedence_over_cached_provider_success() {
        let mut flow = SetupFlow::default();
        flow.begin();
        flow.reconcile(
            0,
            SetupFacts {
                engine: EngineReadiness::Unavailable,
                provider: ProviderReadiness::Configured,
                model_ready: true,
                project_ready: true,
            },
        );
        assert_eq!(flow.stage(), SetupStage::EngineUnavailable);
        assert!(!flow.can_complete());
    }

    #[test]
    fn provider_pending_failure_and_project_choice_are_distinct() {
        let mut flow = SetupFlow::default();
        flow.begin();
        for (provider, expected) in [
            (
                ProviderReadiness::Connecting,
                SetupStage::ConnectingProvider,
            ),
            (ProviderReadiness::Failed, SetupStage::ProviderFailed),
            (ProviderReadiness::Configured, SetupStage::ChooseProject),
        ] {
            flow.reconcile(
                0,
                SetupFacts {
                    engine: EngineReadiness::Ready,
                    provider,
                    model_ready: true,
                    project_ready: false,
                },
            );
            assert_eq!(flow.stage(), expected);
        }
        flow.reconcile(
            0,
            SetupFacts {
                engine: EngineReadiness::Ready,
                provider: ProviderReadiness::Configured,
                model_ready: false,
                project_ready: true,
            },
        );
        assert_eq!(flow.stage(), SetupStage::ChooseModel);
        assert!(!flow.can_complete());
    }

    #[test]
    fn epoch_exhaustion_does_not_wrap_or_reset_acknowledged_facts() {
        let mut flow = SetupFlow {
            started: true,
            epoch: u64::MAX,
            facts: SetupFacts {
                engine: EngineReadiness::Ready,
                provider: ProviderReadiness::Configured,
                model_ready: true,
                project_ready: true,
            },
        };
        let before = flow.clone();
        assert_eq!(flow.retry(), None);
        assert_eq!(flow, before);
    }
}

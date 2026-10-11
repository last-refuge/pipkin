//! Full-window first-run surface. Only Pi-confirmed auth and model states advance it.
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    StyledImage, Window, div, img, px,
};
use pipkin_core::{
    Command,
    onboarding::{SetupStage, SignInAnswer, SignInProvider},
};

use super::controls::{Btn, BtnKind};
use super::workspace::Workspace;
use crate::model::Model;
use crate::theme::ActiveTheme;

/// One subscription sign-in button on the connect step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProviderChoice {
    pub provider: SignInProvider,
    pub id: &'static str,
    pub reason_id: &'static str,
    pub label: &'static str,
    /// The engine cannot sign in to this provider; the button is disabled and this says why.
    pub reason: Option<String>,
}

/// The sign-in buttons for `auth`, in display order. An unavailable provider stays listed (so
/// the person learns why it is missing) but can never be started.
pub(super) fn provider_choices(
    auth: &pipkin_core::onboarding::SignInSnapshot,
    managing: bool,
) -> Vec<ProviderChoice> {
    [
        (
            SignInProvider::Claude,
            "Claude Pro / Max",
            "setup-claude",
            "setup-claude-unavailable",
        ),
        (
            SignInProvider::ChatGpt,
            "ChatGPT Plus / Pro",
            "setup-chatgpt",
            "setup-chatgpt-unavailable",
        ),
    ]
    .into_iter()
    .map(|(provider, label, id, reason_id)| {
        let reason = auth.unavailable_reason(provider).map(|why| {
            let name = match provider {
                SignInProvider::Claude => "Claude",
                SignInProvider::ChatGpt => "ChatGPT",
            };
            format!("{name} sign-in is not available yet: {why}")
        });
        let label = if reason.is_none() && managing && auth.existing_providers.contains(&provider) {
            match provider {
                SignInProvider::Claude => "Sign in to Claude again",
                SignInProvider::ChatGpt => "Sign in to ChatGPT again",
            }
        } else {
            label
        };
        ProviderChoice {
            provider,
            id,
            reason_id,
            label,
            reason,
        }
    })
    .collect()
}

impl Workspace {
    fn setup_action(
        &self,
        id: &'static str,
        label: &'static str,
        kind: BtnKind,
        enabled: bool,
        f: impl Fn(&mut Model, &mut Context<Model>) + 'static,
    ) -> Btn {
        let model = self.model.clone();
        Btn::new(id)
            .label(label)
            .kind(kind)
            .disabled(!enabled)
            .on_click(move |_, cx| {
                model.update(cx, |m, cx| f(m, cx));
            })
    }

    pub(super) fn submit_sign_in_answer(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.model.read(cx).sign_in.clone();
        let (Some(attempt), Some(challenge)) = (snapshot.attempt, snapshot.challenge) else {
            return;
        };
        let response = self.setup_input.read(cx).text();
        if response.is_empty() {
            return;
        }
        self.dispatch(
            Command::AnswerSignIn {
                attempt,
                challenge: challenge.id,
                response: SignInAnswer::new(response),
            },
            cx,
        );
        // The response lives in the editor only until it is submitted. It is never a draft.
        self.setup_input.update(cx, |e, cx| e.set_text("", cx));
    }

    pub(super) fn render_setup(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let m = self.model.read(cx);
        let managing = m.managing_connections;
        let pending_removal = m.pending_removal;
        let stage = if managing && m.state.connection.is_ready() {
            match m.sign_in.status {
                pipkin_core::onboarding::SignInStatus::Connecting
                | pipkin_core::onboarding::SignInStatus::Waiting
                | pipkin_core::onboarding::SignInStatus::Prompt => SetupStage::ConnectingProvider,
                _ => SetupStage::ConnectProvider,
            }
        } else {
            m.setup.stage()
        };
        let auth = m.sign_in.clone();
        let connection = m.state.connection.clone();
        let current_project = m.state.current_project().map(|p| p.name.clone());
        let models: Vec<_> = m
            .state
            .models
            .iter()
            .filter(|info| {
                m.accepted_provider
                    .is_some_and(|provider| info.id.starts_with(&format!("{}/", provider.pi_id())))
            })
            .cloned()
            .collect();
        let has_conversation = m.state.current().is_some();
        let project_edit = self.setup_project_edit;
        let stage = if project_edit && matches!(stage, SetupStage::ChooseModel | SetupStage::Ready)
        {
            SetupStage::ChooseProject
        } else {
            stage
        };
        let (heading, description) = if managing {
            (
                "Model connections",
                "Connect another provider or sign in again. Removing a saved connection does not cancel your subscription or delete conversations.",
            )
        } else {
            match stage {
                SetupStage::Welcome => (
                    "A little help for your next big thing.",
                    "Connect your AI, choose a project, and start with something real.",
                ),
                SetupStage::PreparingEngine => (
                    "Making a little room to work.",
                    "Pipkin is preparing its built-in engine. This should only take a moment.",
                ),
                SetupStage::EngineUnavailable => (
                    "The engine isn't ready yet.",
                    "Pipkin couldn't reach its built-in engine. Check the app installation and try reopening Pipkin; your work is safe.",
                ),
                SetupStage::ConnectProvider => (
                    "Connect your AI.",
                    "Choose the subscription you'd like Pipkin to use. Sign-in happens with your provider; Pipkin does not store your password or code.",
                ),
                SetupStage::ConnectingProvider => (
                    "Continue with your provider.",
                    "Finish sign-in in your browser, then return here. Pipkin will wait for Pi to confirm your connection.",
                ),
                SetupStage::ProviderFailed => (
                    "That connection didn't finish.",
                    "Nothing was sent to a model. Try signing in again or choose another provider.",
                ),
                SetupStage::ChooseProject => (
                    "What are we working on?",
                    "Choose a folder for your first conversation. Pipkin will keep the work in that project.",
                ),
                SetupStage::ChooseModel => (
                    "Choose a model.",
                    "These are the models the engine reports as available for your connection.",
                ),
                SetupStage::Ready => (
                    "Ready when you are.",
                    "Your provider, model and project are connected. Start by describing what you'd like to do.",
                ),
            }
        };
        let title = div()
            .text_size(px(28.0 * theme.scale))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c.text)
            .child(heading);
        let intro = div()
            .text_size(px(15.0 * theme.scale))
            .text_color(c.text_muted)
            .child(description);
        let mascot = img(crate::assets::MASCOT)
            .w(px(110.0))
            .h(px(110.0))
            .object_fit(gpui::ObjectFit::Contain);
        let mut body = div().flex().flex_col().gap(px(12.0));
        match stage {
            SetupStage::Welcome => {
                let can_begin =
                    !matches!(
                        connection,
                        pipkin_core::Connection::Connecting | pipkin_core::Connection::Reconnecting
                    ) && (!connection.is_ready() || !auth.available || auth.credentials_known);
                if !can_begin {
                    let copy = if auth.credential_lookup_failed {
                        "Pipkin couldn't check saved connections. Explore your workspace or reopen the app to try again."
                    } else if connection.is_ready() {
                        "Checking your saved connections…"
                    } else {
                        "Preparing the built-in engine…"
                    };
                    body = body.child(
                        div()
                            .text_color(if auth.credential_lookup_failed {
                                c.warning
                            } else {
                                c.text_muted
                            })
                            .child(copy),
                    );
                }
                body = body
                    .child(self.setup_action(
                        "setup-start",
                        "Get started",
                        BtnKind::Primary,
                        can_begin,
                        |m, cx| m.begin_setup(cx),
                    ))
                    .child(self.setup_action(
                        "setup-explore",
                        "Explore the workspace",
                        BtnKind::Ghost,
                        true,
                        |m, cx| m.explore_workspace(cx),
                    ));
            }
            SetupStage::PreparingEngine | SetupStage::EngineUnavailable => {
                body = body
                    .child(
                        div()
                            .text_color(c.text_muted)
                            .child(match connection.banner() {
                                Some(message) => message,
                                None => "Waiting for the engine…".to_string(),
                            }),
                    )
                    .child(self.setup_action(
                        "setup-explore-engine",
                        "Explore the workspace",
                        BtnKind::Ghost,
                        true,
                        |m, cx| m.explore_workspace(cx),
                    ));
            }
            SetupStage::ConnectProvider | SetupStage::ProviderFailed => {
                if !auth.available {
                    body = body.child(div().text_color(c.warning).child("Subscription sign-in is not available in this Pi engine. An updated bundled engine is required."));
                } else {
                    for choice in provider_choices(&auth, managing) {
                        let provider = choice.provider;
                        body = body.child(self.setup_action(
                            choice.id,
                            choice.label,
                            BtnKind::Subtle,
                            connection.is_ready() && choice.reason.is_none(),
                            move |m, cx| m.start_sign_in(provider, cx),
                        ));
                        if let Some(reason) = choice.reason {
                            // Said plainly, with no flow behind it: nothing is started, reused
                            // or substituted for an unavailable provider.
                            body = body.child(
                                div()
                                    .id(choice.reason_id)
                                    .text_size(theme.small_size())
                                    .text_color(c.text_muted)
                                    .child(reason),
                            );
                        }
                    }
                    body = body.child(div().text_size(theme.small_size()).text_color(c.text_muted)
                        .child("ChatGPT sign-in uses Pi's openai-codex connection, not an OpenAI API key."));
                    for (provider, label, id) in [
                        (
                            SignInProvider::Claude,
                            "Use my existing Claude connection",
                            "setup-reuse-claude",
                        ),
                        (
                            SignInProvider::ChatGpt,
                            "Use my existing ChatGPT connection",
                            "setup-reuse-chatgpt",
                        ),
                    ] {
                        if !managing
                            && auth.existing_providers.contains(&provider)
                            && auth.can_sign_in(provider)
                        {
                            body = body.child(self.setup_action(
                                id,
                                label,
                                BtnKind::Ghost,
                                true,
                                move |m, cx| m.accept_existing_provider(provider, cx),
                            ));
                        }
                    }
                }
                if managing {
                    if !auth.message.is_empty() {
                        body =
                            body.child(div().text_color(c.text_muted).child(auth.message.clone()));
                    }
                    for (provider, label, id) in [
                        (
                            SignInProvider::Claude,
                            "Remove Claude connection",
                            "remove-claude",
                        ),
                        (
                            SignInProvider::ChatGpt,
                            "Remove ChatGPT connection",
                            "remove-chatgpt",
                        ),
                    ] {
                        if auth.existing_providers.contains(&provider) {
                            body = body.child(self.setup_action(
                                id,
                                label,
                                BtnKind::Ghost,
                                connection.is_ready(),
                                move |m, cx| {
                                    m.pending_removal = Some(provider);
                                    cx.notify();
                                },
                            ));
                        }
                    }
                    if let Some(provider) = pending_removal {
                        body = body.child(div().text_color(c.warning).child(format!("Remove the saved {} connection from Pi's local credential storage?", provider.pi_id())))
                            .child(self.setup_action("confirm-remove", "Remove saved connection", BtnKind::Danger, connection.is_ready(), move |m, cx| {
                                m.pending_removal = None;
                                m.accepted_provider = None;
                                m.dispatch(Command::RemoveSignIn(provider), cx);
                            }))
                            .child(self.setup_action("cancel-remove", "Keep connection", BtnKind::Ghost, true, |m, cx| {
                                m.pending_removal = None;
                                cx.notify();
                            }));
                    }
                } else {
                    body = body.child(self.setup_action(
                        "setup-back-welcome",
                        "Back to welcome",
                        BtnKind::Ghost,
                        true,
                        |m, cx| m.back_to_welcome(cx),
                    ));
                    body = body.child(self.setup_action(
                        "setup-explore-provider",
                        "Explore the workspace",
                        BtnKind::Ghost,
                        true,
                        |m, cx| m.explore_workspace(cx),
                    ));
                }
            }
            SetupStage::ConnectingProvider => {
                if !auth.message.is_empty() {
                    body = body.child(div().text_color(c.text_muted).child(auth.message));
                }
                if let Some(url) = auth.url {
                    body = body.child(
                        Btn::new("setup-open-auth")
                            .label("Open sign-in page")
                            .kind(BtnKind::Primary)
                            .on_click(move |_, cx| cx.open_url(&url)),
                    );
                }
                if let Some(code) = auth.device_code {
                    body = body.child(
                        div()
                            .text_color(c.text)
                            .child(format!("Device code: {code}")),
                    );
                }
                if let Some(challenge) = auth.challenge {
                    body = body.child(div().text_color(c.text).child(challenge.message));
                    if challenge.kind == "select" {
                        for (index, (id, label)) in challenge.options.into_iter().enumerate() {
                            let attempt = auth.attempt.clone().unwrap_or_default();
                            let challenge_id = challenge.id.clone();
                            let model = self.model.clone();
                            body = body.child(
                                Btn::new(format!("setup-option-{index}"))
                                    .label(label)
                                    .kind(BtnKind::Subtle)
                                    .on_click(move |_, cx| {
                                        model.update(cx, |m, cx| {
                                            m.dispatch(
                                                Command::AnswerSignIn {
                                                    attempt: attempt.clone(),
                                                    challenge: challenge_id.clone(),
                                                    response: SignInAnswer::new(id.clone()),
                                                },
                                                cx,
                                            )
                                        });
                                    }),
                            );
                        }
                    } else if challenge.kind == "secret" {
                        body = body.child(div().text_color(c.warning).child("This provider requested a private field that Pipkin cannot safely display here. Cancel sign-in and try again later."));
                    } else {
                        body = body
                            .child(div().w_full().child(self.setup_input.clone()))
                            .child(
                                Btn::new("setup-answer")
                                    .label("Continue sign-in")
                                    .kind(BtnKind::Primary)
                                    .on_click({
                                        let this = cx.entity().downgrade();
                                        move |_, cx| {
                                            this.update(cx, |w, cx| w.submit_sign_in_answer(cx))
                                                .ok();
                                        }
                                    }),
                            );
                    }
                }
                if let Some(attempt) = auth.attempt {
                    let model = self.model.clone();
                    body = body.child(
                        Btn::new("setup-cancel-auth")
                            .label("Cancel sign-in")
                            .kind(BtnKind::Ghost)
                            .on_click(move |_, cx| {
                                model.update(cx, |m, cx| {
                                    m.dispatch(
                                        Command::CancelSignIn {
                                            attempt: attempt.clone(),
                                        },
                                        cx,
                                    );
                                });
                            }),
                    );
                }
            }
            SetupStage::ChooseProject => {
                if let Some(name) = current_project {
                    body = body.child(
                        div()
                            .text_color(c.text)
                            .child(format!("Selected folder: {name}")),
                    );
                }
                body = body.child(
                    Btn::new("setup-folder")
                        .label("Choose a project folder…")
                        .kind(BtnKind::Subtle)
                        .on_click({
                            let this = cx.entity().downgrade();
                            move |_, cx| {
                                this.update(cx, |w, cx| w.open_project(cx)).ok();
                            }
                        }),
                );
                if self.state(cx).current_project().is_some() {
                    let can_continue =
                        has_conversation || self.state(cx).availability().new_conversation;
                    body = body.child(
                        Btn::new("setup-project-continue")
                            .label(if can_continue {
                                "Continue"
                            } else {
                                "Opening conversation…"
                            })
                            .kind(BtnKind::Primary)
                            .disabled(!can_continue)
                            .on_click({
                                let this = cx.entity().downgrade();
                                move |_, cx| {
                                    this.update(cx, |w, cx| {
                                        w.setup_project_edit = false;
                                        if w.state(cx).current().is_none() {
                                            w.dispatch(Command::NewConversation, cx);
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            }),
                    );
                }
                body = body.child(
                    Btn::new("setup-back-provider")
                        .label("Back to connections")
                        .kind(BtnKind::Ghost)
                        .on_click({
                            let model = self.model.clone();
                            move |_, cx| model.update(cx, |m, cx| m.back_to_provider(cx))
                        }),
                );
            }
            SetupStage::ChooseModel => {
                if models.is_empty() {
                    body = body.child(div().text_color(c.warning).child(if has_conversation {
                        "No models are available for this connection yet. Refresh after signing in, or try another provider."
                    } else { "Opening the project conversation to discover available models…" }));
                    if has_conversation {
                        body = body.child(
                            Btn::new("setup-refresh-models")
                                .label("Refresh models")
                                .kind(BtnKind::Subtle)
                                .on_click({
                                    let model = self.model.clone();
                                    move |_, cx| {
                                        model.update(cx, |m, cx| {
                                            m.dispatch(Command::RefreshModels, cx)
                                        })
                                    }
                                }),
                        );
                    }
                }
                for (index, model_info) in models.into_iter().enumerate() {
                    let id = model_info.id;
                    let label = model_info.name;
                    let model = self.model.clone();
                    body = body.child(
                        Btn::new(format!("setup-model-{index}"))
                            .label(label)
                            .kind(BtnKind::Subtle)
                            .on_click(move |_, cx| {
                                model.update(cx, |m, cx| {
                                    m.dispatch(Command::SetModel(id.clone()), cx)
                                });
                            }),
                    );
                }
                body = body.child(
                    Btn::new("setup-back-folder")
                        .label("Back to project")
                        .kind(BtnKind::Ghost)
                        .on_click({
                            let this = cx.entity().downgrade();
                            move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.setup_project_edit = true;
                                    cx.notify();
                                })
                                .ok();
                            }
                        }),
                );
            }
            SetupStage::Ready => {
                body = body
                    .child(self.setup_action(
                        "setup-enter",
                        "Start a conversation",
                        BtnKind::Primary,
                        has_conversation,
                        |m, cx| m.enter_workspace(cx),
                    ))
                    .child(
                        Btn::new("setup-back-ready")
                            .label("Back to project")
                            .kind(BtnKind::Ghost)
                            .on_click({
                                let this = cx.entity().downgrade();
                                move |_, cx| {
                                    this.update(cx, |w, cx| {
                                        w.setup_project_edit = true;
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            }),
                    );
            }
        }
        if managing {
            body = body.child(self.setup_action(
                "connections-done",
                "Back to workspace",
                BtnKind::Subtle,
                true,
                |m, cx| m.explore_workspace(cx),
            ));
        }
        div()
            .id("setup-root")
            .key_context("Setup")
            .track_focus(&self.root_focus)
            .on_key_down(|ev, window, cx| {
                if ev.keystroke.key == "tab" {
                    if ev.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
            .size_full()
            .overflow_y_scroll()
            .bg(c.bg_app)
            .text_color(c.text)
            .font_family(theme.ui_font())
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(28.0))
                    .py(px(40.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(510.0))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(20.0))
                            .child(mascot)
                            .child(
                                div()
                                    .text_size(px(18.0 * theme.scale))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("Pipkin"),
                            )
                            .child(title)
                            .child(intro)
                            .child(div().h(px(16.0)))
                            .child(body),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use pipkin_core::onboarding::SignInSnapshot;

    use super::*;

    #[test]
    fn pi_offers_both_subscriptions_as_before() {
        let auth = SignInSnapshot {
            available: true,
            existing_providers: vec![SignInProvider::Claude],
            ..SignInSnapshot::default()
        };
        let choices = provider_choices(&auth, false);
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().all(|c| c.reason.is_none()));
        assert_eq!(choices[0].label, "Claude Pro / Max");
        assert_eq!(choices[1].label, "ChatGPT Plus / Pro");
        let managing = provider_choices(&auth, true);
        assert_eq!(managing[0].label, "Sign in to Claude again");
        assert_eq!(managing[1].label, "ChatGPT Plus / Pro");
    }

    #[test]
    fn a_reserved_claude_sign_in_is_shown_unavailable_with_its_reason_and_chatgpt_works() {
        let auth = SignInSnapshot {
            available: true,
            existing_providers: vec![SignInProvider::ChatGpt],
            unavailable: vec![(
                SignInProvider::Claude,
                "reserved for a later integration".into(),
            )],
            ..SignInSnapshot::default()
        };
        let choices = provider_choices(&auth, true);
        let claude = &choices[0];
        assert_eq!(claude.provider, SignInProvider::Claude);
        assert_eq!(
            claude.label, "Claude Pro / Max",
            "no 'sign in again' for it"
        );
        assert_eq!(
            claude.reason.as_deref(),
            Some("Claude sign-in is not available yet: reserved for a later integration")
        );
        let chatgpt = &choices[1];
        assert_eq!(chatgpt.reason, None);
        assert_eq!(chatgpt.label, "Sign in to ChatGPT again");
        assert!(!auth.can_sign_in(SignInProvider::Claude));
        assert!(auth.can_sign_in(SignInProvider::ChatGpt));
    }
}

//! Ordinary conversation entry and concise Claude setup. No I/O in render.
use super::*;
use gpui::PathPromptOptions;

impl Workspace {
    pub(in crate::shell) fn begin_conversation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state(cx).mode == Mode::Real
            && matches!(
                self.state(cx).next_provider_model(),
                Some(pipkin_core::ProviderModel::ClaudeCli(_))
            )
        {
            self.dispatch(CoreCommand::NewConversation, cx);
            self.open_overlay(Overlay::Prefs, window, cx);
            // Opening Settings from the workspace resets its page; choose Connections after it.
            self.settings_section = super::super::settings::SettingsSection::Connections;
        } else {
            self.dispatch(CoreCommand::NewConversation, cx);
            self.focus_composer(window, cx);
        }
    }

    pub(super) fn pick_cli_path(
        &mut self,
        executable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state(cx).local_cli_availability().edit {
            return;
        }
        let revision = self.state(cx).local_cli.input_revision();
        let draft = self.state(cx).local_cli.draft.clone();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: executable,
            directories: !executable,
            multiple: false,
            prompt: Some(
                if executable {
                    "Choose Claude executable"
                } else {
                    "Choose project"
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.first().and_then(|p| p.to_str()).map(str::to_owned) else {
                return;
            };
            let _ = this.update_in(cx, |w, window, cx| {
                let s = &w.state(cx).local_cli;
                if w.overlay != Overlay::LocalCli
                    || !s.guided
                    || s.input_revision() != revision
                    || s.draft != draft
                {
                    return;
                }
                let mut changed = draft.clone();
                if executable {
                    changed.executable = path;
                } else {
                    changed.project = path;
                }
                w.dispatch(CoreCommand::LocalCli(C::Configure(changed)), cx);
                window.focus(&w.menu_focus, cx);
            });
        })
        .detach();
    }

    pub(super) fn render_guided_cli(&self, cx: &Context<Self>) -> impl IntoElement {
        let state = self.state(cx);
        let s = &state.local_cli;
        let a = state.local_cli_availability();
        let existing = matches!(s.target, Some(LocalCliTarget::Existing { .. }));
        let close = cx.entity();
        let browse = cx.entity();
        let project = cx.entity();
        let advanced = cx.entity();
        let mut panel = elevated(cx).id("guided-claude-setup").occlude()
            .track_focus(&self.menu_focus.clone().tab_index(0).tab_stop(false))
            .tab_group().tab_stop(false).track_scroll(&self.cli_dialog_scroll)
            .capture_key_down(cx.listener(Self::cli_dialog_key))
            .role(Role::Dialog).aria_label("Claude Code connection")
            .w(px(580.)).max_h(px(650.)).overflow_y_scroll()
            .p(px(20.)).flex().flex_col().gap(px(12.))
            .child(div().flex().justify_between().child(if existing { "Reconnect Claude Code" } else if s.provider_only { "Configure Claude Code provider" } else { "Connect Claude Code conversation" })
                .child(Btn::new("guided-close").label("Close").on_click(move |window, cx| close.update(cx, |w,cx| w.close_overlay(window,cx)))))
            .child(if s.provider_only { "Uses Claude Code's normal login. Verification configures this provider only; choose a model from the normal model picker to start a conversation. Other providers remain usable." } else { "Uses Claude Code's normal login. Connecting opens a separate CLI conversation; it never converts an existing chat or forwards its draft. No message is sent by setup." });
        if existing && s.draft.executable.is_empty() {
            panel = panel.child(if s.error.is_some() {
                "Saved configuration is unavailable. No installation or model has been substituted."
            } else {
                "Loading saved conversation details… Nothing is being executed."
            });
            if s.error.is_some() {
                panel = panel.child(self.cli_button(
                    "guided-reload-hints",
                    "Reload saved details",
                    a.session_status,
                    C::SessionStatus,
                    cx,
                ));
            }
        } else if s.discovering {
            panel =
                panel.child("Finding Claude Code on the engine host… Nothing is being executed.");
        } else if !existing && s.draft.executable.is_empty() {
            panel = panel.child(if s.candidates.is_empty() { "Claude Code was not found. Choose its executable, or install/login using Claude Code's normal setup, then find it again." } else { "Multiple installations found. Choose the one you intend to trust:" });
            for (i, candidate) in s.candidates.iter().enumerate() {
                let mut draft = s.draft.clone();
                draft.executable = candidate.clone();
                let this = cx.entity();
                panel = panel.child(
                    Btn::new(("claude-candidate", i))
                        .label(candidate.escape_debug().to_string())
                        .disabled(!a.edit)
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.dispatch(CoreCommand::LocalCli(C::Configure(draft.clone())), cx)
                            })
                        }),
                );
            }
        }
        let executable = s
            .ticket
            .as_ref()
            .map(|t| t.executable.as_str())
            .unwrap_or(&s.draft.executable);
        let root = s
            .ticket
            .as_ref()
            .map(|t| t.project.as_str())
            .unwrap_or(&s.draft.project);
        if !executable.is_empty() {
            panel = panel.child(format!("Executable: {}", executable.escape_debug()));
        }
        panel = panel.child(if root.is_empty() {
            if existing {
                "Project: awaiting saved configuration"
            } else {
                "Project: choose a folder"
            }
            .into()
        } else {
            format!("Project: {}", root.escape_debug())
        });
        if existing {
            if !s.draft.executable.is_empty() {
                panel = panel.child(format!(
                    "Model: {:?} · tools: {:?}",
                    s.draft.model, s.draft.mode
                ));
            }
            if s.session_status.as_ref().is_some_and(|v| !v.context_valid) {
                panel = panel.child("This conversation cannot resume safely. Inspect any uncertain effects before starting a separate conversation; do not repeat its last message.");
                if let Some(target) = &s.target {
                    panel = panel.child(self.cli_button(
                        "guided-fresh",
                        "Set up a separate conversation",
                        a.fresh_setup,
                        C::FreshSetup(target.clone()),
                        cx,
                    ));
                }
            }
        }
        if !existing {
            panel = panel.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Btn::new("claude-browse")
                            .label("Choose executable…")
                            .disabled(!a.edit)
                            .on_click(move |window, cx| {
                                browse.update(cx, |w, cx| w.pick_cli_path(true, window, cx))
                            }),
                    )
                    .child(
                        Btn::new("claude-project")
                            .label("Change project…")
                            .disabled(!a.edit)
                            .on_click(move |window, cx| {
                                project.update(cx, |w, cx| w.pick_cli_path(false, window, cx))
                            }),
                    ),
            );
            let mut models = div().flex().gap(px(8.)).child("Model:");
            for (id, label, model) in [
                ("guided-haiku", "Haiku", CliModel::Haiku),
                ("guided-sonnet", "Sonnet", CliModel::Sonnet),
                ("guided-opus", "Opus", CliModel::Opus),
            ] {
                let mut draft = s.draft.clone();
                draft.model = model;
                models = models.child(
                    self.cli_button(id, label, a.edit, C::Configure(draft), cx)
                        .selected(s.draft.model == model),
                );
            }
            panel = panel.child(models);
            let mut modes = div().flex().gap(px(8.)).child("Tools:");
            for (id, label, mode) in [
                ("guided-chat", "Chat only", CliMode::ChatOnly),
                (
                    "guided-write",
                    "Write with approval",
                    CliMode::RestrictedWrite,
                ),
            ] {
                let mut draft = s.draft.clone();
                draft.mode = mode;
                modes = modes.child(
                    self.cli_button(id, label, a.edit, C::Configure(draft), cx)
                        .selected(s.draft.mode == mode),
                );
            }
            panel = panel.child(modes);
        }
        panel = panel.child(if s.draft.mode == CliMode::RestrictedWrite { "Write only: each Write needs separate argument review. No Read, Edit, Bash or MCP; not a sandbox." } else { "Chat only: no file tools. Model and tools are fixed for this conversation." });
        panel = panel.child("Login and compatibility are checked only after your confirmation. Account allowance or already-enabled extra usage may apply.");
        match s.stage {
            LocalCliStage::AwaitingTrust => if let Some(ticket) = &s.ticket {
                panel = panel.child("Confirm that you trust BOTH the executable and project shown above. This permits a login/version check and conversation setup—not a message or a Write.")
                    .child(div().flex().justify_center().debug_selector(|| "claude-confirm".into()).child(self.cli_button("claude-connect","Trust these paths and connect",a.trust,C::ConfirmAndConnect { setup: ticket.setup.clone() },cx)));
            },
            LocalCliStage::Inspecting => panel = panel.child("Resolving the selected paths…"),
            LocalCliStage::Checking => panel = panel.child("Checking Claude login and compatibility…"),
            LocalCliStage::Creating | LocalCliStage::Reconciling => panel = panel.child("Preparing your conversation… No message is being sent."),
            LocalCliStage::CheckUnknown => panel = panel.child("The check's outcome is unknown. Check its status; it will not be repeated automatically.").child(self.cli_button("guided-status","Check status",a.status,C::Status,cx)),
            LocalCliStage::CreateUnknown => panel = panel.child("Conversation creation is uncertain. Look up the original request before doing anything else.").child(self.cli_button("guided-lookup","Look up conversation",a.lookup_creation,C::LookupCreation,cx)),
            LocalCliStage::Verified if s.provider_only => panel = panel.child("Provider verified. Close Settings and choose Claude Code CLI · Haiku, Sonnet or Opus in the normal model picker. Each new conversation still needs scoped path confirmation."),
            LocalCliStage::Verified => panel = panel.child(self.cli_button("guided-continue",if existing { "Return to conversation" } else { "Create conversation" },if existing { a.return_to_conversation } else { a.create },if existing { C::ReturnToConversation } else { C::Create },cx)),
            _ => {}
        }
        if a.inspect && !s.discovering {
            panel = panel.child(self.cli_button(
                "guided-review",
                "Review selected paths",
                !s.draft.executable.is_empty() && (!s.draft.project.is_empty() || existing),
                C::Inspect,
                cx,
            ));
            if !existing {
                panel = panel.child(div().debug_selector(|| "claude-find-again".into()).child(
                    self.cli_button("guided-find", "Find Claude again", true, C::Discover, cx),
                ));
            }
        }
        if let Some(error) = &s.error {
            panel = panel.child(format!("Could not connect: {error:?}"));
        }
        if let Some(reason) = s.ticket.as_ref().and_then(|t| t.reason.as_ref()) {
            panel = panel.child(reason.clone());
        }
        if a.revoke {
            panel = panel.child(self.cli_button(
                "guided-cancel",
                "Cancel connection approval",
                true,
                C::Cancel,
                cx,
            ));
        }
        panel.child(
            Btn::new("claude-advanced")
                .label("Advanced paths, diagnostics and recovery…")
                .on_click(move |window, cx| {
                    advanced.update(cx, |w, cx| {
                        w.local_cli_advanced = true;
                        window.focus(&w.menu_focus, cx);
                        cx.notify();
                    })
                }),
        )
    }
}

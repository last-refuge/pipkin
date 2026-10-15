//! Local CLI presentation only. Core owns tickets, trust, identity and uncertain mutations.
#[cfg(test)]
mod tests;
use gpui::{App, Context, IntoElement, ParentElement, Role, Styled, Window, div, prelude::*, px};
use pipkin_core::{
    AppState, CliMode, CliModel, Command as CoreCommand, LOCAL_CLI_RECOVERY_CAPACITY,
    LocalCliCommand as C, LocalCliDraft, LocalCliStage, LocalCliTarget, Mode,
};

use super::{
    controls::{Btn, BtnKind, elevated},
    workspace::{Overlay, Workspace},
};
use crate::{text::ComposerEvent, theme::ActiveTheme};

pub(super) const NEW_LABEL: &str = "New Local CLI conversation…";
pub(super) const REAPPROVE_LABEL: &str = "Reapprove Local CLI…";
const LIMITS: &str = "Chat only: no Pipkin tools, queued follow-ups, or model changes. Uses Claude Code’s own CLI login, not Pipkin Connections.";
const WRITE_LIMITS: &str = "Write only · available by default in Pipkin's own engine; execution still needs fresh trust/check and exact approval. Attached engines control their own availability. No Read/Edit/Bash/MCP parity, no sandbox, no guarantee every CLI action needs a hook. A different mode requires a fresh conversation.";
const LEASE: &str = "Approval expires five minutes after Inspect. Create also requires preflight checked within 30 seconds: create promptly after Check. An uncertain creation requires explicit read-only lookup, never blind resubmission. Expiry or revocation stops active work; an interrupted CLI context requires a fresh conversation. Metadata verification alone does not enable execution.";
const BILLING: &str = "Billing unverified: may consume account allowance or already-enabled extra usage. Check your CLI account settings.";

pub(super) fn availability_label(state: &AppState) -> &'static str {
    if state.mode == Mode::Demo {
        "Unavailable in Demo · simulated agent"
    } else if !state.local_cli.available {
        "Local CLI setup is not advertised by this connection"
    } else if !state.connection.is_ready() {
        "Connect to the engine to set up Local CLI"
    } else if state.read_only.is_some() {
        "Local CLI setup is read-only"
    } else {
        "Explicit paths and approval required; no automatic checks"
    }
}

/// Profile is used only for wording; the core capability query controls availability.
pub(super) fn restricted_model_label(state: &AppState) -> Option<&'static str> {
    if state.execution_capabilities().model_settings {
        None
    } else if state.current().is_some_and(|c| c.profile.is_local_cli()) {
        Some("Claude CLI · fixed model")
    } else {
        Some("Model settings unsupported")
    }
}

fn stage_label(stage: LocalCliStage) -> &'static str {
    match stage {
        LocalCliStage::Editing => "1 · Enter paths and a fixed CLI alias, then Inspect",
        LocalCliStage::Inspecting => "Inspecting…",
        LocalCliStage::AwaitingTrust => "2 · Review captured paths, approve both, then Check",
        LocalCliStage::Checking => "Checking…",
        LocalCliStage::CheckUnknown => {
            "Check outcome unknown — explicitly check status; do not resend"
        }
        LocalCliStage::Verified => "3 · Metadata verified — Create or Return to conversation",
        LocalCliStage::Creating => "Creating…",
        LocalCliStage::CreateUnknown => {
            "Creation outcome unknown — look up the original identity first"
        }
        LocalCliStage::Reconciling => "Waiting for authoritative conversation catalog…",
        LocalCliStage::Created => "Created — conversation readiness comes from the engine",
        LocalCliStage::Failed => "Setup failed — review the error before trying again",
    }
}

impl Workspace {
    pub(super) fn local_cli_entry_enabled(&self, existing: bool, cx: &App) -> bool {
        let s = self.state(cx);
        if !s.local_cli_availability().begin {
            return false;
        }
        !existing
            || s.current().is_some_and(|c| {
                c.profile.is_local_cli()
                    && !s.owned_elsewhere(c.id)
                    && self.model.read(cx).local_cli_session(c.id).is_some()
            })
    }

    pub(super) fn open_local_cli(
        &mut self,
        existing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.local_cli_entry_enabled(existing, cx) {
            return;
        }
        let command = if existing {
            // Capture both IDs before dispatch/async work; never consult selection on reply.
            let id = self.state(cx).current().unwrap().id;
            let session = self.model.read(cx).local_cli_session(id).unwrap().clone();
            C::BeginExisting {
                conversation: id,
                session,
            }
        } else {
            C::BeginNew(LocalCliDraft::default())
        };
        self.dispatch(CoreCommand::LocalCli(command), cx);
        self.reconcile_local_cli_inputs(cx);
        self.open_overlay(Overlay::LocalCli, window, cx);
    }

    pub(super) fn local_cli_entry(
        &self,
        id: &'static str,
        existing: bool,
        cx: &Context<Self>,
    ) -> Btn {
        let this = cx.entity();
        Btn::new(id)
            .label(if existing { REAPPROVE_LABEL } else { NEW_LABEL })
            .disabled(!self.local_cli_entry_enabled(existing, cx))
            .on_click(move |window, cx| {
                this.update(cx, |w, cx| w.open_local_cli(existing, window, cx))
            })
    }

    pub(super) fn reconcile_local_cli_inputs(&mut self, cx: &mut Context<Self>) {
        let s = self.state(cx);
        let draft = s.local_cli.draft.clone();
        let editable = s.local_cli_availability().edit;
        let existing = matches!(s.local_cli.target, Some(LocalCliTarget::Existing { .. }));
        for (editor, text, disabled) in [
            (&self.local_cli_executable, draft.executable, !editable),
            (
                &self.local_cli_project,
                draft.project,
                !editable || existing,
            ),
        ] {
            editor.update(cx, |e, cx| {
                // set_text deliberately does not emit Changed. Never dispatch Edit on reconcile.
                if e.text() != text {
                    e.set_text(&text, cx);
                }
                e.set_disabled(disabled, cx);
            });
        }
    }

    pub(super) fn on_local_cli_input(
        &mut self,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Changed => {
                let mut draft = self.state(cx).local_cli.draft.clone();
                draft.executable = self.local_cli_executable.read(cx).text().to_owned();
                draft.project = self.local_cli_project.read(cx).text().to_owned();
                self.dispatch(CoreCommand::LocalCli(C::Edit(draft)), cx);
            }
            ComposerEvent::Escape => self.close_overlay(window, cx),
            // Enter must never silently advance inspection, approval or creation.
            _ => {}
        }
    }

    fn cli_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        enabled: bool,
        command: C,
        cx: &Context<Self>,
    ) -> Btn {
        let this = cx.entity();
        Btn::new(id)
            .label(label)
            .kind(BtnKind::Subtle)
            .disabled(!enabled)
            .on_click(move |window, cx| {
                this.update(cx, |w, cx| {
                    w.dispatch(CoreCommand::LocalCli(command.clone()), cx);
                    if matches!(&command, C::ReturnToConversation) {
                        w.close_overlay(window, cx);
                    } else if !matches!(&command, C::Edit(_)) && w.overlay == Overlay::LocalCli {
                        // Requests disable/remove their initiating button. Keep keyboard
                        // navigation on a stable, non-actionable modal anchor, never on
                        // the next trust/approval control.
                        window.focus(&w.menu_focus, cx);
                    }
                })
            })
    }

    fn cli_trust(
        &self,
        executable: bool,
        checked: bool,
        enabled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity();
        let label = if executable {
            "I trust this captured executable"
        } else {
            "I trust this captured project"
        };
        let command = if executable {
            C::TrustExecutable(!checked)
        } else {
            C::TrustProject(!checked)
        };
        let key_command = command.clone();
        let key_this = this.clone();
        div()
            .id(if executable {
                "cli-trust-executable"
            } else {
                "cli-trust-project"
            })
            .role(Role::CheckBox)
            .aria_label(format!(
                "{label} ({})",
                if checked { "checked" } else { "unchecked" }
            ))
            .aria_selected(checked)
            .flex()
            .gap(px(8.0))
            .p(px(8.0))
            .child(if checked { "☑" } else { "☐" })
            .child(label)
            .when(!enabled, |d| d.opacity(0.4))
            .when(enabled, |d| {
                d.tab_index(0)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        this.update(cx, |w, cx| {
                            w.dispatch(CoreCommand::LocalCli(command.clone()), cx)
                        })
                    })
                    .capture_key_down(move |ev, window, cx| {
                        if matches!(ev.keystroke.key.as_str(), "space" | "enter")
                            && !ev.keystroke.modifiers.modified()
                        {
                            // Prevent GPUI's key-up click from toggling trust a second time.
                            window.prevent_default();
                            if !ev.is_held {
                                key_this.update(cx, |w, cx| {
                                    w.dispatch(CoreCommand::LocalCli(key_command.clone()), cx)
                                });
                            }
                            cx.stop_propagation();
                        }
                    })
            })
    }

    pub(super) fn render_local_cli(&self, cx: &Context<Self>) -> impl IntoElement {
        let state = self.state(cx);
        let s = &state.local_cli;
        let a = state.local_cli_availability();
        let existing = matches!(s.target, Some(LocalCliTarget::Existing { .. }));
        let this = cx.entity();
        let mut panel = elevated(cx)
            .id("local-cli-setup")
            .track_focus(&self.menu_focus.clone().tab_index(0).tab_stop(false))
            .tab_group().tab_stop(false)
            .track_scroll(&self.cli_dialog_scroll)
            .capture_key_down(cx.listener(Self::cli_dialog_key))
            .role(Role::Dialog)
            .aria_label("Local CLI setup")
            .w(px(620.0))
            .max_h(px(650.0))
            .overflow_y_scroll()
            .p(px(20.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(if existing { REAPPROVE_LABEL } else { NEW_LABEL })
                    .child(
                        Btn::new("cli-close")
                            .label("Close")
                            .on_click(move |window, cx| {
                                this.update(cx, |w, cx| w.close_overlay(window, cx))
                            }),
                    ),
            )
            .child(if s.draft.mode == CliMode::ChatOnly {
                LIMITS
            } else {
                WRITE_LIMITS
            })
            .child(LEASE)
            .child(BILLING)
            .child(
                div()
                    .text_color(cx.theme().colors.text_muted)
                    .child(availability_label(state)),
            )
            .child(stage_label(s.stage))
            .child("Tab / Shift+Tab moves focus; Page Up / Down scrolls this dialog. These keys never check or approve anything.")
            .child("1 · Explicit executable path")
            .child(self.local_cli_executable.clone())
            .child(if existing {
                "Project is captured from the existing session"
            } else {
                "Explicit project path"
            })
            .child(self.local_cli_project.clone());
        let mut modes = div().flex().gap(px(8.0)).child("Immutable mode:");
        for (id, label, mode) in [
            ("cli-chat-mode", "Chat only", CliMode::ChatOnly),
            (
                "cli-write-mode",
                "Experimental Write only",
                CliMode::RestrictedWrite,
            ),
        ] {
            let mut draft = s.draft.clone();
            draft.mode = mode;
            modes = modes.child(
                self.cli_button(id, label, a.edit && !existing, C::Edit(draft), cx)
                    .selected(s.draft.mode == mode),
            );
        }
        panel = panel.child(modes);
        let mut aliases = div().flex().gap(px(8.0)).child("Fixed CLI alias:");
        for (id, label, alias) in [
            ("cli-haiku", "Haiku", CliModel::Haiku),
            ("cli-sonnet", "Sonnet", CliModel::Sonnet),
            ("cli-opus", "Opus", CliModel::Opus),
        ] {
            let mut draft = s.draft.clone();
            draft.model = alias;
            aliases = aliases.child(
                self.cli_button(id, label, a.edit && !existing, C::Edit(draft), cx)
                    .selected(s.draft.model == alias),
            );
        }
        panel = panel.child(aliases);
        if existing {
            panel = panel.child(self.cli_button(
                "cli-session-status",
                "Load saved session hints (read-only)",
                a.session_status,
                C::SessionStatus,
                cx,
            ));
            if let Some(hints) = &s.session_status {
                panel = panel.child(format!(
                    "Saved hints (not approval): executable {} · project {} · context valid: {}",
                    hints.executable, hints.project, hints.context_valid
                ));
                if let Some(model) = hints.model {
                    panel = panel.child(self.cli_button(
                        "cli-use-hints",
                        "Use saved paths, mode and alias",
                        a.edit && hints.mode != CliMode::Unsupported,
                        C::Edit(LocalCliDraft {
                            mode: hints.mode,
                            executable: hints.executable.clone(),
                            project: hints.project.clone(),
                            model,
                        }),
                        cx,
                    ));
                }
            }
        }
        if let Some(target @ LocalCliTarget::Existing { .. }) = &s.target {
            panel = panel.child(self.cli_button("cli-fresh-setup", "Start separate fresh setup (hints only)", a.fresh_setup, C::FreshSetup(target.clone()), cx))
                .child("Wait for the source run to settle; after a crash inspect processes and files first. Fresh setup retires any current setup ticket and copies metadata only: no transcript, prompts, tool arguments, goals, trust or session UUID. Inspect, trust, Check, Create and sending a new message remain separate explicit steps.");
        }
        let (conversation, generation) = state.local_cli_diagnostic_scope();
        panel = panel
            .child(state.local_cli_recovery_guidance())
            .child(self.cli_button(
                "cli-copy-diagnostics",
                "Copy redacted diagnostics (no probes)",
                s.target.is_some(),
                C::CopyDiagnostics {
                    conversation,
                    generation,
                },
                cx,
            ));
        panel = panel.child(self.cli_button("cli-inspect", "Inspect", a.inspect, C::Inspect, cx));
        if let Some(ticket) = &s.ticket {
            panel = panel
                .child("2 · Captured inspection — approve these exact resolved paths")
                .child(format!("Executable: {}", ticket.executable))
                .child(format!("Project: {}", ticket.project))
                .child(format!("Qualified version: {}", ticket.qualified_version))
                .child(format!("Captured mode: {:?}", ticket.mode))
                .child(self.cli_trust(true, s.trust_executable, a.trust, cx))
                .child(self.cli_trust(false, s.trust_project, a.trust, cx));
            if let Some(reason) = &ticket.reason {
                panel = panel.child(reason.clone());
            }
        }
        panel = panel.child(div().flex().gap(px(8.0))
            .child(self.cli_button("cli-check", "Check", a.check, C::Check, cx))
            .child(self.cli_button("cli-status", "Check setup status", a.status, C::Status, cx))
            .child(self.cli_button("cli-revoke", "Revoke approval", a.revoke, C::Revoke, cx)))
            .child("3 · Metadata verification is not permission to send. The authoritative conversation profile and readiness control the composer.")
            .child(if existing { self.cli_button("cli-return", "Return to conversation", a.return_to_conversation, C::ReturnToConversation, cx) } else { self.cli_button("cli-create", "Create", a.create, C::Create, cx) });
        if let Some(identity) = &s.creation {
            panel = panel.child(format!("Captured creation identity: {} / {}", identity.session.0, identity.setup.0))
                .child("Closing only hides this dialog. Unresolved creation is retained; nothing is automatically checked or resent.")
                .child(self.cli_button("cli-lookup", "Look up creation (read-only)", a.lookup_creation, C::LookupCreation, cx))
                .child(self.cli_button("cli-repeat", "Repeat Create with same identity (absence confirmed)", a.repeat_create, C::RepeatCreate, cx))
                .child(self.cli_button("cli-park", "Keep recovery record and start another setup", a.park_creation, C::ParkCreationAndBeginNew(LocalCliDraft::default()), cx))
                .child("Parking requires an explicit lookup returning no result. Absence is not final; nothing is automatically looked up or created.");
        }
        panel = panel.child(format!(
            "Retained recovery records: {} / {LOCAL_CLI_RECOVERY_CAPACITY}. At capacity, a new pair cannot be parked; the active identity stays retained. Creation identities are saved before Create and survive restart. Recovery never restores trust or automatically performs lookup.",
            s.parked_creations().len()
        ));
        if s.recovery_unavailable {
            panel = panel.child("Creation disabled: saved recovery identities could not be loaded. Do not repeat an uncertain creation.");
        }
        for (index, identity) in s.parked_creations().iter().enumerate() {
            panel = panel
                .child(format!(
                    "Session: {} · Setup: {}",
                    identity.session.0, identity.setup.0
                ))
                .child(self.cli_button(
                    ("cli-recover", index),
                    "Select recovery identity",
                    a.recover_creation,
                    C::RecoverCreation(identity.clone()),
                    cx,
                ));
        }
        if !s.parked_creations().is_empty() {
            panel = panel.child("Selecting recovery retires the current setup approval and can interrupt a run using it. Only the exact identity is restored, never approval or trust. Choose Look up creation explicitly next; recovery cannot create or repeat a creation.");
        }
        if let Some(error) = &s.error {
            panel = panel.child(format!("Setup result: {error:?}"));
        }
        panel
    }
}

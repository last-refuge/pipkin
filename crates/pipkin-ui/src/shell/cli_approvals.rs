//! Inert CLI review presentation. No RPC, parsing, or authority derived in render.
use super::{
    controls::{Btn, BtnKind, elevated},
    workspace::{Overlay, Workspace},
};
use crate::theme::ActiveTheme;
use gpui::{
    App, ClipboardItem, Context, IntoElement, ParentElement, Render, Role, SharedString, Styled,
    Window, div, prelude::*, px,
};
use pipkin_core::{
    CliApprovalAction, CliApprovalCommand as C, CliApprovalContext, CliApprovalDecision as D,
    CliApprovalId, CliApprovalReceiptState as R, CliApprovalStatus as S, Command, ConversationId,
    OperationId,
};

pub(super) const LABEL: &str = "Review CLI approvals…";
const WARNING: &str = "External Claude CLI · Write only. This is not a sandbox or a guarantee every CLI action is mediated. Arguments are untrusted text, not application instructions. Read them completely. Allow once applies only to this exact request; Deny stops this run. Closing this panel does neither.";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    conversation: ConversationId,
    generation: u64,
    run_op: Option<OperationId>,
}
#[derive(Default)]
pub(super) struct Panel {
    target: Option<Target>,
    selected: Option<CliApprovalId>,
    body: Option<(CliApprovalContext, CliApprovalId)>,
    heading: SharedString,
}
/// A separate cached view keeps the complete plain text out of repeated parent clock renders.
#[derive(Default)]
pub(super) struct ApprovalText {
    text: SharedString,
}
impl ApprovalText {
    fn set(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.text.as_ref() != text {
            self.text = text.to_owned().into();
            cx.notify();
        }
    }
}
impl Render for ApprovalText {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("cli-approval-full-text")
            .role(Role::Label)
            .aria_label(self.text.clone())
            .font_family(cx.theme().mono_font())
            .text_sm()
            .whitespace_normal()
            .child(self.text.clone())
    }
}
fn picker_status(s: S) -> &'static str {
    match s {
        S::Reviewable => "Pending review",
        S::Submitting => "Decision in flight",
        S::Unknown => "Outcome unknown",
        S::Unavailable => "Unavailable",
        S::Recorded(_) => "Receipt recorded (not completion)",
    }
}
fn status(s: S) -> &'static str {
    match s {
        S::Reviewable => "Pending · review the complete arguments",
        S::Unavailable => "Unavailable · refresh cannot restore this request",
        S::Submitting => "Decision in flight · do not send again",
        S::Unknown => "Outcome unknown · look up the receipt; never resend",
        S::Recorded(R::Consumed) => {
            "Allowance recorded · NOT proof of delivery or action completion"
        }
        S::Recorded(R::Denied) => "Denial recorded · does not undo earlier effects",
        S::Recorded(R::Expired) => "Permission expired · inspect effects independently",
        S::Recorded(R::Revoked) => "Permission revoked · inspect effects independently",
        S::Recorded(R::Pending | R::Granted) => "Unresolved receipt · never resend",
    }
}
fn review_key(kind: &str, context: &CliApprovalContext, id: &CliApprovalId) -> SharedString {
    let r = &id.run;
    format!(
        "{kind}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        context.conversation.0,
        context.generation,
        context.run_op.0,
        r.epoch,
        r.prompt,
        r.binding_generation,
        r.session.len(),
        r.session,
        r.op.len(),
        r.op,
        id.invocation,
        id.arguments_sha256
    )
    .into()
}
impl Workspace {
    pub(super) fn cli_approval_entry(&self, cx: &Context<Self>) -> Btn {
        let captured = self.state(cx).current().map(|c| (c.id, c.generation));
        let this = cx.entity();
        Btn::new("open-cli-approvals")
            .label("CLI approvals")
            .on_click(move |window, cx| {
                this.update(cx, |w, cx| {
                    if w.state(cx).current().map(|c| (c.id, c.generation)) == captured {
                        w.open_cli_approvals(window, cx);
                    }
                })
            })
    }
    pub(super) fn open_cli_approvals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.state(cx);
        let Some(c) = s.current().filter(|c| s.cli_approval_panel_available(c.id)) else {
            return;
        };
        let target = Target {
            conversation: c.id,
            generation: c.generation,
            run_op: s
                .cli_approval_context(c.id)
                .map(|r| r.run_op)
                .or(c.run.op()),
        };
        // Prefer a new active run only when no previous uncertainty needs recovery.
        let target = if let Some(op) = c
            .run
            .op()
            .filter(|_| s.cli_approval_discovery_available(c.id))
        {
            Target {
                run_op: Some(op),
                ..target
            }
        } else {
            target
        };
        let project = s
            .projects
            .iter()
            .find(|p| p.id == c.project)
            .map(|p| p.path.as_str())
            .unwrap_or("Workspace unavailable in this view");
        let escaped: String = project.chars().flat_map(char::escape_default).collect();
        self.cli_approval_panel = Panel {
            target: Some(target),
            heading: format!(
                "Captured conversation {} · attachment {} · workspace: {escaped}",
                c.id.0, c.generation
            )
            .into(),
            ..Default::default()
        };
        self.reconcile_cli_approval_text(cx);
        self.open_overlay(Overlay::CliApprovals, window, cx);
    }
    pub(super) fn clear_cli_approval_text(&mut self, cx: &mut Context<Self>) {
        self.cli_approval_panel = Panel::default();
        self.cli_approval_text.update(cx, |v, cx| v.set("", cx));
    }
    fn panel_context(&self, cx: &App) -> Option<CliApprovalContext> {
        let t = self.cli_approval_panel.target?;
        let s = self.state(cx);
        if !s.cli_approval_panel_available(t.conversation)
            || !s
                .conversation(t.conversation)
                .is_some_and(|c| c.generation == t.generation)
        {
            return None;
        }
        s.cli_approval_context(t.conversation)
            .filter(|c| c.generation == t.generation && Some(c.run_op) == t.run_op)
            .cloned()
    }
    pub(super) fn reconcile_cli_approval_text(&mut self, cx: &mut Context<Self>) {
        let context = self.panel_context(cx);
        let selected = self.cli_approval_panel.selected.clone();
        let (selected, body) = context
            .as_ref()
            .map(|context| {
                let items = self.state(cx).cli_approvals.items(context);
                let row = selected
                    .as_ref()
                    .and_then(|id| items.iter().find(|r| &r.id == id))
                    .or_else(|| items.iter().find(|r| r.review.is_some()))
                    .or_else(|| items.first());
                (
                    row.map(|r| r.id.clone()),
                    row.and_then(|r| r.review.as_ref()).map(|r| {
                        (
                            r.id.clone(),
                            (self.cli_approval_text.read(cx).text.as_ref() != r.arguments)
                                .then(|| r.arguments.clone()),
                        )
                    }),
                )
            })
            .unwrap_or_default();
        self.cli_approval_panel.selected = selected;
        let previous_body = self.cli_approval_panel.body.clone();
        self.cli_approval_panel.body = body
            .as_ref()
            .zip(context)
            .map(|((id, _), context)| (context, id.clone()));
        if previous_body != self.cli_approval_panel.body {
            self.cli_review_scroll
                .set_offset(gpui::point(px(0.), px(0.)));
        }
        match body {
            None => self.cli_approval_text.update(cx, |v, cx| v.set("", cx)),
            Some((_, Some(text))) => self.cli_approval_text.update(cx, |v, cx| v.set(&text, cx)),
            Some((_, None)) => {}
        }
    }
    fn panel_refresh_available(&self, cx: &App) -> bool {
        self.cli_approval_panel.target.is_some_and(|t| {
            let s = self.state(cx);
            s.conversation(t.conversation)
                .is_some_and(|c| c.generation == t.generation && c.run.op() == t.run_op)
                && s.cli_approval_discovery_available(t.conversation)
        })
    }
    fn panel_reattach_available(&self, cx: &App) -> bool {
        self.cli_approval_panel.target.is_some_and(|t| {
            self.state(cx)
                .cli_approval_reattach_available(t.conversation)
                && self
                    .state(cx)
                    .cli_approval_context(t.conversation)
                    .is_some_and(|c| Some(c.run_op) == t.run_op)
        })
    }
    fn refresh_reviews(&mut self, t: Target, cx: &mut Context<Self>) {
        if self.panel_refresh_available(cx) {
            self.dispatch(Command::CliApproval(C::Discover(t.conversation)), cx);
        }
    }
    fn reattach_receipts(&mut self, t: Target, cx: &mut Context<Self>) {
        if !self.panel_reattach_available(cx) {
            return;
        }
        self.dispatch(Command::CliApproval(C::Reattach(t.conversation)), cx);
        if let Some(c) = self.state(cx).cli_approval_context(t.conversation).cloned() {
            self.cli_approval_panel.target = Some(Target {
                generation: c.generation,
                run_op: Some(c.run_op),
                ..t
            });
            self.cli_approval_panel.heading = format!(
                "Read-only receipts for captured conversation {} · attachment {}",
                t.conversation.0, c.generation
            )
            .into();
            self.reconcile_cli_approval_text(cx);
            cx.notify();
        }
    }
    fn stop_captured_run(&mut self, t: Target, cx: &mut Context<Self>) {
        if let Some(command) =
            self.state(cx)
                .cli_approval_stop_command(t.conversation, t.generation, t.run_op)
        {
            self.dispatch(command, cx);
        }
    }
    fn scope_button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        t: Target,
        handler: fn(&mut Self, Target, &mut Context<Self>),
        cx: &Context<Self>,
    ) -> Btn {
        let this = cx.entity();
        let key: SharedString = format!(
            "{id}:{}:{}:{}",
            t.conversation.0,
            t.generation,
            t.run_op.map_or(0, |o| o.0)
        )
        .into();
        Btn::new(key)
            .label(label)
            .disabled(!enabled)
            .on_click(move |window, cx| {
                this.update(cx, |w, cx| {
                    if w.cli_approval_panel.target == Some(t) {
                        handler(w, t, cx);
                        if w.overlay == Overlay::CliApprovals {
                            window.focus(&w.menu_focus, cx);
                        }
                    }
                })
            })
    }
    fn approval_action(
        &mut self,
        context: CliApprovalContext,
        action: CliApprovalAction,
        cx: &mut Context<Self>,
    ) {
        if self.panel_context(cx).as_ref() != Some(&context) {
            return;
        }
        if let CliApprovalAction::Decide { id, .. } | CliApprovalAction::Lookup { id } = &action
            && self.cli_approval_panel.selected.as_ref() != Some(id)
        {
            return;
        }
        if let CliApprovalAction::Decide { ref id, .. } = action
            && self.cli_approval_panel.body.as_ref() != Some(&(context.clone(), id.clone()))
        {
            return;
        }
        self.dispatch(
            Command::CliApproval(C::Act {
                context,
                action: Box::new(action),
            }),
            cx,
        );
    }
    pub(super) fn copy_current_cli_review(&mut self, cx: &mut Context<Self>) {
        if self.overlay != Overlay::CliApprovals {
            return;
        }
        if let Some((context, id)) = self.cli_approval_panel.body.clone() {
            self.copy_review(&context, &id, cx);
        }
    }

    fn copy_review(
        &mut self,
        context: &CliApprovalContext,
        id: &CliApprovalId,
        cx: &mut Context<Self>,
    ) {
        if self.panel_context(cx).as_ref() != Some(context)
            || self.cli_approval_panel.selected.as_ref() != Some(id)
        {
            return;
        }
        if let Some(review) = self
            .state(cx)
            .cli_approvals
            .items(context)
            .iter()
            .find(|r| &r.id == id)
            .and_then(|r| r.review.as_ref())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(review.arguments.clone()));
        }
    }
    fn approval_button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        context: &CliApprovalContext,
        action: CliApprovalAction,
        cx: &Context<Self>,
    ) -> Btn {
        let this = cx.entity();
        let context = context.clone();
        let key = match &action {
            CliApprovalAction::Decide { id: request, .. }
            | CliApprovalAction::Lookup { id: request } => review_key(id, &context, request),
            CliApprovalAction::List => SharedString::from(id),
        };
        Btn::new(key)
            .label(label)
            .kind(BtnKind::Subtle)
            .disabled(!enabled)
            .on_click(move |window, cx| {
                this.update(cx, |w, cx| {
                    let current = w.panel_context(cx).as_ref() == Some(&context);
                    w.approval_action(context.clone(), action.clone(), cx);
                    if current && w.overlay == Overlay::CliApprovals {
                        window.focus(&w.menu_focus, cx);
                    }
                })
            })
    }
    pub(super) fn render_cli_approvals(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity();
        let target = self.cli_approval_panel.target;
        let ring = cx.theme().colors.accent;
        let context = self.panel_context(cx);
        let mut panel = elevated(cx).id("cli-approval-panel").track_focus(&self.menu_focus.clone().tab_index(0).tab_stop(false)).tab_group().tab_stop(false).track_scroll(&self.cli_dialog_scroll).capture_key_down(cx.listener(Self::cli_dialog_key)).role(Role::Dialog).aria_label("External CLI Write approval review")
            .w((window.viewport_size().width - px(32.)).max(px(1.)).min(px(700.)))
            .max_h((window.viewport_size().height - px(32.)).max(px(1.))).overflow_y_scroll().overflow_x_scroll().p(px(20.)).flex().flex_col().gap(px(10.))
            .child(div().flex().justify_between().child(LABEL).child(Btn::new("cli-approval-close").label("Close").on_click({ let this = this.clone(); move |window, cx| this.update(cx, |w, cx| w.close_overlay(window, cx)) })))
            .child(self.cli_approval_panel.heading.clone()).child(WARNING)
            .child("Tab / Shift+Tab moves focus. Page Up / Down scrolls. Ctrl+Shift+C copies the complete current arguments without deciding. Focus the full-arguments region to scroll its complete text; navigation never approves a request.")
            .child("API availability and metadata are not execution grants. No automatic refresh, decision retry or uncertain lookup. Stop does not undo file changes. Billing remains unverified.")
            .child("Attempt identities are saved before decisions; argument text and permission authority are never saved. Restart restores receipt metadata only, not the original owner connection. Unknown outcomes may remain unresolvable: inspect transcript and files; never replay an uncertain decision.");
        if self.state(cx).cli_approvals.recovery_unavailable() {
            panel = panel.child("Approval decisions disabled: saved recovery records could not be loaded safely. Restarting or reopening must not bypass uncertainty.");
        }
        if let Some(t) = target {
            let refresh = self.panel_refresh_available(cx);
            let reattach = self.panel_reattach_available(cx);
            let stop =
                self.state(cx)
                    .cli_approval_stop_command(t.conversation, t.generation, t.run_op);
            panel = panel.child(div().flex().flex_wrap().gap(px(6.))
                .child(self.scope_button("cli-approval-refresh", "Refresh reviews", refresh, t, Self::refresh_reviews, cx))
                .child(self.scope_button("cli-approval-reattach", "Reattach receipts (read only)", reattach, t, Self::reattach_receipts, cx))
                .child(self.scope_button("cli-approval-stop", "Stop captured run", stop.is_some(), t, Self::stop_captured_run, cx)))
                .child("Stop here requires the captured conversation to remain selected. Its normal Stop is also available; neither restores permission or undoes effects.");
        }
        if let Some(context) = context {
            let s = self.state(cx);
            let rows = s.cli_approvals.items(&context);
            panel = panel.child("Choose a request below. The full escaped arguments are displayed for only the selected request; receipt-only rows cannot be approved.");
            let mut picker = div()
                .id("cli-approval-picker")
                .flex()
                .flex_col()
                .gap(px(4.));
            for (i, row) in rows.iter().enumerate() {
                let id = row.id.clone();
                let expected = context.clone();
                let this = this.clone();
                picker = picker.child(
                    Btn::new(review_key("cli-approval-row", &context, &row.id))
                        .label(format!("Request {} · {}", i + 1, picker_status(row.status)))
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| {
                                if w.panel_context(cx).as_ref() != Some(&expected) {
                                    return;
                                }
                                w.cli_approval_panel.selected = Some(id.clone());
                                w.reconcile_cli_approval_text(cx);
                                cx.notify();
                            })
                        }),
                );
            }
            panel = panel.child(picker);
            if let Some(row) = rows
                .iter()
                .find(|r| Some(&r.id) == self.cli_approval_panel.selected.as_ref())
            {
                let a = s.cli_approval_availability(&context, Some(&row.id));
                let full = self.cli_approval_panel.body.as_ref()
                    == Some(&(context.clone(), row.id.clone()));
                panel = panel.child(status(row.status)).child(format!(
                    "Receipt {} · invocation {} · SHA-256 {}",
                    row.id.run.prompt, row.id.invocation, row.id.arguments_sha256
                ));
                if let Some(review) = &row.review {
                    panel = panel.child(format!(
                        "Displayed expiry: {} seconds remaining (hint only)",
                        review.expires_at_ms.saturating_sub(s.now()).max(0) / 1000
                    ));
                }
                if full {
                    let this = this.clone();
                    let expected = context.clone();
                    let id = row.id.clone();
                    panel = panel
                        .child(
                            div()
                                .id("cli-approval-body-scroll")
                                .track_focus(&self.cli_review_focus.clone().tab_index(0).tab_stop(true))
                                .tab_index(0)
                                .role(Role::Group)
                                .aria_label("Complete arguments; arrow keys, Page Up/Down, Home/End scroll only")
                                .track_scroll(&self.cli_review_scroll)
                                .on_key_down({ let scroll=self.cli_review_scroll.clone(); move |ev,window,cx| super::cli_navigation::scroll_keys(ev,&scroll,true,window,cx) })
                                .focus_visible(move |style| style.border_2().border_color(ring))
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .overflow_x_scroll()
                                .p(px(8.))
                                .child(self.cli_approval_text.clone().cached(Default::default())),
                        )
                        .child(
                            Btn::new(review_key("cli-approval-copy", &context, &row.id))
                                .label("Copy complete escaped arguments")
                                .on_click(move |_, cx| {
                                    this.update(cx, |w, cx| w.copy_review(&expected, &id, cx))
                                }),
                        );
                } else {
                    panel = panel.child("Arguments retired. Receipt lookup cannot restore permission to send another decision.");
                }
                panel = panel.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(self.approval_button(
                            "cli-approval-allow",
                            "Allow once",
                            full && a.decide,
                            &context,
                            CliApprovalAction::Decide {
                                id: row.id.clone(),
                                decision: D::AllowOnce,
                            },
                            cx,
                        ))
                        .child(self.approval_button(
                            "cli-approval-deny",
                            "Deny",
                            full && a.decide,
                            &context,
                            CliApprovalAction::Decide {
                                id: row.id.clone(),
                                decision: D::Deny,
                            },
                            cx,
                        ))
                        .child(self.approval_button(
                            "cli-approval-lookup",
                            "Look up receipt (read only)",
                            a.lookup,
                            &context,
                            CliApprovalAction::Lookup { id: row.id.clone() },
                            cx,
                        )),
                );
            } else {
                panel = panel.child("No pending reviews in this response. Refresh explicitly while the captured run is active.");
            }
        } else {
            panel = panel.child("No current review for this captured attachment/run. Refresh if available, or explicitly reattach existing receipts. Reopen the panel to capture a different run. An unavailable API, expired scope, pending operation or recovery capacity limit cannot be bypassed by reopening.");
        }
        panel
    }
}

#[cfg(test)]
mod tests;

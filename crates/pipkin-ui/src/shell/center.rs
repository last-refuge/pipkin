use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px,
};
use pipkin_core::*;

use super::controls::*;
use super::overlays::effort_label;
use super::workspace::{Overlay, Panel, Workspace};
use crate::theme::ActiveTheme;
use crate::transcript::tools::recent_activity;

/// How long ago `then` was, for a saved copy's label.
fn ago(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        0..=59 => "moments ago".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn fmt_size(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1048575 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1048576.0),
    }
}

impl Workspace {
    pub(super) fn render_center(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let nav_docked = self.nav_docked(window);
        let insp_docked = self.inspector_docked(window, cx);
        let (title, project, has_conv, demo, banner, storage_issue, extras) = {
            let s = self.state(cx);
            let conv = s.current();
            // What the engine's extensions and the saved history have to say, as small facts the
            // strips below are built from.
            let extras = (
                conv.and_then(|c| {
                    c.cached_at
                        .filter(|_| !c.opened)
                        .map(|at| (at, c.stale_reason.clone()))
                }),
                conv.map(|c| c.ui_notices.clone()).unwrap_or_default(),
                conv.map(|c| c.ui_status.clone()).unwrap_or_default(),
                s.search_return
                    .and_then(|id| s.conversation(id).map(|c| c.title.clone())),
                s.now(),
            );
            (
                s.current().map(|c| c.title.clone()),
                s.current_project()
                    .map(|p| format!("{} · {}", p.name, p.path))
                    .unwrap_or_default(),
                s.current().is_some(),
                s.mode == Mode::Demo,
                s.connection.banner(),
                s.storage_issue.clone(),
                extras,
            )
        };
        let this = cx.entity();
        let (saved_copy, ui_notices, ui_status, search_return, now) = extras;
        let waiting = self.waiting_question(cx, true);
        let question_open = self.overlay == Overlay::Question;
        let storage_strip = storage_issue.map(|message| {
            let this = this.clone();
            div().px(px(12.0)).pt(px(8.0)).flex_none().child(strip(
                cx,
                "triangle-alert",
                c.warning,
                "Saved data",
                &message,
                vec![
                    Btn::new("dismiss-storage-issue")
                        .icon("x")
                        .aria("Dismiss")
                        .compact()
                        .on_click(move |_, cx| {
                            this.update(cx, |t, cx| t.dispatch(Command::DismissStorageIssue, cx))
                        })
                        .into_any_element(),
                ],
            ))
        });
        let mut strips: Vec<gpui::AnyElement> = Vec::new();
        // A conversation is open but the engine is not answering: say so where it can be seen,
        // not only in the disabled buttons. (With none open, the centre already says it.)
        if has_conv && !demo {
            let conn = self.state(cx).connection.clone();
            let (glyph, color, title, hint) = match &conn {
                Connection::Failed(_) | Connection::Incompatible(_) => (
                    "circle-alert",
                    c.danger,
                    "The coding engine is not available",
                    " Sending is off. Use Copy diagnostics in the command palette for support details.",
                ),
                Connection::Offline(_) => (
                    "triangle-alert",
                    c.warning,
                    "Not connected to the coding engine",
                    " Retrying. Your drafts are kept.",
                ),
                Connection::Reconnecting => (
                    "clock",
                    c.warning,
                    "Connection lost",
                    " Reconnecting. Your drafts are kept.",
                ),
                _ => ("", c.text, "", ""),
            };
            if !title.is_empty() {
                let why = conn.banner().unwrap_or_default();
                strips.push(strip(
                    cx,
                    glyph,
                    color,
                    title,
                    &format!("{why}.{hint}"),
                    vec![],
                ));
            }
        }
        if !demo
            && self
                .state(cx)
                .current()
                .is_some_and(|cv| cv.recovery_notice)
        {
            let this = this.clone();
            strips.push(strip(
                cx,
                "triangle-alert",
                c.warning,
                "Recovery notice",
                RECOVERY_DETAIL,
                vec![
                    Btn::new("copy-recovery-diagnostics")
                        .icon("copy")
                        .label("Diagnostics")
                        .aria("Copy diagnostics: metadata only, no logs or conversation text")
                        .compact()
                        .on_click({
                            let this = this.clone();
                            move |window, cx| {
                                this.update(cx, |t, cx| {
                                    t.run_command(
                                        &super::commands::Run::CopyDiagnostics,
                                        window,
                                        cx,
                                    )
                                })
                            }
                        })
                        .into_any_element(),
                    Btn::new("dismiss-recovery-notice")
                        .icon("x")
                        .aria("Dismiss recovery notice")
                        .compact()
                        .on_click(move |_, cx| {
                            this.update(cx, |t, cx| t.dispatch(Command::DismissRecoveryNotice, cx))
                        })
                        .into_any_element(),
                ],
            ));
        }
        if let Some((at, reason)) = saved_copy {
            strips.push(strip(
                cx,
                "clock",
                c.warning,
                "Saved copy",
                &format!(
                    "Showing the conversation as it was saved {}. {}",
                    ago(now, at),
                    reason.unwrap_or_else(|| "Loading the live conversation…".into())
                ),
                vec![],
            ));
        }
        if let Some(back_to) = search_return {
            let this = this.clone();
            strips.push(strip(
                cx,
                "search",
                c.accent,
                "Opened from a search",
                &format!("You were in \u{201c}{back_to}\u{201d}."),
                vec![
                    Btn::new("return-from-search")
                        .label("Go back")
                        .kind(BtnKind::Subtle)
                        .compact()
                        .on_click(move |_, cx| {
                            this.update(cx, |t, cx| t.dispatch(Command::ReturnFromSearch, cx))
                        })
                        .into_any_element(),
                ],
            ));
        }
        if let Some(q) = waiting.as_ref().filter(|_| !question_open) {
            let (answer_this, decline_this) = (this.clone(), this.clone());
            let id = q.id.clone();
            strips.push(strip(
                cx,
                "circle-help",
                c.accent,
                "An extension is waiting for an answer",
                &q.title,
                vec![
                    Btn::new("question-answer")
                        .label("Answer")
                        .kind(BtnKind::Primary)
                        .compact()
                        .on_click(move |window, cx| {
                            answer_this.update(cx, |t, cx| {
                                t.question_dismissed.clear();
                                t.open_overlay(Overlay::Question, window, cx);
                            })
                        })
                        .into_any_element(),
                    Btn::new("question-decline-strip")
                        .label("Decline")
                        .kind(BtnKind::Subtle)
                        .compact()
                        .on_click(move |_, cx| {
                            decline_this.update(cx, |t, cx| {
                                t.dispatch(Command::CancelUiRequest(id.clone()), cx)
                            })
                        })
                        .into_any_element(),
                ],
            ));
        }
        if !ui_notices.is_empty() {
            let this = this.clone();
            // The latest three, oldest first, as they were posted.
            let lines: Vec<String> = ui_notices
                .iter()
                .skip(ui_notices.len().saturating_sub(3))
                .map(|n| n.message.clone())
                .collect();
            let level = ui_notices
                .iter()
                .map(|n| match n.level {
                    UiNoticeLevel::Error => 2,
                    UiNoticeLevel::Warning => 1,
                    UiNoticeLevel::Info => 0,
                })
                .max()
                .unwrap_or(0);
            let (icon_name, color) = match level {
                2 => ("circle-alert", c.danger),
                1 => ("triangle-alert", c.warning),
                _ => ("message-square", c.accent),
            };
            strips.push(strip(
                cx,
                icon_name,
                color,
                "From an extension",
                &lines.join("\n"),
                vec![
                    Btn::new("dismiss-ui-notices")
                        .icon("x")
                        .aria("Dismiss")
                        .compact()
                        .on_click(move |_, cx| {
                            this.update(cx, |t, cx| t.dispatch(Command::DismissUiNotices, cx))
                        })
                        .into_any_element(),
                ],
            ));
        }
        let status_line = (!ui_status.is_empty()).then(|| {
            div()
                .id("extension-status")
                .role(Role::Status)
                .px(px(14.0))
                .py(px(2.0))
                .flex_none()
                .truncate()
                .text_size(t.small_size())
                .text_color(c.text_faint)
                .child(
                    ui_status
                        .iter()
                        .map(|(k, v)| format!("{k}: {v}"))
                        .collect::<Vec<_>>()
                        .join("  \u{b7}  "),
                )
        });
        let nav_toggle = (!nav_docked).then(|| {
            let this = this.clone();
            Btn::new("toggle-nav")
                .icon("panel-left")
                .aria("Show navigation")
                .selected(self.temp_panel == Some(Panel::Nav))
                .on_click(move |window, cx| {
                    this.update(cx, |this, cx| {
                        if this.temp_panel == Some(Panel::Nav) {
                            this.close_panel(window, cx)
                        } else {
                            this.open_panel(Panel::Nav, window, cx)
                        }
                    })
                })
        });
        let insp_toggle = {
            Btn::new("toggle-inspector")
                .icon("panel-right")
                .aria("Toggle workspace inspector")
                .selected(insp_docked || self.temp_panel == Some(Panel::Inspector))
                .on_click(move |window, cx| {
                    window.dispatch_action(Box::new(super::actions::ToggleInspector), cx)
                })
        };

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.0))
            // Same height and side padding as the changes pane's header, so their rules line up.
            .h(px(68.0 * t.scale.max(1.0)))
            .px(px(24.0))
            .border_b_1()
            .border_color(c.border)
            .children(nav_toggle)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("conv-title")
                            .role(Role::Heading)
                            .aria_label(
                                title
                                    .clone()
                                    .unwrap_or_else(|| "No conversation selected".into()),
                            )
                            .truncate()
                            .text_size(px(16.0 * t.scale))
                            .line_height(px(23.0 * t.scale))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(
                                title
                                    .clone()
                                    .unwrap_or_else(|| "No conversation selected".into()),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .mt(px(3.0))
                            .text_size(t.small_size())
                            .line_height(px(17.0 * t.scale))
                            .text_color(c.text_faint)
                            .child(project),
                    ),
            )
            .children(demo.then(|| chip("Demo · simulated agent", c.text_muted, c.bg_active, cx)))
            .when(self.subagent_rows.total > 0, |d| {
                d.child(
                    Btn::new("show-subagent-tasks")
                        .label(self.subagent_rows.tab_label())
                        .aria(format!(
                            "Show subagent tasks, {} active",
                            self.subagent_rows.active
                        ))
                        .selected(
                            self.inspector_tab == super::subagents::InspectorTab::Subagents
                                && (insp_docked || self.temp_panel == Some(Panel::Inspector)),
                        )
                        .on_click(|window, cx| {
                            window.dispatch_action(Box::new(super::actions::ShowSubagents), cx)
                        }),
                )
            })
            .child(insp_toggle);

        let body = if has_conv {
            div()
                .flex_1()
                .min_h_0()
                .child(self.transcript.clone())
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                // With nothing wrong to report, greet with the mascot; otherwise say what is.
                .when(banner.is_none(), |d| {
                    d.child(
                        gpui::img(crate::assets::MASCOT)
                            .w(px(126.0 * t.scale.max(1.0)))
                            .h(px(131.0 * t.scale.max(1.0)))
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                })
                .child(
                    div()
                        .text_size(if banner.is_none() {
                            px(18.0 * t.scale)
                        } else {
                            t.ui_size()
                        })
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(if banner.is_none() {
                            c.text
                        } else {
                            c.text_muted
                        })
                        .child(if banner.is_none() {
                            "Ready when you are."
                        } else {
                            "No conversation selected"
                        }),
                )
                .child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_faint)
                        .child(
                            banner
                                .unwrap_or_else(|| "Press Ctrl+N to start a conversation.".into()),
                        ),
                )
                .into_any_element()
        };

        let drop_tint = c.accent_bg;
        div()
            .id("conversation")
            .role(Role::Main)
            .aria_label("Conversation")
            // Files dropped from a file manager are attached to the message being written.
            .drag_over::<gpui::ExternalPaths>(move |style, _, _, _| style.bg(drop_tint))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                this.attach_paths(paths.paths().to_vec(), cx)
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(c.bg_surface)
            .child(header)
            .children(storage_strip)
            .children(
                strips
                    .into_iter()
                    .map(|s| div().px(px(12.0)).pt(px(8.0)).flex_none().child(s)),
            )
            .children(status_line)
            .child(body)
            .children(has_conv.then(|| self.render_bottom(window, cx)))
    }

    /// Run status, queue, and composer, anchored below the transcript.
    fn render_bottom(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let (
            run,
            queue,
            draft_save,
            attachments,
            avail,
            models,
            model_id,
            retry_hint,
            intent_error,
            pending_queue,
            real,
            activity,
        ) = {
            let s = self.state(cx);
            let cv = s.current().unwrap();
            (
                cv.run.clone(),
                cv.queue.clone(),
                cv.draft.save.clone(),
                cv.draft.attachments.clone(),
                s.availability(),
                s.models.clone(),
                s.prefs.model.clone(),
                cv.last_submission.is_some(),
                cv.intent_error.clone(),
                cv.pending_queue.clone(),
                s.mode == Mode::Real,
                recent_activity(&cv.items),
            )
        };
        let _ = retry_hint;
        let stop_error = self
            .state(cx)
            .current()
            .and_then(|cv| cv.stop_error.clone());
        let status_error = self
            .state(cx)
            .current()
            .and_then(|cv| cv.status_error.clone());
        let check_status = || {
            Btn::new("check-status")
                .icon("refresh-cw")
                .label("Check status")
                .kind(BtnKind::Subtle)
                .compact()
                .disabled(!avail.check_status)
                .on_click({
                    let this = cx.entity();
                    move |_, cx| this.update(cx, |t, cx| t.dispatch(Command::CheckStatus, cx))
                })
                .into_any_element()
        };
        let goal = self.state(cx).current().and_then(|cv| cv.goal.clone());
        // The plan strip: task counts only, with the open tasks' titles, never a prediction.
        let plan_strip = self
            .state(cx)
            .current()
            .map(|cv| &cv.plan)
            .filter(|p| p.available && p.counts.live() > 0)
            .map(|p| {
                let mut titles: Vec<String> = p
                    .tasks
                    .iter()
                    .filter(|t| t.status == pipkin_core::PlanTaskStatus::Active)
                    .take(2)
                    .map(|t| t.title.clone())
                    .collect();
                if titles.is_empty() {
                    titles.extend(
                        p.tasks
                            .iter()
                            .filter(|t| !t.status.finished())
                            .take(1)
                            .map(|t| t.title.clone()),
                    );
                }
                let mut detail = super::plan::counts_line(&p.counts);
                if !titles.is_empty() {
                    detail.push_str(" · ");
                    detail.push_str(&titles.join(" · "));
                }
                detail
            });
        let no_models = models.is_empty();
        // This build cannot send, or the built-in engine owns this conversation.
        let read_only = match self.state(cx).current() {
            Some(cv) => self.state(cx).read_only_reason(cv.id),
            None => self.state(cx).read_only.clone(),
        };
        let model_name = models
            .iter()
            .find(|m| Some(&m.id) == model_id.as_ref())
            .map(|m| m.name.clone())
            .unwrap_or_else(|| {
                if models.is_empty() {
                    "No models available".into()
                } else {
                    "Choose model".into()
                }
            });
        let model_name = if real {
            match self
                .state(cx)
                .current()
                .and_then(|c| c.thinking_level.as_deref())
            {
                Some("off") | None => model_name,
                Some(level) => format!("{model_name} · {}", effort_label(level)),
            }
        } else {
            model_name
        };
        let this = cx.entity();

        // ---- not-sent strip: the text was not sent (or not queued) and is still in the composer
        let not_sent: Option<gpui::AnyElement> = intent_error.as_deref().map(|message| {
            strip(
                cx,
                "circle-alert",
                c.danger,
                "Not sent",
                message,
                vec![
                    Btn::new("dismiss-intent-error")
                        .icon("x")
                        .aria("Dismiss")
                        .compact()
                        .on_click({
                            let this = this.clone();
                            move |_, cx| {
                                this.update(cx, |t, cx| t.dispatch(Command::DismissFailure, cx))
                            }
                        })
                        .into_any_element(),
                ],
            )
        });

        // ---- status strip
        let status: Option<gpui::AnyElement> = match &run {
            // The engine has no model Pipkin can offer: say what to do about it.
            // Only when connected: offline, which models exist is not known.
            RunState::Idle if real && no_models && read_only.is_none() && avail.refresh_models => {
                let can_connect = self.model.read(cx).sign_in.available;
                let action = if can_connect {
                    Btn::new("connect-account")
                        .label("Connect account")
                        .kind(BtnKind::Primary)
                        .compact()
                        .on_click({
                            let model = self.model.clone();
                            move |_, cx| model.update(cx, |m, cx| m.open_account_setup(cx))
                        })
                        .into_any_element()
                } else {
                    Btn::new("refresh-models")
                        .icon("refresh-cw")
                        .label("Refresh models")
                        .kind(BtnKind::Subtle)
                        .compact()
                        .on_click({
                            let this = this.clone();
                            move |_, cx| {
                                this.update(cx, |t, cx| t.dispatch(Command::RefreshModels, cx))
                            }
                        })
                        .into_any_element()
                };
                Some(strip(
                    cx,
                    "circle-help",
                    c.warning,
                    "No model is ready",
                    if can_connect {
                        MODEL_SETUP_DETAIL
                    } else {
                        MODEL_UNAVAILABLE_DETAIL
                    },
                    vec![action],
                ))
            }
            // This build can read sessions but not run them: say so where sending is offered.
            RunState::Idle if read_only.is_some() => Some(strip(
                cx,
                "circle-help",
                c.text_muted,
                "Read-only",
                read_only.as_deref().unwrap_or_default(),
                vec![],
            )),
            RunState::Idle | RunState::Submitting { .. } => None,
            RunState::Running { .. } => Some(strip(
                cx,
                "loader-circle",
                c.accent,
                "Working…",
                activity
                    .as_deref()
                    .unwrap_or("Pipkin is working. You can steer, queue a follow-up, or stop."),
                vec![
                    Btn::new("stop-run")
                        .icon("square")
                        .label("Stop")
                        .kind(BtnKind::Danger)
                        .compact()
                        .disabled(!avail.cancel)
                        .on_click({
                            let this = this.clone();
                            move |_, cx| this.update(cx, |t, cx| t.dispatch(Command::Cancel, cx))
                        })
                        .into_any_element(),
                ],
            )),
            RunState::Stopping { .. } => {
                let detail = if real {
                    stopping_detail(stop_error.as_deref())
                } else {
                    "Waiting for the simulated run to stop.".into()
                };
                let mut actions = vec![];
                if real {
                    actions.push(check_status());
                    if stop_error.is_some() {
                        let this = this.clone();
                        actions.push(
                            Btn::new("retry-stop")
                                .icon("square")
                                .label("Retry stop")
                                .kind(BtnKind::Subtle)
                                .compact()
                                .disabled(!avail.retry_stop)
                                .on_click(move |_, cx| {
                                    this.update(cx, |t, cx| t.dispatch(Command::RetryStop, cx))
                                })
                                .into_any_element(),
                        );
                    }
                }
                Some(strip(
                    cx,
                    "loader-circle",
                    c.warning,
                    "Stopping…",
                    &detail,
                    actions,
                ))
            }
            RunState::OutcomeUnknown { .. } => Some(strip(
                cx,
                "circle-help",
                c.warning,
                "Outcome unknown",
                UNKNOWN_DETAIL,
                vec![check_status()],
            )),
            RunState::Failed { message } => Some(strip(
                cx,
                "circle-alert",
                c.danger,
                "Run failed",
                message,
                vec![
                    Btn::new("retry")
                        .icon("refresh-cw")
                        .label("Retry")
                        .kind(BtnKind::Subtle)
                        .compact()
                        .disabled(!avail.retry)
                        .on_click({
                            let this = this.clone();
                            move |_, cx| this.update(cx, |t, cx| t.dispatch(Command::Retry, cx))
                        })
                        .into_any_element(),
                    Btn::new("dismiss-failure")
                        .icon("x")
                        .aria("Dismiss")
                        .compact()
                        .on_click({
                            let this = this.clone();
                            move |_, cx| {
                                this.update(cx, |t, cx| t.dispatch(Command::DismissFailure, cx))
                            }
                        })
                        .into_any_element(),
                ],
            )),
        };

        // ---- queue
        // The engine's queue, then what was sent and not yet confirmed. Only what the engine
        // holds can be removed.
        let queue_total = queue.len() + pending_queue.len();
        let queue_el = (queue_total > 0).then(|| {
            let row = |i: usize, mode: QueueMode, text: String, note: Option<&'static str>| {
                let label = match mode {
                    QueueMode::Steer => "Steer",
                    QueueMode::FollowUp => "Follow-up",
                };
                div()
                    .id(("queued", i))
                    .role(Role::ListItem)
                    .aria_label(format!("{label}: {text}"))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_color(c.text_faint).child(format!("{}.", i + 1)))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.accent)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c.text_muted)
                            .child(text),
                    )
                    .children(note.map(|n| {
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_faint)
                            .child(n)
                    }))
            };
            let held = queue.len();
            div()
                .id("queue")
                .role(Role::List)
                .aria_label("Queued input")
                .flex()
                .flex_col()
                .gap(px(2.0))
                .py(px(6.0))
                .child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_faint)
                        .child(format!("Queued ({queue_total})")),
                )
                .children(queue.into_iter().enumerate().map(|(i, q)| {
                    let this = this.clone();
                    let qid = q.id;
                    row(i, q.mode, q.text, None).child(
                        Btn::new(("unqueue", qid.0 as usize))
                            .icon("x")
                            .compact()
                            .aria("Remove queued prompt")
                            .on_click(move |_, cx| {
                                this.update(cx, |t, cx| t.dispatch(Command::RemoveQueued(qid), cx))
                            }),
                    )
                }))
                .children(pending_queue.into_iter().enumerate().map(|(i, p)| {
                    let note = match p.state {
                        QueueSend::Sending => "Sending…",
                        QueueSend::Unknown => "Checking with the engine…",
                    };
                    row(held + i, p.mode, p.text, Some(note))
                }))
        });

        // ---- attachments
        let att_el = (!attachments.is_empty()).then(|| {
            div().flex().flex_wrap().gap(px(6.0)).pb(px(6.0)).children(
                attachments.into_iter().enumerate().map(|(i, a)| {
                    let this = this.clone();
                    let (fg, bg) = if a.error.is_some() {
                        (c.danger, c.danger_bg)
                    } else {
                        (c.text_muted, c.bg_active)
                    };
                    let label = match (&a.error, a.size) {
                        (Some(e), _) => format!("{} — {}", a.name, e),
                        (None, Some(sz)) => format!("{} · {}", a.name, fmt_size(sz)),
                        _ => a.name.clone(),
                    };
                    chip(label, fg, bg, cx).id(("att", i)).child(
                        Btn::new(("rm-att", i))
                            .icon("x")
                            .compact()
                            .aria(format!("Remove {}", a.name))
                            .on_click(move |_, cx| {
                                this.update(cx, |t, cx| {
                                    t.dispatch(Command::RemoveAttachment(i), cx)
                                })
                            }),
                    )
                }),
            )
        });

        // ---- save state line
        let save_line: Option<gpui::AnyElement> = match &draft_save {
            SaveState::Failed(e) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(4.0))
                    .text_size(t.small_size())
                    .text_color(c.danger)
                    .child(icon("triangle-alert", px(14.0), c.danger))
                    .child(
                        div()
                            .flex_1()
                            .child(format!("Draft not saved: {e}. Your text is still here.")),
                    )
                    .child({
                        let this = this.clone();
                        Btn::new("retry-save")
                            .icon("refresh-cw")
                            .label("Retry save")
                            .compact()
                            .disabled(!avail.retry_save)
                            .on_click(move |_, cx| {
                                this.update(cx, |t, cx| t.dispatch(Command::RetrySave, cx))
                            })
                    })
                    .child({
                        let this = this.clone();
                        Btn::new("copy-draft")
                            .icon("copy")
                            .label("Copy draft")
                            .compact()
                            .on_click(move |_, cx| this.update(cx, |t, cx| t.copy_draft(cx)))
                    })
                    .into_any_element(),
            ),
            _ => None,
        };
        let save_label = match draft_save {
            SaveState::Clean => "",
            SaveState::Dirty => "Unsaved",
            SaveState::Saving => "Saving…",
            SaveState::Saved => "Draft saved",
            SaveState::Failed(_) => "Not saved",
        };

        // ---- toolbar
        let running = matches!(run, RunState::Running { .. } | RunState::Submitting { .. });
        let primary: gpui::AnyElement = if running {
            Btn::new("steer")
                .icon("arrow-up")
                .label("Steer")
                .kind(BtnKind::Primary)
                .disabled(!avail.steer)
                .on_click({
                    let this = this.clone();
                    move |_, cx| this.update(cx, |t, cx| t.dispatch(Command::Steer, cx))
                })
                .into_any_element()
        } else {
            Btn::new("send")
                .icon("arrow-right")
                .aria("Send message (Enter)")
                .kind(BtnKind::Primary)
                .disabled(!avail.submit)
                .on_click({
                    let this = this.clone();
                    move |window, cx| this.update(cx, |t, cx| t.submit_primary(window, cx))
                })
                .into_any_element()
        };
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .pt(px(6.0))
            .child({
                let this = this.clone();
                Btn::new("attach")
                    .icon("paperclip")
                    .aria("Attach files (Ctrl+O)")
                    .on_click(move |_, cx| this.update(cx, |t, cx| t.attach_files(cx)))
            })
            .child({
                let this = this.clone();
                let measure = this.clone();
                div()
                    .relative()
                    .child(
                        Btn::new("model-menu")
                            .label(model_name)
                            .trailing_icon("chevron-up")
                            .aria("Model and effort settings (Ctrl+M for models)")
                            .selected(matches!(
                                self.overlay,
                                Overlay::ModelSettings | Overlay::Model | Overlay::Effort
                            ))
                            .disabled(no_models)
                            .on_click(move |window, cx| {
                                this.update(cx, |t, cx| {
                                    t.open_overlay(Overlay::ModelSettings, window, cx)
                                })
                            }),
                    )
                    // Remember where the button is, for the menu that opens above it.
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                measure.update(cx, |t, _| t.model_button = Some(bounds))
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
            })
            .when_some(
                self.render_status_line(
                    _window.viewport_size().width < px(760.0 * t.scale.max(1.0)),
                    cx,
                ),
                |d, line| d.child(line),
            )
            .child(div().flex_1())
            .when(!running && !real, |d| {
                d.child(
                    div()
                        .px(px(8.0))
                        .text_size(t.small_size())
                        .font_family(t.mono_font())
                        .text_color(c.text_faint)
                        .child("Enter to send \u{b7} Shift+Enter for a new line"),
                )
            })
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_faint)
                    .child(save_label),
            )
            .when(running, |d| {
                let this = this.clone();
                d.child(
                    Btn::new("queue-btn")
                        .label("Queue")
                        .kind(BtnKind::Subtle)
                        .disabled(!avail.queue)
                        .on_click(move |_, cx| {
                            this.update(cx, |t, cx| t.dispatch(Command::QueueFollowUp, cx))
                        }),
                )
            })
            .child(primary);

        let focused_ring = self
            .composer
            .read(cx)
            .focus_handle(cx)
            .contains_focused(_window, cx);

        div()
            .flex_none()
            .px(px(16.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.0 * t.scale.max(1.0)))
                    .flex()
                    .flex_col()
                    .children(not_sent)
                    .children(plan_strip.map(|detail| {
                        strip(
                            cx,
                            "circle-check",
                            c.text_muted,
                            "Plan",
                            &detail,
                            vec![
                                Btn::new("show-plan")
                                    .label("Show plan")
                                    .aria("Show the plan in the inspector")
                                    .compact()
                                    .on_click({
                                        let this = this.clone();
                                        move |window, cx| {
                                            this.update(cx, |_, cx| {
                                                window.dispatch_action(
                                                    Box::new(super::actions::ShowPlan),
                                                    cx,
                                                );
                                            })
                                        }
                                    })
                                    .into_any_element(),
                            ],
                        )
                    }))
                    .children(goal.map(|goal| {
                        let state = goal
                            .paused
                            .as_deref()
                            .unwrap_or("Working until met, blocked, or unreachable");
                        let mut detail = format!("{} · {}", goal.text, state);
                        if goal.native {
                            if goal.criteria_total > 0 {
                                detail.push_str(&format!(
                                    " · {} of {} criteria met",
                                    goal.criteria_met, goal.criteria_total
                                ));
                            }
                            match goal.max_turns {
                                Some(max) => {
                                    detail.push_str(&format!(" · turn {} of {}", goal.turns, max))
                                }
                                None if goal.turns > 0 => {
                                    detail.push_str(&format!(" · turn {}", goal.turns))
                                }
                                None => {}
                            }
                            if let Some(max) = goal.max_tokens {
                                detail.push_str(&format!(
                                    " · {} of {} tokens",
                                    goal.usage_tokens, max
                                ));
                            }
                            if let Some(max) = goal.max_cost_usd {
                                detail.push_str(&format!(
                                    " · ${:.2} of ${:.2}",
                                    goal.usage_cost_usd, max
                                ));
                            }
                            if goal.audit_required {
                                detail.push_str(" · audit required");
                            }
                            if let Some(verdict) = &goal.audit_verdict {
                                detail
                                    .push_str(&format!(" · audit: {}", verdict.replace('_', " ")));
                            }
                        }
                        let can_pause = goal.paused.is_none();
                        let can_resume = match goal.state {
                            Some(s) => s.resumable(),
                            None => goal.paused.is_some(),
                        };
                        let mut actions = Vec::new();
                        if can_pause {
                            actions.push(
                                Btn::new("pause-goal")
                                    .label("Pause")
                                    .aria("Pause the goal")
                                    .compact()
                                    .on_click({
                                        let this = this.clone();
                                        move |_, cx| {
                                            this.update(cx, |t, cx| {
                                                t.dispatch(Command::PauseGoal, cx)
                                            })
                                        }
                                    })
                                    .into_any_element(),
                            );
                        }
                        if can_resume {
                            actions.push(
                                Btn::new("resume-goal")
                                    .label("Resume")
                                    .aria("Resume the goal")
                                    .compact()
                                    .on_click({
                                        let this = this.clone();
                                        move |_, cx| {
                                            this.update(cx, |t, cx| {
                                                t.dispatch(Command::ResumeGoal, cx)
                                            })
                                        }
                                    })
                                    .into_any_element(),
                            );
                        }
                        actions.push(
                            Btn::new("clear-goal")
                                .icon("x")
                                .aria(if goal.native {
                                    "Archive goal and stop"
                                } else {
                                    "Clear goal and stop"
                                })
                                .compact()
                                .on_click({
                                    let this = this.clone();
                                    move |_, cx| {
                                        this.update(cx, |t, cx| t.dispatch(Command::ClearGoal, cx))
                                    }
                                })
                                .into_any_element(),
                        );
                        strip(cx, "zap", c.text_muted, "Goal", &detail, actions)
                    }))
                    .children(status)
                    .children(status_error.as_deref().map(|message| {
                        strip(
                            cx,
                            "circle-alert",
                            c.warning,
                            "Status check failed",
                            message,
                            vec![],
                        )
                    }))
                    .children(queue_el)
                    .child(
                        div()
                            .id("composer-box")
                            .role(Role::Group)
                            .aria_label("Message composer")
                            .flex()
                            .flex_col()
                            .p(px(12.0))
                            .rounded(px(9.0))
                            .bg(c.bg_input)
                            .border_1()
                            // Neutral like the mock; focus is a lighter edge, still clearly visible.
                            .border_color(if focused_ring {
                                c.text_faint
                            } else {
                                c.border_strong
                            })
                            .children(att_el)
                            .child(self.composer.clone())
                            .child(toolbar),
                    )
                    .children(save_line),
            )
    }
}

const MODEL_SETUP_DETAIL: &str = "Connect a Claude or ChatGPT subscription in Pipkin to choose a model. Your existing conversations stay here.";
const MODEL_UNAVAILABLE_DETAIL: &str = "No model is available. This engine does not offer in-app subscription sign-in; use an updated bundled engine, then refresh models.";
const RECOVERY_DETAIL: &str = "Pipkin does not resend the prompt. Some engines may repeat a partially executed tool during recovery. Review tool output, project Changes and external effects before sending more work. If unresolved, Check status reconciles with the engine; copy metadata-only diagnostics for support. Detailed CLI diagnostics may contain sensitive logs.";
const UNKNOWN_DETAIL: &str = "The app or connection stopped before acknowledgement. This does not prove the prompt or its tools never ran. Check status asks the engine what happened without resending; sending stays disabled until reconciled. Review project and external effects before repeating work.";

fn stopping_detail(error: Option<&str>) -> String {
    let waiting = "The engine has not confirmed that the run stopped; tools may still be running. If this takes longer than expected, Check status reads the engine state without resending the prompt.";
    match error {
        Some(error) => format!("{error} {waiting} Retry stop repeats only the stop request."),
        None => waiting.into(),
    }
}

pub(super) fn strip(
    cx: &gpui::App,
    icon_name: &'static str,
    color: gpui::Hsla,
    title: &str,
    detail: &str,
    actions: Vec<gpui::AnyElement>,
) -> gpui::AnyElement {
    let t = cx.theme();
    div()
        .id(gpui::SharedString::from(format!("run-status-{title}")))
        .role(Role::Status)
        .aria_label(format!("{title}. {detail}"))
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(8.0))
        .mb(px(8.0))
        .rounded(px(8.0))
        .bg(t.colors.bg_active)
        .child(icon(icon_name, px(16.0), color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_color(color)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(t.small_size())
                        .text_color(t.colors.text_muted)
                        .child(detail.to_string()),
                ),
        )
        .children(actions)
        .into_any_element()
}

#[cfg(test)]
mod recovery_copy_tests {
    use super::*;

    #[test]
    fn model_recovery_uses_gui_sign_in_without_sending_users_to_terminal_login() {
        assert!(MODEL_SETUP_DETAIL.contains("in Pipkin"));
        assert!(MODEL_SETUP_DETAIL.contains("existing conversations stay"));
        assert!(MODEL_UNAVAILABLE_DETAIL.contains("updated bundled engine"));
        assert!(!MODEL_SETUP_DETAIL.contains("/login"));
        assert!(!MODEL_UNAVAILABLE_DETAIL.contains("/login"));
    }

    #[test]
    fn unknown_and_recovery_guidance_explain_partial_effects_and_safe_support() {
        assert!(UNKNOWN_DETAIL.contains("does not prove"));
        assert!(UNKNOWN_DETAIL.contains("without resending"));
        assert!(UNKNOWN_DETAIL.contains("disabled until reconciled"));
        assert!(RECOVERY_DETAIL.contains("repeat a partially executed tool"));
        assert!(RECOVERY_DETAIL.contains("external effects"));
        assert!(RECOVERY_DETAIL.contains("metadata-only diagnostics"));
        assert!(RECOVERY_DETAIL.contains("sensitive logs"));
    }

    #[test]
    fn stopping_never_claims_settlement_and_failed_stop_explains_safe_retry() {
        let waiting = stopping_detail(None);
        assert!(waiting.contains("not confirmed"));
        assert!(waiting.contains("tools may still be running"));
        assert!(waiting.contains("without resending the prompt"));
        let failed = stopping_detail(Some("connection lost"));
        assert!(failed.contains("connection lost"));
        assert!(failed.contains("only the stop request"));
    }
}

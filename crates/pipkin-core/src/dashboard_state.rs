use super::*;
use crate::dashboard::{self as d, Action, Freshness, Request};
impl AppState {
    pub fn dashboard_attachment_ready(&self, id: ConversationId) -> bool {
        self.conversation(id).is_some_and(|c| {
            c.opened
                && c.lifecycle_ready
                && (self.selected == Some(id)
                    || self.dashboard.attachment == Some((id, c.generation)))
        })
    }
    pub fn dashboard_attach_available(&self) -> bool {
        self.mode == Mode::Real
            && self.dashboard.open
            && self.connection.is_ready()
            && self.dashboard.freshness == Freshness::Live
            && self.read_only.is_none()
            && self.dashboard.pending_send.is_none()
            && self.dashboard.control.is_none()
            && self.dashboard.selected_row().is_some_and(|r| {
                r.worker.kind == d::WorkerKind::Root
                    && r.conversation.is_some_and(|id| {
                        !self.conversation_is_closed(id)
                            && !self.owned_elsewhere(id)
                            && !self.local_cli.reconnect_unsettled(id)
                            && !self.cli_approvals.busy(id)
                            && self.conversation(id).is_some_and(|c| {
                                self.dashboard.attachment != Some((id, c.generation))
                                    || self.dashboard_attachment_ready(id)
                                    || self.dashboard.attachment_error.is_some()
                            })
                    })
            })
    }

    /// Called only by explicit request loading or a verified reconnect, never observation.
    pub(crate) fn dashboard_attach(&mut self, id: ConversationId) -> Outcome {
        let Some(c) = self.conv_mut(id) else {
            return Outcome::default();
        };
        c.generation += 1;
        c.opened = false;
        c.lifecycle_ready = false;
        c.status_check_pending = false;
        c.changes_state = ChangesState::Unscanned;
        c.intent_error = None;
        let generation = c.generation;
        self.dashboard.attachment = Some((id, generation));
        self.dashboard.attachment_error = None;
        Self::dashboard_request(Request::Attach {
            conversation: id,
            generation,
        })
    }
    fn dashboard_request(request: Request) -> Outcome {
        Outcome {
            effects: vec![Effect::Backend(BackendRequest::Dashboard(request))],
            notes: vec![Note::Other],
        }
    }
    fn dashboard_filter(&self) -> d::ActivityFilter {
        let mut filter = self.dashboard.filter.clone();
        filter.hidden_projects = self.dashboard.hidden_projects.clone();
        filter.hidden_sessions = self.dashboard.hidden_threads.keys().cloned().collect();
        filter.inactive_days = Some(self.dashboard.recency.days());
        filter.reviewed_completions = Some(self.dashboard.reviewed_completions.clone());
        filter
    }
    pub(crate) fn prune_dashboard_history(&mut self) {
        let filter = self.dashboard_filter();
        let now = self.now().saturating_mul(1000);
        let closed = &self.prefs.closed_conversations;
        let selected_was_visible = self.dashboard.selected_row().is_some();
        self.dashboard.rows.retain(|r| {
            filter.includes_visibility(&r.worker, now)
                && (r.worker.has_live_work()
                    || r.root
                        .or(r.conversation)
                        .is_none_or(|id| !closed.contains(&id.0)))
        });
        if selected_was_visible && self.dashboard.selected_row().is_none() {
            self.dashboard.selected = None;
            self.dashboard.detail.generation += 1;
            self.dashboard.detail.items.clear();
            self.dashboard.pending_send = None;
        }
    }
    fn dashboard_watch(&mut self) -> Outcome {
        self.prune_dashboard_history();
        self.dashboard.filter.seen_failures = self.dashboard.seen.clone();
        self.dashboard.generation += 1;
        self.dashboard.loading = true;
        self.dashboard.error = None;
        self.dashboard.freshness = Freshness::Updating;
        Self::dashboard_request(Request::Watch {
            generation: self.dashboard.generation,
            closed: self
                .prefs
                .closed_conversations
                .iter()
                .copied()
                .map(ConversationId)
                .collect(),
            filter: self.dashboard_filter(),
            group: self.dashboard.group,
        })
    }
    pub fn dashboard_draft(&self) -> (String, u64) {
        let Some(row) = self.dashboard.selected_row() else {
            return Default::default();
        };
        if let Some(id) = row.conversation {
            return self
                .conversation(id)
                .map(|c| (c.draft.text.clone(), c.draft.rev))
                .unwrap_or_default();
        }
        self.dashboard
            .child_drafts
            .get(&row.worker.session)
            .cloned()
            .unwrap_or_default()
    }
    pub fn dashboard_answer_ready(&self) -> bool {
        self.dashboard
            .selected_row()
            .and_then(|r| r.conversation)
            .is_some_and(|id| {
                self.dashboard.open
                    && self.connection.is_ready()
                    && self.dashboard.freshness == Freshness::Live
                    && self.read_only.is_none()
                    && self.dashboard.detail.error.is_none()
                    && !self.conversation_is_closed(id)
                    && !self.owned_elsewhere(id)
                    && self.dashboard_attachment_ready(id)
                    && self.conversation(id).is_some_and(|c| {
                        c.opened && c.lifecycle_ready && c.execution_blocked.is_none()
                    })
            })
    }

    /// Shared by dashboard controls and dispatch; observation alone never attaches a session.
    pub fn dashboard_action_unavailable(&self, action: Action) -> Option<&'static str> {
        use d::{WorkState as W, WorkerKind as K};
        let Some(row) = self.dashboard.selected_row() else {
            return Some("Select a thread first.");
        };
        if !self.dashboard.open
            || !self.connection.is_ready()
            || self.dashboard.freshness != Freshness::Live
        {
            return Some("Wait for a live connection before taking action.");
        }
        if self.read_only.is_some() {
            return Some("This workspace is read-only.");
        }
        if self.dashboard.detail.error.is_some() {
            return Some("Refresh this thread before taking action.");
        }
        if self.dashboard.control.is_some() || self.dashboard.pending_send.is_some() {
            return Some("Wait for the pending action, or check its status.");
        }
        if action != Action::Stop
            && row
                .conversation
                .is_some_and(|id| self.local_cli.reconnect_unsettled(id))
        {
            return Some("Finish the pending Claude connection check before sending a prompt.");
        }
        if row
            .root
            .is_some_and(|id| self.conversation_is_closed(id) || self.owned_elsewhere(id))
        {
            return Some("Open this thread in its owning window before taking action.");
        }
        if action != Action::Stop
            && matches!(
                row.worker.state,
                W::Waiting {
                    reason: d::WaitReason::Question | d::WaitReason::CliApproval
                }
            )
        {
            return Some("Review the pending question or permission request first.");
        }
        if action != Action::Stop && self.dashboard_draft().0.trim().is_empty() {
            return Some("Write a message first.");
        }
        let active = matches!(
            row.worker.state,
            W::Running | W::Pending | W::Queued | W::Waiting { .. }
        );
        match action {
            Action::Send
                if row.worker.kind != K::Root
                    || row.conversation.is_none()
                    || !matches!(
                        row.worker.state,
                        W::Ready | W::Stopped | W::Failed { .. } | W::Interrupted
                    ) =>
            {
                Some("Send is available when a conversation is ready for its next prompt.")
            }
            Action::Queue | Action::Steer if !active => {
                Some("Queue and Steer require an active thread.")
            }
            Action::Queue | Action::Steer
                if row.worker.execution_backend == d::ExecutionBackend::LocalClaudeCli
                    || row.worker.execution_backend == d::ExecutionBackend::Unsupported =>
            {
                Some(
                    "This Claude CLI mode does not support Queue or Steer. Send another prompt after the current turn finishes.",
                )
            }
            Action::Queue if row.worker.kind != K::Root => {
                Some("Child tasks support Steer instead of Queue.")
            }
            Action::Stop
                if !active
                    || (row.worker.run.as_ref().is_none_or(|r| r.phase.is_none())
                        && row.worker.child.is_none()) =>
            {
                Some("This thread has no active run to stop.")
            }
            Action::FollowUp if !row.finished_child() => {
                Some("Follow up is available for finished child tasks.")
            }
            _ => None,
        }
    }
    pub fn dispatch_dashboard(&mut self, command: d::Command) -> Outcome {
        let mut out = Outcome {
            effects: vec![],
            notes: vec![Note::Other],
        };
        match command {
            d::Command::CreateNative { project, model } => {
                out.merge(self.create_dashboard_native(project, model));
            }
            d::Command::DismissCreation => {
                if !matches!(self.dashboard.creation, Some(d::Creation::Pending(_))) {
                    self.dashboard.creation = None;
                }
            }
            d::Command::Attach {
                session,
                generation,
                revision,
            } => {
                if self.dashboard_attach_available()
                    && generation == self.dashboard.detail.generation
                    && let Some(id) = self
                        .dashboard
                        .selected_row()
                        .filter(|r| r.worker.session == session && r.worker.revision == revision)
                        .and_then(|r| r.conversation)
                {
                    out.merge(self.dashboard_attach(id));
                }
            }
            d::Command::Open => {
                self.dashboard.open = true;
                out.merge(self.dashboard_watch());
            }
            d::Command::Close => {
                self.dashboard.open = false;
                self.dashboard.generation += 1;
                self.dashboard.pending_send = None;
                out.merge(Self::dashboard_request(Request::Close {
                    generation: self.dashboard.generation,
                }));
            }
            d::Command::Refresh => out.merge(self.dashboard_watch()),
            d::Command::Group(group) => {
                self.dashboard.group = group;
                self.dashboard
                    .rows
                    .retain(|r| Some(&r.worker.session) == self.dashboard.selected.as_ref());
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
            }
            d::Command::HideProject(path) => {
                if self.dashboard.rows.iter().any(|r| r.worker.cwd == path) {
                    self.dashboard.hidden_projects.insert(path);
                    self.dashboard.next = None;
                    out.merge(self.dashboard_watch());
                }
            }
            d::Command::HideThread(session) => {
                if let Some(row) = self
                    .dashboard
                    .rows
                    .iter()
                    .find(|r| r.worker.session == session)
                {
                    let hidden = d::HiddenThread {
                        title: row.title().to_owned(),
                        project: row.worker.cwd.clone(),
                    };
                    self.dashboard.hidden_threads.insert(session, hidden);
                    self.dashboard.next = None;
                    out.merge(self.dashboard_watch());
                }
            }
            d::Command::ShowProject(path) => {
                self.dashboard.hidden_projects.remove(&path);
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
            }
            d::Command::ShowThread(session) => {
                self.dashboard.hidden_threads.remove(&session);
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
            }
            d::Command::ShowAllHidden => {
                self.dashboard.hidden_projects.clear();
                self.dashboard.hidden_threads.clear();
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
            }
            d::Command::Recency(recency) => {
                self.dashboard.recency = recency;
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
            }
            d::Command::Filter(filter) => {
                self.dashboard.filter = filter;
                self.dashboard
                    .rows
                    .retain(|r| Some(&r.worker.session) == self.dashboard.selected.as_ref());
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
            }
            d::Command::MarkReviewed {
                session,
                generation,
                completion,
            } => {
                if self.dashboard.open
                    && generation == self.dashboard.detail.generation
                    && self.dashboard.selected_row().is_some_and(|row| {
                        row.worker.session == session
                            && row.worker.completion_key().as_ref() == Some(&completion)
                    })
                {
                    self.dashboard
                        .reviewed_completions
                        .insert(session, completion);
                    out.merge(self.dashboard_watch());
                }
            }
            d::Command::Select(session) => {
                if self
                    .dashboard
                    .rows
                    .iter()
                    .any(|r| r.worker.session == session)
                {
                    self.dashboard.selected = Some(session.clone());
                    self.dashboard.pending_send = None;
                    self.dashboard.detail = d::Detail {
                        session: session.clone(),
                        generation: self.dashboard.detail.generation + 1,
                        loading: true,
                        ..Default::default()
                    };
                    out.merge(Self::dashboard_request(Request::Detail {
                        generation: self.dashboard.detail.generation,
                        session,
                        before: None,
                    }));
                }
            }
            d::Command::ToolOutput(call_id) => {
                if let Some(session) = self.dashboard.selected.clone() {
                    self.dashboard.detail.output = None;
                    out.merge(Self::dashboard_request(Request::ToolOutput {
                        generation: self.dashboard.detail.generation,
                        session,
                        call_id,
                    }));
                }
            }
            d::Command::Back => {
                self.dashboard.selected = None;
                self.dashboard.detail.generation += 1;
                self.dashboard.pending_send = None;
            }
            d::Command::More => {
                if !self.dashboard.loading
                    && let Some(after) = self.dashboard.next.clone()
                {
                    self.dashboard.loading = true;
                    out.merge(Self::dashboard_request(Request::More {
                        generation: self.dashboard.generation,
                        filter: self.dashboard_filter(),
                        group: self.dashboard.group.unwrap_or(d::ActivityGroup::Overview),
                        after,
                    }));
                }
            }
            d::Command::Older => {
                if !self.dashboard.detail.loading
                    && self.dashboard.detail.has_older
                    && let Some(session) = self.dashboard.selected.clone()
                {
                    self.dashboard.detail.loading = true;
                    out.merge(Self::dashboard_request(Request::Detail {
                        generation: self.dashboard.detail.generation,
                        session,
                        before: self.dashboard.detail.oldest,
                    }));
                }
            }
            d::Command::Edit(text) => {
                if let Some(row) = self.dashboard.selected_row().cloned() {
                    if let Some(id) = row.conversation {
                        if let Some(c) = self.conv_mut(id)
                            && c.draft.text != text
                        {
                            c.draft.text = text;
                            c.draft.rev += 1;
                            c.draft.sync_epoch += 1;
                            c.draft.save = SaveState::Dirty;
                        }
                        out.merge(self.flush_draft(id));
                    } else {
                        let draft = self
                            .dashboard
                            .child_drafts
                            .entry(row.worker.session)
                            .or_default();
                        if draft.0 != text {
                            draft.0 = text;
                            draft.1 += 1;
                        }
                    }
                }
            }
            d::Command::Act {
                session,
                generation,
                revision,
                draft_revision,
                action,
            } => {
                let Some(row) = self
                    .dashboard
                    .selected_row()
                    .filter(|r| r.worker.session == session && r.worker.revision == revision)
                    .cloned()
                else {
                    return out;
                };
                if self.dashboard_action_unavailable(action).is_some() {
                    return out;
                }
                if generation != self.dashboard.detail.generation
                    || self.read_only.is_some()
                    || !self.connection.is_ready()
                    || self.dashboard.freshness != Freshness::Live
                    || self.dashboard.detail.error.is_some()
                    || self.dashboard.control.is_some()
                {
                    return out;
                }
                let (text, rev) = self.dashboard_draft();
                if rev != draft_revision {
                    return out;
                }
                if row
                    .root
                    .is_some_and(|id| self.conversation_is_closed(id) || self.owned_elsewhere(id))
                {
                    return out;
                }
                if matches!(action, Action::Send | Action::Queue | Action::Steer)
                    && let Some(id) = row.conversation
                {
                    if text.trim().is_empty() {
                        return out;
                    }
                    self.dashboard.pending_send = Some((id, rev, action));
                    if !self.dashboard_attachment_ready(id) {
                        out.merge(self.dashboard_attach(id));
                    } else {
                        out.merge(self.dashboard_continue_send());
                    }
                } else {
                    let run = row
                        .worker
                        .run
                        .as_ref()
                        .filter(|r| r.phase.is_some())
                        .map(|r| r.id);
                    if action == Action::Stop && run.is_none() && row.worker.child.is_none() {
                        return out;
                    }
                    if action == Action::FollowUp && !row.finished_child() {
                        return out;
                    }
                    if matches!(action, Action::Steer | Action::FollowUp) && text.trim().is_empty()
                    {
                        return out;
                    }
                    if !matches!(action, Action::Stop | Action::Steer | Action::FollowUp) {
                        return out;
                    }
                    let request = self.alloc_request().0;
                    let pending = d::PendingControl {
                        generation,
                        session: session.clone(),
                        root: row.worker.root.clone(),
                        child: row.worker.child,
                        run,
                        request: request.clone(),
                        text: text.clone(),
                        action,
                        uncertain: false,
                    };
                    self.dashboard.control = Some(pending);
                    self.prefs.dashboard = self.dashboard.saved();
                    out.effects.push(Effect::SaveDashboard {
                        prefs: self.prefs.clone(),
                        request,
                    });
                }
            }
            d::Command::Answer {
                session,
                generation,
                revision,
                request,
                answer,
            } => {
                let target = self
                    .dashboard
                    .selected_row()
                    .filter(|r| {
                        r.worker.session == session
                            && r.worker.revision == revision
                            && r.worker.kind == d::WorkerKind::Root
                    })
                    .and_then(|r| r.conversation);
                let Some(id) = target else {
                    return out;
                };
                let allowed = self.conversation(id).is_some_and(|c| {
                    c.execution_profile(self.mode)
                        .allows_command(&Command::AnswerUiRequest {
                            id: request.clone(),
                            answer: answer.clone(),
                        })
                });
                if self.refuse_if_profile_disallows(id, allowed, &mut out) {
                    return out;
                }
                if !self.dashboard_answer_ready()
                    || generation != self.dashboard.detail.generation
                    || self.read_only.is_some()
                    || !self.connection.is_ready()
                    || self.dashboard.freshness != Freshness::Live
                    || self.conversation_is_closed(id)
                    || self.owned_elsewhere(id)
                    || self.refuse_if_execution_blocked(id, &mut out)
                {
                    return out;
                }
                let Some(c) = self.conv_mut(id) else {
                    return out;
                };
                let Some(q) = c.ui_requests.iter().find(|q| q.id == request) else {
                    return out;
                };
                let valid = match (&q.kind, &answer) {
                    (UiRequestKind::Select, UiAnswer::Choice(value)) => {
                        q.items.iter().any(|i| &i.value == value)
                    }
                    (UiRequestKind::Confirm, UiAnswer::Confirm(_)) => true,
                    (UiRequestKind::Input, UiAnswer::Text(value)) => !value.trim().is_empty(),
                    _ => false, // Complex questionnaires retain their existing dedicated UI.
                };
                if !valid || !c.opened || !c.lifecycle_ready || c.ui_answering.contains(&request) {
                    return out;
                }
                c.ui_answering.push(request.clone());
                c.ui_error = None;
                out.effects.push(Effect::Backend(BackendRequest::UiRespond {
                    conversation: id,
                    generation: c.generation,
                    id: request,
                    answer,
                }));
            }
            d::Command::Reconcile => {
                if let Some(p) = self.dashboard.control.clone().filter(|p| p.uncertain) {
                    out.merge(Self::dashboard_request(Request::Control {
                        generation: p.generation,
                        session: p.session,
                        root: p.root,
                        child: p.child,
                        run: p.run,
                        request: p.request,
                        text: p.text,
                        action: p.action,
                        lookup: true,
                    }));
                }
            }
        }
        self.prefs.dashboard = self.dashboard.saved();
        if !out
            .effects
            .iter()
            .any(|e| matches!(e, Effect::SaveDashboard { .. }))
        {
            out.effects.push(Effect::SavePrefs(self.prefs.clone()));
        }
        out
    }
    pub(super) fn dashboard_continue_send(&mut self) -> Outcome {
        let mut out = Outcome::default();
        let Some((id, rev, action)) = self.dashboard.pending_send else {
            return out;
        };
        let Some(c) = self.conversation(id) else {
            self.dashboard.pending_send = None;
            return out;
        };
        if c.draft.rev != rev
            || !self.dashboard.open
            || self.dashboard.selected_row().and_then(|r| r.conversation) != Some(id)
        {
            self.dashboard.pending_send = None;
            return out;
        }
        if !c.opened || !c.lifecycle_ready {
            return out;
        }
        let available = self.availability_for(Some(id));
        let allowed = match action {
            Action::Send => available.submit,
            Action::Queue => available.queue,
            Action::Steer => available.steer,
            _ => false,
        };
        self.dashboard.pending_send = None;
        if allowed {
            let c = self.conversation(id).unwrap();
            let text = c.draft.text.trim_end().to_owned();
            let atts = c.draft.attachments.clone();
            out.merge(self.submit(
                id,
                text,
                atts,
                match action {
                    Action::Queue => IntentOrigin::FollowUp,
                    Action::Steer => IntentOrigin::Steer,
                    _ => IntentOrigin::Draft,
                },
            ));
        } else {
            self.dashboard.error = Some(
                "This target is not ready for that action. Check its connection and current state."
                    .into(),
            );
            out.notes.push(Note::Other);
        }
        out
    }
    pub fn dashboard_event(&mut self, event: d::Event) -> Outcome {
        let mut out = Outcome {
            effects: vec![],
            notes: vec![Note::Other],
        };
        match event {
            d::Event::Counts(counts) => self.dashboard.global_counts = Some(counts),
            d::Event::ToolOutput {
                generation,
                session,
                call_id,
                result,
            } => {
                if generation == self.dashboard.detail.generation
                    && self.dashboard.selected.as_deref() == Some(&session)
                {
                    self.dashboard.detail.output = Some((call_id, result));
                }
            }
            d::Event::Persisted { request, result } => {
                if let Some(p) = self
                    .dashboard
                    .control
                    .clone()
                    .filter(|p| p.request == request && !p.uncertain)
                {
                    match result {
                        Ok(()) => out.merge(Self::dashboard_request(Request::Control {
                            generation: p.generation,
                            session: p.session,
                            root: p.root,
                            child: p.child,
                            run: p.run,
                            request: p.request,
                            text: p.text,
                            action: p.action,
                            lookup: false,
                        })),
                        Err(e) => {
                            self.dashboard.control = None;
                            self.dashboard.error = Some(format!(
                                "The action was not sent because its receipt could not be saved: {e}"
                            ));
                        }
                    }
                }
            }
            d::Event::Available(yes) => {
                self.dashboard.available = yes;
                if !yes {
                    self.dashboard.freshness = Freshness::Unavailable;
                } else if self.dashboard.open {
                    out.merge(self.dashboard_watch());
                }
            }
            d::Event::Directory {
                generation,
                rows,
                page,
                append,
            } => {
                if !self.dashboard.open
                    || generation != self.dashboard.generation
                    || page.through < self.dashboard.through
                {
                    return Outcome::default();
                }
                self.dashboard.loading = false;
                self.dashboard.error = None;
                self.dashboard.freshness = Freshness::Live;
                self.dashboard.through = page.through;
                self.dashboard.counts = Some(page.counts);
                self.dashboard.coverage = Some(page.coverage);
                self.dashboard.next = page.next;
                if !append {
                    // Keep a selected terminal item visible even after it leaves the active page.
                    let selected = self.dashboard.selected_row().cloned();
                    let previous = &self.dashboard.rows;
                    let rows = rows
                        .into_iter()
                        .map(|r| {
                            previous
                                .iter()
                                .find(|old| {
                                    old.worker.session == r.worker.session
                                        && old.worker.revision > r.worker.revision
                                })
                                .cloned()
                                .unwrap_or(r)
                        })
                        .collect();
                    self.dashboard.rows = rows;
                    if let Some(selected) = selected
                        && !self
                            .dashboard
                            .rows
                            .iter()
                            .any(|r| r.worker.session == selected.worker.session)
                    {
                        self.dashboard.rows.push(selected);
                    }
                } else {
                    for row in rows {
                        if let Some(old) = self
                            .dashboard
                            .rows
                            .iter_mut()
                            .find(|r| r.worker.session == row.worker.session)
                        {
                            if row.worker.revision >= old.worker.revision {
                                *old = row;
                            }
                        } else {
                            self.dashboard.rows.push(row);
                        }
                    }
                }
                self.prune_dashboard_history();
                if self.dashboard.selected_row().is_some()
                    && self.dashboard.selected.as_deref() != Some(&self.dashboard.detail.session)
                {
                    out.merge(self.dispatch_dashboard(d::Command::Select(
                        self.dashboard.selected.clone().unwrap(),
                    )));
                }
                if self.dashboard.rows.len() > d::MAX_ROWS {
                    let excess = self.dashboard.rows.len() - d::MAX_ROWS;
                    self.dashboard.rows.drain(0..excess);
                }
            }
            d::Event::Detail {
                generation,
                session,
                before,
                items,
                oldest,
                has_older,
            } => {
                if generation != self.dashboard.detail.generation
                    || self.dashboard.selected.as_deref() != Some(&session)
                {
                    return Outcome::default();
                }
                let detail = &mut self.dashboard.detail;
                detail.loading = false;
                detail.error = None;
                if before.is_some() {
                    detail.items.splice(0..0, items);
                    detail.oldest = oldest;
                    detail.has_older = has_older;
                } else {
                    let first = items.first().map(|i| i.id);
                    if let Some(first) = first {
                        detail.items.retain(|i| i.id < first);
                    }
                    if detail.items.is_empty() {
                        detail.oldest = oldest;
                        detail.has_older = has_older;
                    }
                    detail.items.extend(items);
                }
                if detail.items.len() > d::MAX_HISTORY {
                    if before.is_some() {
                        detail.items.truncate(d::MAX_HISTORY);
                    } else {
                        let excess = detail.items.len() - d::MAX_HISTORY;
                        detail.items.drain(0..excess);
                        detail.oldest = detail.items.first().map(|i| i.id.0 / 1024);
                        detail.has_older = true;
                    }
                }
                if let Some(row) = self.dashboard.selected_row()
                    && matches!(
                        row.worker.state,
                        d::WorkState::Failed { .. } | d::WorkState::Interrupted
                    )
                {
                    let revision = row.worker.revision;
                    if self.dashboard.seen.get(&session) != Some(&revision) {
                        self.dashboard.seen.insert(session, revision);
                        out.merge(self.dashboard_watch());
                    }
                }
            }
            d::Event::Invalidated { session, revision } => {
                if self
                    .dashboard
                    .rows
                    .iter()
                    .any(|r| r.worker.session == session && r.worker.revision < revision)
                {
                    self.dashboard.freshness = Freshness::Updating;
                }
            }
            d::Event::Failed {
                generation,
                session,
                message,
            } => {
                if session.as_ref() == self.dashboard.selected.as_ref()
                    && generation == self.dashboard.detail.generation
                {
                    self.dashboard.detail.loading = false;
                    self.dashboard.detail.error = Some(message);
                } else if generation == self.dashboard.generation {
                    self.dashboard.loading = false;
                    self.dashboard.freshness = Freshness::Reconnecting;
                    self.dashboard.error = Some(message);
                }
            }
            d::Event::Control {
                generation,
                request,
                result,
                uncertain,
            } => {
                let Some(p) = self
                    .dashboard
                    .control
                    .as_ref()
                    .filter(|p| p.generation == generation && p.request == request)
                    .cloned()
                else {
                    return Outcome::default();
                };
                if uncertain {
                    self.dashboard.control.as_mut().unwrap().uncertain = true;
                    self.dashboard.error = Some(
                        "The action's outcome is unknown. Check status; do not resend it.".into(),
                    );
                } else {
                    self.dashboard.control = None;
                    match result {
                        Ok(_) => {
                            if let Some(draft) = self.dashboard.child_drafts.get_mut(&p.session)
                                && draft.0 == p.text
                            {
                                draft.0.clear();
                                draft.1 += 1;
                            }
                            self.dashboard.error = None;
                            if self.dashboard.open {
                                out.merge(self.dashboard_watch());
                            }
                        }
                        Err(error) => self.dashboard.error = Some(error),
                    }
                }
            }
        }
        let saved = self.dashboard.saved();
        if self.prefs.dashboard != saved {
            self.prefs.dashboard = saved;
            out.effects.push(Effect::SavePrefs(self.prefs.clone()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> AppState {
        let mut s = AppState::new(
            Bootstrap {
                projects: vec![],
                models: vec![],
                conversations: vec![
                    (ConversationId(1), ProjectId(1), "normal".into(), 0),
                    (ConversationId(2), ProjectId(1), "target".into(), 0),
                ],
                now: 0,
            },
            Prefs::default(),
        );
        let worker: d::ActivityRow=serde_json::from_value(serde_json::json!({"session":"target","revision":3,"kind":"root","root":"target","cwd":"/p","state":{"state":"ready"},"createdAtMs":0,"updatedAtMs":0,"sortAtMs":0})).unwrap();
        s.dashboard.rows.push(d::Row {
            worker,
            conversation: Some(ConversationId(2)),
            root: Some(ConversationId(2)),
        });
        s.dashboard.open = true;
        s.dashboard.available = true;
        s.dashboard.selected = Some("target".into());
        s.dashboard.detail.session = "target".into();
        s.dashboard.detail.generation = 7;
        s.dashboard.freshness = Freshness::Live;
        s
    }
    #[test]
    fn completed_results_require_explicit_review_and_persist_by_turn() {
        let mut s = state();
        s.dashboard.rows[0].worker.run = Some(pipkin_engine_protocol::RunBrief {
            id: 1,
            phase: None,
            outcome: Some(pipkin_engine_protocol::RunOutcome::Completed),
            started_at_ms: 100,
            ended_at_ms: Some(200),
        });
        let row = s.dashboard.rows[0].clone();
        assert!(s.dashboard.completion_unread(&row));
        // Automatic history refresh (even when selected) must not acknowledge a result.
        s.dashboard_event(d::Event::Detail {
            generation: 7,
            session: "target".into(),
            before: None,
            items: vec![],
            oldest: None,
            has_older: false,
        });
        assert!(s.dashboard.completion_unread(&row));
        let key = row.worker.completion_key().unwrap();
        s.dispatch_dashboard(d::Command::MarkReviewed {
            session: "target".into(),
            generation: 6,
            completion: key.clone(),
        });
        assert!(s.dashboard.completion_unread(&row));
        let out = s.dispatch_dashboard(d::Command::MarkReviewed {
            session: "target".into(),
            generation: 7,
            completion: key.clone(),
        });
        assert!(!s.dashboard.completion_unread(&row));
        assert!(
            out.effects
                .iter()
                .any(|e| matches!(e, Effect::SavePrefs(_)))
        );
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Dashboard(Request::Watch { filter, .. })) if filter.reviewed_completions.as_ref().is_some_and(|m| m.get("target") == Some(&key)))));
        let restored = d::State::restore(&s.prefs.dashboard);
        assert!(!restored.completion_unread(&row));
        s.dashboard.rows[0].worker.run.as_mut().unwrap().id = 2;
        // A late click for the previous turn cannot clear the new result.
        s.dispatch_dashboard(d::Command::MarkReviewed {
            session: "target".into(),
            generation: 7,
            completion: key,
        });
        assert!(s.dashboard.completion_unread(&s.dashboard.rows[0]));
        s.dispatch_dashboard(d::Command::Filter(Default::default()));
        assert_eq!(
            s.dashboard.reviewed_completions,
            restored.reviewed_completions
        );
    }

    #[test]
    fn manual_hiding_persists_and_preserves_work_drafts_and_individual_choices() {
        let mut s = state();
        s.conversations[1].draft.text = "keep draft".into();
        s.dashboard.rows[0].worker.state = d::WorkState::Running;
        let original = s.dashboard.rows[0].clone();
        let mut child = original.clone();
        child.worker.session = "child".into();
        child.worker.kind = d::WorkerKind::Child;
        s.dashboard.rows.push(child);
        let out = s.dispatch_dashboard(d::Command::HideThread("target".into()));
        assert!(
            s.dashboard.rows.is_empty(),
            "explicit hiding also hides active descendants"
        );
        assert!(s.dashboard.selected.is_none());
        assert_eq!(s.dashboard.detail.generation, 8);
        assert_eq!(s.conversations[1].draft.text, "keep draft");
        assert!(s.prefs.closed_conversations.is_empty());
        assert!(
            out.effects
                .iter()
                .any(|e| matches!(e, Effect::SavePrefs(_)))
        );
        assert!(out.effects.iter().all(|e| !matches!(e, Effect::Backend(request) if !matches!(request, BackendRequest::Dashboard(Request::Watch { .. })))));
        s.dashboard.rows.push(original.clone());
        s.dispatch_dashboard(d::Command::HideProject("/p".into()));
        s.dispatch_dashboard(d::Command::Filter(Default::default()));
        let restored = d::State::restore(&s.prefs.dashboard);
        assert!(restored.hidden_projects.contains("/p"));
        assert_eq!(restored.hidden_threads["target"].title, original.title());
        s.dashboard = restored;
        s.dispatch_dashboard(d::Command::ShowProject("/p".into()));
        assert!(
            !s.dashboard_filter()
                .includes_visibility(&original.worker, 0),
            "showing a project retains individually hidden threads"
        );
        // A selected row returning in a later page cannot bypass the policy.
        s.dashboard.rows.push(original.clone());
        s.dashboard.selected = Some("target".into());
        s.prune_dashboard_history();
        assert!(s.dashboard.rows.is_empty());
        s.dispatch_dashboard(d::Command::ShowThread("target".into()));
        assert!(
            s.dashboard_filter()
                .includes_visibility(&original.worker, 0)
        );
        s.dashboard.hidden_projects.insert("/p".into());
        s.dashboard.hidden_threads.insert(
            "target".into(),
            d::HiddenThread {
                title: "target".into(),
                project: "/p".into(),
            },
        );
        s.dispatch_dashboard(d::Command::ShowAllHidden);
        let restored = d::State::restore(&s.prefs.dashboard);
        assert!(restored.hidden_projects.is_empty() && restored.hidden_threads.is_empty());
    }

    #[test]
    fn recency_prunes_selected_and_closed_history_without_deleting_conversations() {
        let mut s = state();
        s.conversations[1].draft.text = "keep draft".into();
        s.set_now(31 * 86_400);
        assert!(s.dashboard.rows.is_empty());
        assert!(s.dashboard.selected.is_none());
        assert_eq!(
            s.conversation(ConversationId(2)).unwrap().draft.text,
            "keep draft"
        );
        assert_eq!(s.dashboard.detail.generation, 8);
        let mut s = state();
        let mut child = s.dashboard.rows[0].clone();
        child.worker.session = "child".into();
        child.worker.kind = d::WorkerKind::Child;
        child.conversation = None;
        s.dashboard.rows.push(child);
        s.prefs.closed_conversations.insert(2);
        let out = s.dispatch_dashboard(d::Command::Refresh);
        assert!(s.dashboard.rows.is_empty());
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Dashboard(Request::Watch { closed, filter, .. })) if closed == &vec![ConversationId(2)] && filter.inactive_days == Some(30))));
        assert_eq!(s.conversations.len(), 2);
    }

    #[test]
    fn recency_setting_survives_search_reset_and_restart_and_protects_active_rows() {
        let mut s = state();
        s.dashboard.rows[0].worker.state = d::WorkState::Waiting {
            reason: d::WaitReason::CliApproval,
        };
        s.set_now(100 * 86_400);
        assert_eq!(s.dashboard.rows.len(), 1);
        s.dispatch_dashboard(d::Command::Recency(d::Recency::TwoMonths));
        let out = s.dispatch_dashboard(d::Command::Filter(Default::default()));
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::Dashboard(Request::Watch { filter, .. })) if filter.inactive_days == Some(60))));
        assert_eq!(d::State::restore(&s.dashboard.saved()).recency.days(), 60);
        assert_eq!(d::State::restore(r#"{"filter":{},"group":null,"selected":null,"child_drafts":{},"seen":{},"control":null}"#).recency.days(), 30);
    }

    fn action(s: &mut AppState, action: Action) -> Outcome {
        s.dispatch_dashboard(d::Command::Act {
            session: "target".into(),
            generation: 7,
            revision: 3,
            draft_revision: s.dashboard_draft().1,
            action,
        })
    }
    #[test]
    fn dashboard_explicit_attach_is_scoped_and_stale_failures_are_ignored() {
        let mut s = state();
        s.mode = Mode::Real;
        s.conversations[0].draft.text = "normal draft".into();
        let attach = |generation, revision| d::Command::Attach {
            session: "target".into(),
            generation,
            revision,
        };
        assert!(
            !s.dispatch_dashboard(attach(6, 3))
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(_)))
        );
        assert!(
            !s.dispatch_dashboard(attach(7, 2))
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(_)))
        );
        s.read_only = Some("readonly".into());
        assert!(
            !s.dispatch_dashboard(attach(7, 3))
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(_)))
        );
        s.read_only = None;
        let out = s.dispatch_dashboard(attach(7, 3));
        let generation = s.conversations[1].generation;
        assert!(out.effects.iter().any(|e| matches!(
            e,
            Effect::Backend(BackendRequest::Dashboard(Request::Attach {
                conversation: ConversationId(2),
                ..
            }))
        )));
        assert_eq!(s.selected, Some(ConversationId(1)));
        assert_eq!(s.conversations[0].draft.text, "normal draft");
        assert!(!s.dashboard_attachment_ready(ConversationId(2)));
        for (g, error) in [(generation - 1, "stale"), (generation, "connect failed")] {
            s.apply_event(BackendEvent {
                conversation: ConversationId(2),
                generation: g,
                op: None,
                kind: EventKind::OpenFailed {
                    message: error.into(),
                },
            });
        }
        assert_eq!(
            s.dashboard.attachment_error.as_deref(),
            Some("connect failed")
        );
        s.dispatch_dashboard(attach(7, 3));
        assert!(s.dashboard.attachment_error.is_none());
    }

    #[test]
    fn controls_follow_live_state_backend_and_write_guards() {
        let mut s = state();
        assert!(s.dashboard_action_unavailable(Action::Send).is_some());
        s.dispatch_dashboard(d::Command::Edit("next prompt".into()));
        assert!(s.dashboard_action_unavailable(Action::Send).is_none());
        assert!(s.dashboard_action_unavailable(Action::Queue).is_some());
        s.dashboard.rows[0].worker.state = d::WorkState::Running;
        assert!(s.dashboard_action_unavailable(Action::Send).is_some());
        assert!(s.dashboard_action_unavailable(Action::Steer).is_none());
        s.dashboard.rows[0].worker.execution_backend = d::ExecutionBackend::LocalClaudeCli;
        for a in [Action::Queue, Action::Steer] {
            assert!(s.dashboard_action_unavailable(a).is_some());
            assert!(action(&mut s, a).effects.is_empty());
        }
        s.dashboard.rows[0].worker.state = d::WorkState::Ready;
        assert!(s.dashboard_action_unavailable(Action::Send).is_none());
        s.dashboard.freshness = Freshness::Reconnecting;
        assert!(s.dashboard_action_unavailable(Action::Send).is_some());
        assert!(action(&mut s, Action::Send).effects.is_empty());
    }

    #[test]
    fn observation_and_editing_keep_normal_selection_and_its_draft() {
        let mut s = state();
        s.conversations[0].draft.text = "normal draft".into();
        let out = s.dispatch_dashboard(d::Command::Select("target".into()));
        assert!(out.effects.iter().any(|e|matches!(e,Effect::Backend(BackendRequest::Dashboard(Request::Detail{session,..})) if session=="target")));
        assert!(
            !out.effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(BackendRequest::Open { .. })))
        );
        s.dispatch_dashboard(d::Command::Edit("target draft".into()));
        assert_eq!(s.selected, Some(ConversationId(1)));
        assert_eq!(s.conversations[0].draft.text, "normal draft");
        assert_eq!(s.conversations[1].draft.text, "target draft");
    }
    #[test]
    fn stale_observer_and_changed_target_cannot_send() {
        let mut s = state();
        s.dispatch_dashboard(d::Command::Edit("hello".into()));
        s.dashboard.detail.generation = 8;
        assert!(action(&mut s, Action::Send).effects.is_empty());
        s.dashboard.detail.generation = 7;
        s.dashboard.freshness = Freshness::Reconnecting;
        assert!(action(&mut s, Action::Send).effects.is_empty());
    }
    #[test]
    fn root_send_attaches_only_explicit_target_and_selection_change_cancels_pending() {
        let mut s = state();
        s.dispatch_dashboard(d::Command::Edit("hello".into()));
        let out = action(&mut s, Action::Send);
        assert!(out.effects.iter().any(|e| matches!(
            e,
            Effect::Backend(BackendRequest::Dashboard(Request::Attach {
                conversation: ConversationId(2),
                ..
            }))
        )));
        assert_eq!(s.selected, Some(ConversationId(1)));
        s.dispatch_dashboard(d::Command::Back);
        assert!(s.dashboard.pending_send.is_none());
    }
    #[test]
    fn child_follow_up_is_journaled_before_send_and_survives_restart_as_lookup_only() {
        let mut s = state();
        let row = &mut s.dashboard.rows[0];
        row.worker.kind = d::WorkerKind::Child;
        row.worker.child = Some(4);
        row.worker.state = d::WorkState::Completed;
        row.conversation = None;
        s.dispatch_dashboard(d::Command::Edit("continue safely".into()));
        let out = action(&mut s, Action::FollowUp);
        assert!(
            out.effects
                .iter()
                .any(|e| matches!(e, Effect::SaveDashboard { .. }))
        );
        assert!(!out.effects.iter().any(|e| matches!(e, Effect::Backend(_))));
        let p = s.dashboard.control.clone().unwrap();
        let recovered = d::State::restore(&s.prefs.dashboard);
        assert!(recovered.control.unwrap().uncertain);
        let out = s.dashboard_event(d::Event::Persisted {
            request: p.request.clone(),
            result: Ok(()),
        });
        assert!(out.effects.iter().any(|e| matches!(
            e,
            Effect::Backend(BackendRequest::Dashboard(Request::Control {
                lookup: false,
                action: Action::FollowUp,
                ..
            }))
        )));
        s.dashboard_event(d::Event::Control {
            generation: 7,
            request: p.request,
            result: Err("lost".into()),
            uncertain: true,
        });
        let out = s.dispatch_dashboard(d::Command::Reconcile);
        assert!(out.effects.iter().any(|e| matches!(
            e,
            Effect::Backend(BackendRequest::Dashboard(Request::Control {
                lookup: true,
                ..
            }))
        )));
        assert_eq!(s.conversations[0].draft.text, "");
    }
    #[test]
    fn failed_receipt_write_sends_nothing_and_keeps_draft() {
        let mut s = state();
        let row = &mut s.dashboard.rows[0];
        row.worker.kind = d::WorkerKind::Child;
        row.worker.child = Some(4);
        row.worker.state = d::WorkState::Completed;
        row.conversation = None;
        s.dispatch_dashboard(d::Command::Edit("next".into()));
        action(&mut s, Action::FollowUp);
        let request = s.dashboard.control.as_ref().unwrap().request.clone();
        let out = s.dashboard_event(d::Event::Persisted {
            request,
            result: Err("disk full".into()),
        });
        assert!(!out.effects.iter().any(|e| matches!(e, Effect::Backend(_))));
        assert_eq!(s.dashboard_draft().0, "next");
        assert!(s.dashboard.control.is_none());
    }
    #[test]
    fn stale_history_does_not_replace_new_target() {
        let mut s = state();
        s.dashboard_event(d::Event::Detail {
            generation: 6,
            session: "target".into(),
            before: None,
            items: vec![],
            oldest: Some(42),
            has_older: true,
        });
        assert_eq!(s.dashboard.detail.oldest, None);
        s.dashboard_event(d::Event::Detail {
            generation: 7,
            session: "other".into(),
            before: None,
            items: vec![],
            oldest: Some(42),
            has_older: true,
        });
        assert_eq!(s.dashboard.detail.oldest, None);
    }
    #[test]
    fn scoped_answer_keeps_selection_and_fences_stale_or_duplicate_answers() {
        let mut s = state();
        let c = &mut s.conversations[1];
        c.opened = true;
        c.lifecycle_ready = true;
        s.dashboard.attachment = Some((ConversationId(2), c.generation));
        c.ui_requests.push(UiRequest {
            id: "question".into(),
            kind: UiRequestKind::Select,
            title: "Retry policy".into(),
            message: None,
            items: vec![UiRequestItem {
                value: "backoff".into(),
                label: "Backoff".into(),
                description: None,
            }],
            placeholder: None,
            default_value: None,
            deadline: None,
            form: None,
            source: None,
        });
        let answer = |generation, revision, value: &str| d::Command::Answer {
            session: "target".into(),
            generation,
            revision,
            request: "question".into(),
            answer: UiAnswer::Choice(value.into()),
        };
        for command in [
            answer(6, 3, "backoff"),
            answer(7, 2, "backoff"),
            answer(7, 3, "invented"),
        ] {
            assert!(
                !s.dispatch_dashboard(command)
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::Backend(BackendRequest::UiRespond { .. })))
            );
        }
        s.dashboard.freshness = Freshness::Reconnecting;
        assert!(
            !s.dispatch_dashboard(answer(7, 3, "backoff"))
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(BackendRequest::UiRespond { .. })))
        );
        s.dashboard.freshness = Freshness::Live;
        let out = s.dispatch_dashboard(answer(7, 3, "backoff"));
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Backend(BackendRequest::UiRespond { conversation: ConversationId(2), id, answer: UiAnswer::Choice(value), .. }) if id == "question" && value == "backoff")));
        assert_eq!(s.selected, Some(ConversationId(1)));
        assert!(
            !s.dispatch_dashboard(answer(7, 3, "backoff"))
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Backend(BackendRequest::UiRespond { .. })))
        );
        assert_eq!(
            s.dashboard.rows[0].worker.state,
            d::WorkState::Ready,
            "sending does not optimistically clear authoritative state"
        );
    }
}

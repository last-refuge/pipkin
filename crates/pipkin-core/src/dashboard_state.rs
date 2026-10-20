use super::*;
use crate::dashboard::{self as d, Action, Freshness, Request};
impl AppState {
    fn dashboard_request(request: Request) -> Outcome {
        Outcome {
            effects: vec![Effect::Backend(BackendRequest::Dashboard(request))],
            notes: vec![Note::Other],
        }
    }
    fn dashboard_watch(&mut self) -> Outcome {
        self.dashboard.filter.seen_failures = self.dashboard.seen.clone();
        self.dashboard.generation += 1;
        self.dashboard.loading = true;
        self.dashboard.error = None;
        self.dashboard.freshness = Freshness::Updating;
        Self::dashboard_request(Request::Watch {
            generation: self.dashboard.generation,
            filter: self.dashboard.filter.clone(),
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
    pub fn dispatch_dashboard(&mut self, command: d::Command) -> Outcome {
        let mut out = Outcome {
            effects: vec![],
            notes: vec![Note::Other],
        };
        match command {
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
            d::Command::Filter(filter) => {
                self.dashboard.filter = filter;
                self.dashboard
                    .rows
                    .retain(|r| Some(&r.worker.session) == self.dashboard.selected.as_ref());
                self.dashboard.next = None;
                out.merge(self.dashboard_watch());
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
                        filter: self.dashboard.filter.clone(),
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
                    if self.selected != Some(id)
                        || !self
                            .conversation(id)
                            .is_some_and(|c| c.opened && c.lifecycle_ready)
                    {
                        if let Some(c) = self.conv_mut(id) {
                            c.generation += 1;
                            c.opened = false;
                            c.lifecycle_ready = false;
                            out.merge(Self::dashboard_request(Request::Attach {
                                conversation: id,
                                generation: c.generation,
                            }));
                        }
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
}

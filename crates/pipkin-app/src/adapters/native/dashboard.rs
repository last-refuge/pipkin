//! Store-only activity observation, with a separate explicit execution attachment.
use super::*;
use pipkin_core::dashboard::{self as d, Action, Request};

const REFRESH: Duration = Duration::from_millis(500);
#[derive(Default)]
pub(super) struct Watch {
    generation: u64,
    filter: pipkin_engine_protocol::ActivityFilter,
    group: Option<pipkin_engine_protocol::ActivityGroup>,
    pub(super) open: bool,
    pub(super) dirty: bool,
    pub(super) detail: Option<(String, u64)>,
    pub(super) detail_dirty: bool,
    older_history: bool,
    start: Option<pipkin_engine_protocol::ActivityCursor>,
    shown: usize,
    pub(super) next: Option<Instant>,
}
impl Live {
    fn dashboard_emit(worker: &Worker, event: d::Event) {
        (worker.sink)(LifecycleEvent::Dashboard(event));
    }
    pub(super) fn dashboard_request(&mut self, worker: &mut Worker, request: Request) {
        match request {
            Request::Watch {
                generation,
                filter,
                group,
            } => {
                if self.dashboard.filter != filter
                    || self.dashboard.group != group
                    || !self.dashboard.open
                {
                    self.dashboard.start = None;
                    self.dashboard.shown = 200;
                }
                self.dashboard.generation = generation;
                self.dashboard.filter = filter;
                self.dashboard.group = group;
                self.dashboard.open = true;
                self.dashboard.dirty = true;
                self.dashboard.next = Some(Instant::now());
                self.dashboard_tick(worker);
            }
            Request::ToolOutput {
                generation,
                session,
                call_id,
            } => {
                let result = match self.call(Call::ToolOutput {
                    session: session.clone(),
                    call_id: call_id.clone(),
                }) {
                    Ok(Reply::ToolOutput(view)) => Ok(view.text),
                    other => Err(format!("Could not read full tool output: {other:?}")),
                };
                Self::dashboard_emit(
                    worker,
                    d::Event::ToolOutput {
                        generation,
                        session,
                        call_id,
                        result,
                    },
                );
            }
            Request::Close { .. } => {
                let seen_failures = std::mem::take(&mut self.dashboard.filter.seen_failures);
                self.dashboard = Watch::default();
                self.dashboard.filter.seen_failures = seen_failures;
            }
            Request::More {
                generation,
                filter,
                group,
                after,
            } => {
                if self.dashboard.shown >= d::MAX_ROWS {
                    self.dashboard.start = Some(after);
                    self.dashboard.shown = 200;
                    self.dashboard_page(worker, generation, filter, Some(group), None);
                } else {
                    self.dashboard.shown += 200;
                    self.dashboard_page(worker, generation, filter, Some(group), Some(after));
                }
            }
            Request::Detail {
                generation,
                session,
                before,
            } => {
                self.dashboard.older_history = before.is_some();
                self.dashboard.detail = Some((session.clone(), generation));
                self.dashboard_history(worker, generation, session, before);
            }
            Request::Attach {
                conversation,
                generation,
            } => {
                worker
                    .attachment_generations
                    .insert(conversation, generation);
                if self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.conversation == conversation)
                {
                    self.open(worker, conversation, generation, true);
                } else {
                    std::mem::swap(&mut self.current, &mut self.secondary);
                    self.open(worker, conversation, generation, true);
                    std::mem::swap(&mut self.current, &mut self.secondary);
                }
            }
            Request::Control {
                generation,
                session,
                root,
                child,
                run,
                request,
                text,
                action,
                lookup,
            } => {
                let call = match (action, child, run, lookup) {
                    (Action::FollowUp, Some(child), _, true) => Call::LookupFollowUpSubagent {
                        session: root,
                        child,
                        request: request.clone(),
                    },
                    (Action::FollowUp, Some(child), _, false) => Call::FollowUpSubagent {
                        session: root,
                        child,
                        request: request.clone(),
                        text,
                    },
                    (Action::Steer, Some(_), _, true) => Call::Lookup {
                        session: session.clone(),
                        request: format!("steer:{request}"),
                    },
                    (Action::Steer, Some(child), _, false) => Call::SteerSubagent {
                        session: root,
                        child,
                        request: request.clone(),
                        text,
                    },
                    (Action::Stop, _, Some(expected_run), false) => Call::StopRun {
                        session: session.clone(),
                        expected_run,
                    },
                    (Action::Stop, Some(child), None, false) => Call::StopSubagent {
                        session: root,
                        child,
                        request: request.clone(),
                    },
                    _ => {
                        // A status lookup must never reissue an uncertain control.
                        let result = self.call(Call::ActivityRows {
                            sessions: vec![session],
                        });
                        let settled = matches!(&result,Ok(Reply::ActivityRows{rows,..}) if rows.first().is_some_and(|r|matches!(r.state,pipkin_engine_protocol::WorkState::Stopped|pipkin_engine_protocol::WorkState::Completed|pipkin_engine_protocol::WorkState::Ready)));
                        Self::dashboard_emit(
                            worker,
                            d::Event::Control {
                                generation,
                                request,
                                result: if settled && action == Action::Stop {
                                    Ok(None)
                                } else {
                                    Err("The original action is still unconfirmed. Inspect the task before issuing new instructions.".into())
                                },
                                uncertain: !(settled && action == Action::Stop),
                            },
                        );
                        return;
                    }
                };
                let result = self.call(call);
                let uncertain = matches!(result, Err(CallError::Lost(_)))
                    || (lookup
                        && matches!(result, Ok(Reply::Ok | Reply::Lookup(LookupResult::Unknown))));
                let result = match result {
                    Ok(Reply::Subagent(row)) => Ok(Some(row.session)),
                    Ok(_) => Ok(None),
                    Err(e) => Err(e.to_string()),
                };
                Self::dashboard_emit(
                    worker,
                    d::Event::Control {
                        generation,
                        request,
                        result,
                        uncertain,
                    },
                );
                self.dashboard.dirty = true;
                self.dashboard.next = Some(Instant::now() + REFRESH);
            }
        }
    }
    fn dashboard_page(
        &mut self,
        worker: &Worker,
        generation: u64,
        filter: pipkin_engine_protocol::ActivityFilter,
        group: Option<pipkin_engine_protocol::ActivityGroup>,
        after: Option<pipkin_engine_protocol::ActivityCursor>,
    ) {
        let append = after.is_some();
        let group = group.unwrap_or(pipkin_engine_protocol::ActivityGroup::Overview);
        let mut cursor = after.or_else(|| self.dashboard.start.clone());
        let wanted = if append {
            200
        } else {
            self.dashboard.shown.clamp(200, d::MAX_ROWS)
        };
        let mut combined: Option<pipkin_engine_protocol::ActivityPage> = None;
        loop {
            let response = self.call(Call::Activity {
                filter: filter.clone(),
                group,
                after: cursor,
                limit: 200,
            });
            match response {
                Ok(Reply::Activity(page)) => {
                    let next = page.next.clone();
                    if let Some(all) = combined.as_mut() {
                        all.rows.extend(page.rows);
                        all.next = page.next;
                        all.through = all.through.max(page.through);
                        all.counts = page.counts;
                    } else {
                        combined = Some(page);
                    }
                    if combined.as_ref().is_some_and(|p| p.rows.len() >= wanted) || next.is_none() {
                        break;
                    }
                    cursor = next;
                }
                other => {
                    Self::dashboard_emit(
                        worker,
                        d::Event::Failed {
                            generation,
                            session: None,
                            message: format!("Could not refresh orchestration: {other:?}"),
                        },
                    );
                    return;
                }
            }
        }
        let mut page = combined.expect("first page");
        if let Some((selected, detail_generation)) = self.dashboard.detail.clone()
            && !page.rows.iter().any(|r| r.session == selected)
        {
            match self.call(Call::ActivityRows {
                sessions: vec![selected.clone()],
            }) {
                Ok(Reply::ActivityRows { rows, missing, .. }) => {
                    page.rows.extend(rows);
                    if missing.contains(&selected) {
                        Self::dashboard_emit(
                            worker,
                            d::Event::Failed {
                                generation: detail_generation,
                                session: Some(selected),
                                message: "This worker was removed. Its controls are unavailable."
                                    .into(),
                            },
                        );
                    }
                }
                other => {
                    Self::dashboard_emit(
                        worker,
                        d::Event::Failed {
                            generation,
                            session: None,
                            message: format!("Could not refresh the selected worker: {other:?}"),
                        },
                    );
                    return;
                }
            }
        }
        let mut unique = std::collections::HashSet::new();
        page.rows.retain(|r| unique.insert(r.session.clone()));
        let rows = page
            .rows
            .iter()
            .cloned()
            .map(|worker| d::Row {
                conversation: if worker.kind == pipkin_engine_protocol::WorkerKind::Root {
                    self.conversation_of(&worker.session)
                } else {
                    None
                },
                root: self.conversation_of(&worker.root),
                worker,
            })
            .collect();
        Self::dashboard_emit(
            worker,
            d::Event::Directory {
                generation,
                rows,
                page,
                append,
            },
        );
    }

    fn dashboard_history(
        &mut self,
        worker: &Worker,
        generation: u64,
        session: String,
        before: Option<u64>,
    ) {
        match self.call(Call::ObserveHistory {
            session: session.clone(),
            before,
            limit: HISTORY_PAGE,
        }) {
            Ok(Reply::Page {
                entries,
                has_older,
                native_history_start,
            }) => {
                let oldest = entries.first().map(|e| e.id);
                let items = transcript::map_entries_with_origin(
                    &entries,
                    &HashMap::new(),
                    Some(native_history_start),
                )
                .items;
                Self::dashboard_emit(
                    worker,
                    d::Event::Detail {
                        generation,
                        session,
                        before,
                        items,
                        oldest,
                        has_older,
                    },
                );
            }
            other => Self::dashboard_emit(
                worker,
                d::Event::Failed {
                    generation,
                    session: Some(session),
                    message: format!("Could not read this history: {other:?}"),
                },
            ),
        }
    }
    pub(super) fn dashboard_counts(&self, worker: &Worker) {
        let filter = pipkin_engine_protocol::ActivityFilter {
            seen_failures: self.dashboard.filter.seen_failures.clone(),
            ..Default::default()
        };
        if let Ok(Reply::Activity(page)) = self.call(Call::Activity {
            filter,
            group: pipkin_engine_protocol::ActivityGroup::Overview,
            after: None,
            limit: 1,
        }) {
            Self::dashboard_emit(worker, d::Event::Counts(page.counts));
        }
    }
    pub(super) fn dashboard_tick(&mut self, worker: &Worker) {
        if self.dashboard.next.is_some_and(|n| Instant::now() >= n) {
            self.dashboard.next = None;
            if std::mem::take(&mut self.dashboard.dirty) {
                self.dashboard_counts(worker);
                if !self.dashboard.open {
                    return;
                }
                self.dashboard_page(
                    worker,
                    self.dashboard.generation,
                    self.dashboard.filter.clone(),
                    self.dashboard.group,
                    None,
                );
            }
            if std::mem::take(&mut self.dashboard.detail_dirty)
                && let Some((session, generation)) = self.dashboard.detail.clone()
            {
                self.dashboard_history(worker, generation, session, None);
            }
        }
    }
    pub(super) fn dashboard_invalidate(&mut self, session: &str) {
        {
            self.dashboard.dirty = true;
            if !self.dashboard.older_history
                && self
                    .dashboard
                    .detail
                    .as_ref()
                    .is_some_and(|(s, _)| s == session)
            {
                self.dashboard.detail_dirty = true;
            }
            self.dashboard.next.get_or_insert(Instant::now() + REFRESH);
        }
    }
    pub(super) fn secondary_request(&self, request: &BackendRequest) -> bool {
        let target = match request {
            BackendRequest::Submit { conversation, .. }
            | BackendRequest::Queue { conversation, .. }
            | BackendRequest::CheckStatus { conversation, .. }
            | BackendRequest::Cancel { conversation, .. }
            | BackendRequest::RefreshChanges { conversation, .. } => Some(*conversation),
            _ => None,
        };
        self.secondary
            .as_ref()
            .is_some_and(|c| Some(c.conversation) == target)
    }
}

//! Owned payload capacities, including vector storage and spare String capacity. Excludes
//! allocator metadata; this is an admission weight, not an RSS measurement. Exhaustive enum
//! matches force new event kinds to choose an accounting rule.
use pipkin_core::*;

fn strings(s: &Option<String>) -> usize {
    s.as_ref().map_or(0, String::capacity)
}
#[allow(clippy::ptr_arg)] // Capacity, not just initialized elements, occupies memory.
fn vec_bytes<T>(v: &Vec<T>, heap: impl Fn(&T) -> usize) -> usize {
    v.capacity() * size_of::<T>() + v.iter().map(heap).sum::<usize>()
}
fn item(i: &TranscriptItem) -> usize {
    match &i.kind {
        ItemKind::User {
            text, attachments, ..
        } => {
            text.capacity()
                + vec_bytes(attachments, |a| {
                    a.path.capacity() + a.name.capacity() + strings(&a.error)
                })
        }
        ItemKind::Assistant { text, .. }
        | ItemKind::Background { text, .. }
        | ItemKind::Notice { text, .. } => text.capacity(),
        ItemKind::Tool(t) => {
            strings(&t.call_id) + t.name.capacity() + t.input.capacity() + t.output.capacity()
        }
        ItemKind::Job(j) => {
            strings(&j.name)
                + j.command.capacity()
                + j.cwd.capacity()
                + strings(&j.reason)
                + strings(&j.call_id)
                + j.delivery.capacity()
        }
    }
}
fn change(c: &FileChange) -> usize {
    c.path.capacity()
        + vec_bytes(&c.hunks, |h| {
            h.header.capacity() + vec_bytes(&h.lines, |l| l.text.capacity())
        })
}
fn child(c: &SubagentInfo) -> usize {
    c.call_id.capacity() + c.task.capacity() + c.status.capacity()
}

fn approval_run(r: &CliApprovalRun) -> usize {
    r.session.capacity() + r.op.capacity()
}
fn approval_id(i: &CliApprovalId) -> usize {
    approval_run(&i.run) + i.invocation.capacity() + i.arguments_sha256.capacity()
}
fn approval_rows(rows: &Vec<CliApprovalReview>) -> usize {
    vec_bytes(rows, |r| approval_id(&r.id) + r.arguments.capacity())
}
fn approval_event(e: &CliApprovalEvent) -> usize {
    match e {
        CliApprovalEvent::DiscoveryFailed(_) => 0,
        CliApprovalEvent::Discovered { run, reviews, .. } => {
            run.as_ref().map_or(0, approval_run) + approval_rows(reviews)
        }
        CliApprovalEvent::Response(r) => {
            approval_run(&r.context.run)
                + match &r.result {
                    CliApprovalResult::Failed => 0,
                    CliApprovalResult::Receipt(receipt) => {
                        receipt.as_ref().map_or(0, |r| approval_id(&r.id))
                    }
                    CliApprovalResult::Listed { run, reviews } => {
                        run.as_ref().map_or(0, approval_run) + approval_rows(reviews)
                    }
                }
        }
    }
}
fn cli_identity(id: &LocalCliCreationId) -> usize {
    id.session.0.capacity() + id.setup.0.capacity()
}
fn cli_event(event: &LocalCliEvent) -> usize {
    match event {
        LocalCliEvent::Inspected(view)
        | LocalCliEvent::Checked(view)
        | LocalCliEvent::Status(view) => {
            let target = match &view.target {
                LocalCliInspectTarget::New { project, .. } => project.capacity(),
                LocalCliInspectTarget::Existing { session, .. } => session.0.capacity(),
            };
            view.setup.0.capacity()
                + target
                + view.root_candidate.as_ref().map_or(0, |s| s.0.capacity())
                + view.executable.capacity()
                + view.project.capacity()
                + view.qualified_version.capacity()
                + strings(&view.reason)
        }
        LocalCliEvent::Created(created) => cli_identity(&created.identity),
        LocalCliEvent::CreationLookup { identity, found } => {
            cli_identity(identity) + found.as_ref().map_or(0, |c| cli_identity(&c.identity))
        }
        LocalCliEvent::SessionStatus(view) => {
            view.session.0.capacity() + view.executable.capacity() + view.project.capacity()
        }
        LocalCliEvent::Failed { failure, .. } => match failure {
            LocalCliFailure::Unknown(reason) | LocalCliFailure::Rejected(reason) => {
                reason.capacity()
            }
            LocalCliFailure::Expired | LocalCliFailure::Conflict => 0,
        },
        LocalCliEvent::Revoked => 0,
    }
}

pub(super) fn event_bytes(event: &BackendEvent) -> usize {
    size_of::<BackendEvent>()
        + match &event.kind {
            EventKind::LocalCli(event) => cli_event(event),
            EventKind::CliApproval(event) => size_of::<CliApprovalEvent>() + approval_event(event),
            EventKind::ExecutionContext {
                execution_blocked, ..
            } => strings(execution_blocked),
            EventKind::Opened { items, changes, .. } => {
                vec_bytes(items, item) + vec_bytes(changes, change)
            }
            EventKind::OlderPage { items, .. }
            | EventKind::Synced { items }
            | EventKind::SubagentView { items, .. }
            | EventKind::SubagentPage { items, .. }
            | EventKind::SubagentOlderPage { items, .. } => vec_bytes(items, item),
            EventKind::SubagentsSynced { children, .. }
            | EventKind::SubagentDirectory { children, .. } => vec_bytes(children, child),
            EventKind::PlanSynced { tasks, .. } => vec_bytes(tasks, |t| {
                t.title.capacity()
                    + t.detail.capacity()
                    + t.criteria.capacity()
                    + t.reason.as_ref().map_or(0, String::capacity)
                    + t.owner.as_ref().map_or(0, String::capacity)
                    + t.depends_on.capacity() * size_of::<u64>()
                    + vec_bytes(&t.evidence, |e| {
                        e.kind.capacity()
                            + e.reference.capacity()
                            + e.note.as_ref().map_or(0, String::capacity)
                    })
            }),
            EventKind::GoalSynced { goal } => goal.as_ref().map_or(0, |g| {
                g.text.capacity() + g.paused.as_ref().map_or(0, String::capacity)
            }),
            EventKind::GoalRefused { reason } => reason.capacity(),
            EventKind::JobsSynced { jobs } => vec_bytes(&jobs.jobs, |j| {
                j.name.as_ref().map_or(0, String::capacity)
                    + j.command.capacity()
                    + j.cwd.capacity()
                    + j.reason.as_ref().map_or(0, String::capacity)
                    + j.call_id.as_ref().map_or(0, String::capacity)
                    + j.delivery.capacity()
            }),
            EventKind::JobLogPage { tail } => tail.text.capacity(),
            EventKind::JobRefused { reason } => reason.capacity(),
            EventKind::ContextSynced { context } => {
                context.model.as_ref().map_or(0, String::capacity) + context.compaction.capacity()
            }
            EventKind::WebSourcesSynced(sources) => vec_bytes(sources, |s| {
                s.id.capacity()
                    + s.url.capacity()
                    + strings(&s.title)
                    + s.provider.capacity()
                    + s.kind.capacity()
            }),
            EventKind::DiagnosticsSynced { records, .. } => vec_bytes(records, |r| {
                r.kind.capacity() + r.line.capacity() + strings(&r.call_id)
            }),
            EventKind::DiagnosticsExported { path, .. } => path.capacity(),
            EventKind::DiagnosticsFailed { reason } => reason.capacity(),
            EventKind::PlanEditApplied { request } | EventKind::SubagentControlDone { request } => {
                request.0.capacity()
            }
            EventKind::PlanEditFailed { request, reason }
            | EventKind::SubagentControlFailed { request, reason } => {
                request.0.capacity() + reason.capacity()
            }
            EventKind::UsageSynced(u) => {
                u.session_id.capacity()
                    + vec_bytes(&u.models, |v| v.0.capacity())
                    + vec_bytes(&u.tools, |v| v.0.capacity())
            }
            EventKind::ChangesReported(changes) | EventKind::ChangesSynced(changes) => {
                vec_bytes(changes, change)
            }
            EventKind::ChangesScanState(state) => match state {
                ChangesState::Unavailable(reason) => reason.capacity(),
                ChangesState::Unscanned
                | ChangesState::Loading
                | ChangesState::Ready
                | ChangesState::NotARepository => 0,
            },
            EventKind::ThinkingState { level, levels } => {
                level.capacity() + vec_bytes(levels, String::capacity)
            }
            EventKind::QueueAdmitted { request, .. } | EventKind::QueueAckLost { request } => {
                request.0.capacity()
            }
            EventKind::QueueRefused { request, reason } => request.0.capacity() + reason.capacity(),
            EventKind::SubagentsUnavailable(s)
            | EventKind::Token(s)
            | EventKind::SubagentViewFailed { reason: s, .. }
            | EventKind::SubagentOlderFailed { reason: s, .. }
            | EventKind::OpenFailed { message: s }
            | EventKind::Rejected { reason: s }
            | EventKind::StopFailed { message: s }
            | EventKind::StatusCheckFailed { message: s }
            | EventKind::Failed { message: s }
            | EventKind::OlderFailed { message: s }
            | EventKind::ToolOutput { chunk: s, .. } => s.capacity(),
            EventKind::ToolStarted { name, input, .. } => name.capacity() + input.capacity(),
            EventKind::ToolOutputFull { call_id, text } => call_id.capacity() + text.capacity(),
            EventKind::ToolOutputUnavailable { call_id, reason } => {
                call_id.capacity() + reason.capacity()
            }
            EventKind::UiRespondRefused { id, reason } => id.capacity() + reason.capacity(),
            EventKind::UiState {
                requests,
                status,
                notices,
            } => {
                vec_bytes(requests, |r| {
                    r.id.capacity()
                        + r.title.capacity()
                        + strings(&r.message)
                        + strings(&r.placeholder)
                        + strings(&r.default_value)
                        + strings(&r.source)
                        + r.form.as_ref().map_or(0, |f| {
                            vec_bytes(&f.questions, |q| {
                                q.id.capacity()
                                    + q.prompt.capacity()
                                    + strings(&q.description)
                                    + strings(&q.preview)
                                    + vec_bytes(&q.options, |o| {
                                        o.id.capacity()
                                            + o.label.capacity()
                                            + strings(&o.description)
                                            + strings(&o.preview)
                                    })
                            })
                        })
                        + vec_bytes(&r.items, |i| {
                            i.value.capacity() + i.label.capacity() + strings(&i.description)
                        })
                }) + vec_bytes(status, |(k, v)| k.capacity() + v.capacity())
                    + vec_bytes(notices, |n| n.id.capacity() + n.message.capacity())
            }
            EventKind::EngineState { queue, .. } => vec_bytes(queue, |q| q.text.capacity()),
            EventKind::Accepted
            | EventKind::AckLost
            | EventKind::StatusResolved { .. }
            | EventKind::ToolFinished { .. }
            | EventKind::Completed
            | EventKind::Cancelled
            | EventKind::SteerAccepted
            | EventKind::QueueCancelled { .. }
            | EventKind::HasOlder(_) => 0,
        }
}

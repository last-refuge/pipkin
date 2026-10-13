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
        ItemKind::Assistant { text, .. } | ItemKind::Notice { text, .. } => text.capacity(),
        ItemKind::Tool(t) => {
            strings(&t.call_id) + t.name.capacity() + t.input.capacity() + t.output.capacity()
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

pub(super) fn event_bytes(event: &BackendEvent) -> usize {
    size_of::<BackendEvent>()
        + match &event.kind {
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

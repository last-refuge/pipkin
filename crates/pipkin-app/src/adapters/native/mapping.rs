//! Pure mappings from the engine's typed views to the core's types: the catalogue (with the
//! same conversation ids the Pi adapter assigns), models, sign-in, extension questions,
//! subagents, outcomes, and the session's changed files.

use std::collections::{BTreeSet, HashMap};

use pipkin_core::onboarding::{SignInChallenge, SignInProvider, SignInSnapshot, SignInStatus};
use pipkin_core::{
    Bootstrap, ConversationId, EventKind, Goal, GoalState, ModelInfo, PlanCounts, PlanEvidence,
    PlanTask, PlanTaskStatus, Project, ProjectId, SubagentInfo, SubagentOrigin, UiAnswer, UiForm,
    UiFormKind, UiFormOption, UiFormQuestion, UiNotice, UiNoticeLevel, UiRequest, UiRequestItem,
    UiRequestKind, project_id_for_path, project_name_for_path,
};
use pipkin_engine_protocol::{
    Answer, AuthSnapshot, AuthStatus, Block, CredentialKind, Entry, EntryKind, ErrorCode,
    FormAnswer, FormKind, FormView, GoalView, Message, ModelsView, Origin, PlanView, QuestionKind,
    RunOutcome, SessionSummary, SubagentsView, UiView,
};

use crate::adapters::session::{self, Session};

/// The project of a session whose working directory is not usable.
pub const SESSIONS_PROJECT: ProjectId = session::SESSIONS_PROJECT;

const MAX_CHILDREN: usize = 4_096;
const MAX_TASK_CHARS: usize = 160;

/// The key a session's desktop id is derived from: an imported session keeps the legacy (Pi)
/// session id it had, so its drafts, cache and search hits stay attached to it.
pub fn identity_key(summary: &SessionSummary) -> &str {
    match &summary.origin {
        Origin::Imported { source, .. } if !source.is_empty() => source,
        _ => &summary.id,
    }
}

fn created_secs(summary: &SessionSummary) -> i64 {
    summary.created_at_ms.div_euclid(1000)
}

fn usable_cwd(summary: &SessionSummary) -> Option<&str> {
    Some(summary.cwd.as_str()).filter(|c| c.starts_with('/'))
}

/// Assign conversation ids exactly as `the preserved shared session::assign_ids` does, keyed by
/// [`identity_key`] and ordered by creation time.
pub fn assign_ids(sessions: &[SessionSummary]) -> Vec<(ConversationId, SessionSummary)> {
    let pi: Vec<Session> = sessions
        .iter()
        .map(|s| Session {
            session_id: identity_key(s).to_owned(),
            created_at: created_secs(s),
        })
        .collect();
    let by_key: HashMap<&str, &SessionSummary> =
        sessions.iter().map(|s| (identity_key(s), s)).collect();
    session::assign_ids(&pi)
        .into_iter()
        .filter_map(|(id, p)| Some((id, (*by_key.get(p.session_id.as_str())?).clone())))
        .collect()
}

pub fn title(summary: &SessionSummary) -> String {
    match summary.title.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t.to_owned(),
        _ => {
            let short: String = identity_key(summary).chars().take(8).collect();
            format!(
                "Session {short} \u{b7} {}",
                session::format_time(created_secs(summary))
            )
        }
    }
}

fn project_of(summary: &SessionSummary) -> ProjectId {
    usable_cwd(summary).map_or(SESSIONS_PROJECT, project_id_for_path)
}

/// One project per working directory (plus a fallback), conversations in id order.
pub fn catalog(
    sessions: &[(ConversationId, SessionSummary)],
    models: Vec<ModelInfo>,
    location: &str,
    now: i64,
) -> Bootstrap {
    let mut projects: Vec<Project> = Vec::new();
    for (_, s) in sessions {
        let id = project_of(s);
        if projects.iter().any(|p| p.id == id) {
            continue;
        }
        projects.push(match usable_cwd(s) {
            Some(cwd) => Project {
                id,
                name: project_name_for_path(cwd),
                path: cwd.to_owned(),
            },
            None => Project {
                id,
                name: "Sessions".into(),
                path: location.into(),
            },
        });
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    Bootstrap {
        projects,
        models,
        conversations: sessions
            .iter()
            .map(|(id, s)| (*id, project_of(s), title(s), created_secs(s)))
            .collect(),
        now,
    }
}

/// Models the engine can use now, the selected one first.
pub fn models(view: &ModelsView) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = view
        .models
        .iter()
        .filter(|m| m.available)
        .map(|m| ModelInfo {
            id: m.key.clone(),
            name: if m.name.is_empty() {
                m.key.clone()
            } else {
                m.name.clone()
            },
            note: if m.reasoning {
                format!("{} \u{b7} reasoning", m.provider)
            } else {
                m.provider.clone()
            },
        })
        .collect();
    if let Some(selected) = &view.selected
        && let Some(at) = out.iter().position(|m| &m.id == selected)
    {
        let m = out.remove(at);
        out.insert(0, m);
    }
    out
}

/// What went wrong reading providers, as one line, or `None` when nothing did.
pub fn refresh_warning(view: &ModelsView) -> Option<String> {
    if view.warnings.is_empty() {
        return None;
    }
    let mut problems = view.warnings.clone();
    problems.sort();
    Some(format!(
        "Some providers could not be read ({}). Check their sign-in or API key, then refresh the models.",
        problems.join("; ")
    ))
}

fn provider_of(id: &str) -> Option<SignInProvider> {
    match id {
        "anthropic" => Some(SignInProvider::Claude),
        "openai-codex" => Some(SignInProvider::ChatGpt),
        _ => None,
    }
}

fn bounded(text: Option<&str>, max: usize) -> Option<String> {
    text.filter(|s| s.len() <= max).map(str::to_owned)
}

/// The engine's per-client sign-in state as the onboarding flow shows it. Only the two
/// allowlisted subscription providers are offered; one the engine reports unavailable (a
/// reserved integration) is listed as such and never offered for reuse.
pub fn sign_in(snapshot: &AuthSnapshot) -> SignInSnapshot {
    let mut existing = Vec::new();
    let mut unavailable = Vec::new();
    for p in &snapshot.providers {
        let Some(provider) = provider_of(&p.id) else {
            continue;
        };
        if !p.available {
            if !unavailable.iter().any(|(q, _)| *q == provider) {
                let reason = p
                    .unavailable_reason
                    .clone()
                    .filter(|r| !r.trim().is_empty())
                    .unwrap_or_else(|| "this engine does not support it yet.".into());
                unavailable.push((provider, reason));
            }
        } else if p.has_credentials
            && p.kind == CredentialKind::OAuth
            && !existing.contains(&provider)
        {
            existing.push(provider);
        }
    }
    let challenge = snapshot.challenge.as_ref().and_then(|c| {
        let options: Vec<(String, String)> = c
            .options
            .iter()
            .take(32)
            .filter(|(id, label)| id.len() <= 128 && label.len() <= 256)
            .cloned()
            .collect();
        let kind = match c.kind.as_str() {
            "code" | "manual_code" => "manual_code",
            "select" | "confirm" if !options.is_empty() => "select",
            "text" | "confirm" => "text",
            _ => return None,
        };
        Some(SignInChallenge {
            id: bounded(Some(&c.id), 128)?,
            kind: kind.to_owned(),
            message: bounded(Some(&c.message), 2048)?,
            placeholder: bounded(c.placeholder.as_deref(), 256),
            options,
        })
    });
    SignInSnapshot {
        available: true,
        credentials_known: !snapshot.credential_lookup_failed,
        credential_lookup_failed: snapshot.credential_lookup_failed,
        has_existing_credentials: snapshot.providers.iter().any(|p| p.has_credentials),
        existing_providers: existing,
        attempt: bounded(snapshot.attempt.as_deref(), 128),
        provider: snapshot.provider.as_deref().and_then(provider_of),
        status: match snapshot.status {
            AuthStatus::Idle => SignInStatus::Idle,
            AuthStatus::Connecting => SignInStatus::Connecting,
            AuthStatus::Waiting => SignInStatus::Waiting,
            AuthStatus::Prompt => SignInStatus::Prompt,
            AuthStatus::Done => SignInStatus::Done,
            AuthStatus::Failed => SignInStatus::Failed,
        },
        message: bounded(Some(&snapshot.message), 4096).unwrap_or_default(),
        url: bounded(snapshot.url.as_deref(), 8192).filter(|u| u.starts_with("https://")),
        device_code: bounded(snapshot.device_code.as_deref(), 128),
        challenge,
        unavailable,
    }
}

/// Questions, status lines and notices extensions posted.
pub fn ui_state(
    view: &UiView,
) -> (
    Vec<pipkin_core::UiRequest>,
    Vec<(String, String)>,
    Vec<UiNotice>,
) {
    let requests = view
        .questions
        .iter()
        .map(|q| UiRequest {
            id: q.id.clone(),
            kind: match q.kind {
                QuestionKind::Select => UiRequestKind::Select,
                QuestionKind::Confirm => UiRequestKind::Confirm,
                QuestionKind::Input => UiRequestKind::Input,
                QuestionKind::Form => UiRequestKind::Form,
            },
            form: q
                .form
                .as_ref()
                .filter(|_| q.kind == QuestionKind::Form)
                .map(form),
            source: q.source.clone(),
            title: q.title.clone(),
            message: q.message.clone(),
            items: q
                .options
                .iter()
                .map(|o| UiRequestItem {
                    value: o.value.clone(),
                    label: o.label.clone(),
                    description: o.description.clone(),
                })
                .collect(),
            placeholder: q.placeholder.clone(),
            default_value: q.default_value.clone(),
            deadline: q.deadline_ms,
        })
        .collect();
    let mut status = view.status.clone();
    status.sort();
    let notices = view
        .notices
        .iter()
        .map(|n| UiNotice {
            id: n.id.clone(),
            level: match n.level {
                pipkin_engine_protocol::NoticeLevel::Info => UiNoticeLevel::Info,
                pipkin_engine_protocol::NoticeLevel::Warning => UiNoticeLevel::Warning,
                pipkin_engine_protocol::NoticeLevel::Error => UiNoticeLevel::Error,
            },
            message: n.message.clone(),
        })
        .collect();
    (requests, status, notices)
}

/// A questionnaire as the engine published it; ids are carried as given.
fn form(view: &FormView) -> UiForm {
    UiForm {
        allow_note: view.allow_note,
        questions: view
            .questions
            .iter()
            .map(|q| UiFormQuestion {
                id: q.id.clone(),
                kind: match q.kind {
                    FormKind::Single => UiFormKind::Single,
                    FormKind::Multiple => UiFormKind::Multiple,
                    FormKind::Text => UiFormKind::Text,
                },
                prompt: q.prompt.clone(),
                description: q.description.clone(),
                preview: q.preview.clone(),
                options: q
                    .options
                    .iter()
                    .map(|o| UiFormOption {
                        id: o.id.clone(),
                        label: o.label.clone(),
                        description: o.description.clone(),
                        preview: o.preview.clone(),
                    })
                    .collect(),
                allow_other: q.allow_other,
                allow_note: q.allow_note,
                default: q.default.clone(),
                placeholder: q.placeholder.clone(),
            })
            .collect(),
    }
}

pub fn answer(answer: UiAnswer) -> Answer {
    match answer {
        UiAnswer::Choice(value) => Answer::Choice { value },
        UiAnswer::Confirm(value) => Answer::Confirm { value },
        UiAnswer::Text(value) => Answer::Text { value },
        UiAnswer::Form(form) => Answer::Form {
            answers: form
                .answers
                .into_iter()
                .map(|a| FormAnswer {
                    question: a.question,
                    selected: a.selected,
                    text: a.text,
                    note: a.note,
                })
                .collect(),
            note: form.note,
        },
    }
}

/// The child directory with the status names the core (and the Pi adapter) use. Anything
/// malformed makes the directory unavailable rather than partially shown.
/// Most plan tasks carried to the core; the engine already bounds live tasks below this.
const MAX_PLAN_TASKS_SHOWN: usize = 2_048;
const MAX_PLAN_TEXT_CHARS: usize = 4_096;

fn plan_status(status: pipkin_engine_protocol::TaskStatus) -> PlanTaskStatus {
    use pipkin_engine_protocol::TaskStatus as S;
    match status {
        S::Pending => PlanTaskStatus::Pending,
        S::Active => PlanTaskStatus::Active,
        S::Blocked => PlanTaskStatus::Blocked,
        S::Completed => PlanTaskStatus::Completed,
        S::Skipped => PlanTaskStatus::Skipped,
        S::Cancelled => PlanTaskStatus::Cancelled,
    }
}

pub fn task_status_to_engine(status: PlanTaskStatus) -> pipkin_engine_protocol::TaskStatus {
    use pipkin_engine_protocol::TaskStatus as S;
    match status {
        PlanTaskStatus::Pending => S::Pending,
        PlanTaskStatus::Active => S::Active,
        PlanTaskStatus::Blocked => S::Blocked,
        PlanTaskStatus::Completed => S::Completed,
        PlanTaskStatus::Skipped => S::Skipped,
        PlanTaskStatus::Cancelled => S::Cancelled,
    }
}

fn clip_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The engine's plan view as the core's projection: `(revision, tasks, counts, truncated)`.
/// Text is bounded; `truncated` also covers tasks this mapping left out.
pub fn plan(view: &PlanView) -> (u64, Vec<PlanTask>, PlanCounts, bool) {
    let tasks: Vec<PlanTask> = view
        .tasks
        .iter()
        .take(MAX_PLAN_TASKS_SHOWN)
        .map(|t| PlanTask {
            id: t.id,
            parent: t.parent,
            title: clip_chars(&t.title, MAX_PLAN_TEXT_CHARS),
            detail: clip_chars(&t.detail, MAX_PLAN_TEXT_CHARS),
            status: plan_status(t.status),
            reason: t
                .reason
                .as_deref()
                .map(|r| clip_chars(r, MAX_PLAN_TEXT_CHARS)),
            depends_on: t.depends_on.clone(),
            criteria: clip_chars(&t.criteria, MAX_PLAN_TEXT_CHARS),
            evidence: t
                .evidence
                .iter()
                .map(|e| PlanEvidence {
                    kind: clip_chars(&e.kind, MAX_PLAN_TEXT_CHARS),
                    reference: clip_chars(&e.reference, MAX_PLAN_TEXT_CHARS),
                    note: e
                        .note
                        .as_deref()
                        .map(|n| clip_chars(n, MAX_PLAN_TEXT_CHARS)),
                })
                .collect(),
            owner: t
                .owner
                .as_deref()
                .map(|o| clip_chars(o, MAX_PLAN_TEXT_CHARS)),
            archived: t.archived,
            stale: t.stale,
            revision: t.revision,
        })
        .collect();
    let counts = PlanCounts {
        pending: view.counts.pending,
        active: view.counts.active,
        blocked: view.counts.blocked,
        completed: view.counts.completed,
        skipped: view.counts.skipped,
        cancelled: view.counts.cancelled,
        archived: view.counts.archived,
    };
    let truncated = view.truncated || view.tasks.len() > MAX_PLAN_TASKS_SHOWN;
    (view.revision, tasks, counts, truncated)
}

/// The engine's goal as the core's `Goal`: the objective, a pause reason (the engine's
/// reason, or its state label when it gave none) whenever it is not working, and the state.
pub fn goal(view: &GoalView) -> Goal {
    let state = GoalState::parse(view.state.as_str()).unwrap_or(GoalState::Paused);
    let paused = match state {
        GoalState::Active => None,
        other => Some(
            view.reason
                .clone()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| other.label().to_owned()),
        ),
    };
    Goal {
        text: view.objective.chars().take(MAX_PLAN_TEXT_CHARS).collect(),
        paused,
        turns: view.turns,
        native: true,
        id: Some(view.id),
        state: Some(state),
        criteria_met: view
            .criteria
            .iter()
            .filter(|c| c.status == pipkin_engine_protocol::CriterionStatus::Met)
            .count() as u32,
        criteria_total: view.criteria.len() as u32,
        usage_tokens: view.usage_tokens,
        max_turns: view.limits.max_turns,
        max_tokens: view.limits.max_tokens,
        audit_required: view.audit.required,
        audit_verdict: view.audit.result.as_ref().map(|r| r.verdict.clone()),
    }
}

pub fn subagents(view: &SubagentsView) -> Result<Vec<SubagentInfo>, String> {
    if view.children.len() > MAX_CHILDREN {
        return Err("This session has too many subagents to display safely.".into());
    }
    view.children
        .iter()
        .map(|row| {
            if row.call_id.is_empty() || row.call_id.len() > 256 {
                return Err("The engine returned an invalid subagent: Invalid parent call.".into());
            }
            // The engine records terminal tasks as `done`/`aborted`; the protocol also names
            // them `completed`/`stopped`. Both spellings mean the same status.
            let status = match row.status.as_str() {
                "completed" | "done" => "done",
                "stopped" | "aborted" => "aborted",
                s @ ("pending" | "running" | "waiting" | "completing" | "failed") => s,
                _ => {
                    return Err(
                        "The engine returned an invalid subagent: Invalid child status.".into(),
                    );
                }
            };
            let origin = match row.origin.as_str() {
                "model" => SubagentOrigin::Model,
                "user" => SubagentOrigin::User,
                _ => {
                    return Err("The engine returned an invalid subagent: Invalid origin.".into());
                }
            };
            Ok(SubagentInfo {
                id: row.child,
                task_id: row.task_id,
                call_id: row.call_id.clone(),
                task: row.task.chars().take(MAX_TASK_CHARS).collect(),
                status: status.to_owned(),
                profile: row
                    .profile
                    .clone()
                    .filter(|p| !p.is_empty() && p.len() <= 64),
                origin,
                background: row.background,
                delivery: row.delivery.chars().take(32).collect(),
                note: row
                    .note
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .map(|n| n.chars().take(200).collect()),
                has_result: row.has_result,
            })
        })
        .collect()
}

/// How a settled run ends in the core.
pub fn outcome(outcome: Option<&RunOutcome>) -> EventKind {
    let detail = |d: &str, fallback: &str| {
        if d.trim().is_empty() {
            fallback.to_owned()
        } else {
            d.to_owned()
        }
    };
    match outcome {
        None | Some(RunOutcome::Completed) => EventKind::Completed,
        Some(RunOutcome::Aborted) => EventKind::Cancelled,
        Some(RunOutcome::ModelError { detail: d }) => EventKind::Failed {
            message: detail(d, "The model returned an error."),
        },
        Some(RunOutcome::NoModel { detail: d }) => EventKind::Failed {
            message: detail(
                d,
                "No model is selected for this session. Choose one and send again.",
            ),
        },
        Some(RunOutcome::Interrupted { detail: d }) => EventKind::Failed {
            message: detail(
                d,
                "The engine stopped while this run was in progress. Its last effects cannot be confirmed; inspect the project before sending more work.",
            ),
        },
        Some(RunOutcome::Failed { detail: d }) => EventKind::Failed {
            message: detail(d, "The run ended without an answer."),
        },
    }
}

/// Error codes after which the engine definitely did not take a submission.
pub fn definite_refusal(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::Busy
            | ErrorCode::Conflict
            | ErrorCode::Invalid
            | ErrorCode::TooLarge
            | ErrorCode::NotFound
            | ErrorCode::Unsupported
            | ErrorCode::Incompatible
            | ErrorCode::Storage
    )
}

/// Files this session changed: paths of successful typed `write`/`edit` calls whose result
/// entry is newer than `after` (the last clean scan), as the Pi adapter scopes them.
pub fn changed_paths<'a>(entries: impl IntoIterator<Item = &'a Entry>, after: u64) -> Vec<String> {
    let mut calls: HashMap<&str, (&str, Option<&str>)> = HashMap::new();
    let mut paths = BTreeSet::new();
    for entry in entries {
        let EntryKind::Message { message, .. } = &entry.kind else {
            continue;
        };
        match message {
            Message::Assistant { content, .. } => {
                for block in content {
                    if let Block::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = block
                    {
                        calls.insert(id, (name, arguments.get("path").and_then(|p| p.as_str())));
                    }
                }
            }
            Message::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => {
                let Some((name, path)) = calls.remove(tool_call_id.as_str()) else {
                    continue;
                };
                if entry.id <= after || *is_error || !matches!(name, "edit" | "write") {
                    continue;
                }
                if let Some(path) = path.filter(|p| !p.is_empty()) {
                    paths.insert(path.to_owned());
                }
            }
            Message::User { .. } | Message::System(_) => {}
        }
    }
    paths.into_iter().collect()
}

/// Whether an entry carries file-change evidence (a tool call or result).
pub fn is_change_evidence(entry: &Entry) -> bool {
    matches!(
        &entry.kind,
        EntryKind::Message {
            message: Message::ToolResult { .. },
            ..
        }
    ) || matches!(&entry.kind, EntryKind::Message { message: m @ Message::Assistant { .. }, .. } if !m.tool_calls().is_empty())
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{
        AuthChallenge, AuthProviderView, ModelView, NoticeView, QuestionOption, QuestionView,
        SubagentRow,
    };
    use serde_json::{Map, json};

    use super::*;

    fn summary(id: &str, created_ms: i64, cwd: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            cwd: cwd.into(),
            title: None,
            created_at_ms: created_ms,
            updated_at_ms: created_ms,
            origin: Origin::Native,
            model: None,
            blocked: None,
        }
    }

    fn imported(id: &str, source: &str, created_ms: i64) -> SessionSummary {
        SessionSummary {
            origin: Origin::Imported {
                engine: "pi".into(),
                source: source.into(),
            },
            ..summary(id, created_ms, "/work/app")
        }
    }

    #[test]
    fn imported_sessions_keep_the_desktop_ids_the_pi_adapter_gave_them() {
        let pi_rows = vec![
            session::Session {
                session_id: "pi-a".into(),
                created_at: 1_700_000_000,
            },
            session::Session {
                session_id: "pi-b".into(),
                created_at: 1_700_000_100,
            },
        ];
        let pi_ids = session::assign_ids(&pi_rows);
        let native = assign_ids(&[
            imported("n-2", "pi-b", 1_700_000_100_000),
            summary("n-new", 1_700_000_200_000, "/work/app"),
            imported("n-1", "pi-a", 1_700_000_000_000),
        ]);
        let id_of = |key: &str| native.iter().find(|(_, s)| s.id == key).unwrap().0;
        assert_eq!(id_of("n-1"), pi_ids[0].0);
        assert_eq!(id_of("n-2"), pi_ids[1].0);
        assert_eq!(
            id_of("n-new"),
            pipkin_core::ConversationId(pipkin_core::stable_id("n-new"))
        );
    }

    #[test]
    fn titles_come_from_the_engine_or_are_derived_like_pi_titles() {
        let mut s = imported("n-1", "0123456789abcdef", 1_700_000_000_000);
        assert_eq!(title(&s), "Session 01234567 \u{b7} 2023-11-14 22:13");
        s.title = Some("  Fix the build ".into());
        assert_eq!(title(&s), "Fix the build");
        s.title = Some("   ".into());
        assert!(title(&s).starts_with("Session "));
    }

    #[test]
    fn the_catalog_groups_by_working_directory_with_a_fallback() {
        let sessions = assign_ids(&[
            summary("a", 1_000, "/work/app"),
            summary("b", 2_000, "/work/app"),
            summary("c", 3_000, "relative"),
        ]);
        let boot = catalog(&sessions, vec![], "/data/engine", 9);
        let names: Vec<_> = boot.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Sessions", "app"]);
        assert_eq!(boot.conversations.len(), 3);
        assert_eq!(boot.conversations[0].3, 1, "created seconds");
        assert_eq!(boot.now, 9);
    }

    fn model(key: &str, available: bool, reasoning: bool) -> ModelView {
        let (provider, id) = key.split_once('/').unwrap();
        ModelView {
            key: key.into(),
            name: id.to_uppercase(),
            provider: provider.into(),
            api: "x".into(),
            reasoning,
            images: false,
            context_window: 1,
            max_tokens: 1,
            available,
            note: None,
        }
    }

    #[test]
    fn only_usable_models_are_offered_with_the_selection_first() {
        let view = ModelsView {
            models: vec![
                model("anthropic/a", false, false),
                model("openai/b", true, false),
                model("openai-codex/c", true, true),
            ],
            selected: Some("openai-codex/c".into()),
            warnings: vec![],
        };
        let list = models(&view);
        assert_eq!(
            list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["openai-codex/c", "openai/b"]
        );
        assert_eq!(list[0].note, "openai-codex \u{b7} reasoning");
        assert_eq!(refresh_warning(&view), None);
        let warned = ModelsView {
            warnings: vec!["z: bad key".into(), "a: expired".into()],
            ..view
        };
        assert!(
            refresh_warning(&warned)
                .unwrap()
                .contains("a: expired; z: bad key")
        );
    }

    fn provider(id: &str, available: bool, creds: bool, reason: Option<&str>) -> AuthProviderView {
        AuthProviderView {
            id: id.into(),
            name: id.into(),
            kind: CredentialKind::OAuth,
            available,
            unavailable_reason: reason.map(str::to_owned),
            has_credentials: creds,
        }
    }

    #[test]
    fn a_reserved_claude_sign_in_is_reported_unavailable_and_never_offered_for_reuse() {
        let snapshot = AuthSnapshot {
            providers: vec![
                provider(
                    "anthropic",
                    false,
                    true,
                    Some("Reserved for a later release."),
                ),
                provider("openai-codex", true, true, None),
                provider("openai", true, true, None),
                provider("github-copilot", true, true, None),
                provider("openrouter", true, true, None),
                provider("kimi-coding", true, true, None),
                provider("meta", true, true, None),
                provider("radius", true, true, None),
                provider("xai", true, true, None),
                provider("custom-radius", true, true, None),
            ],
            ..AuthSnapshot::default()
        };
        let s = sign_in(&snapshot);
        assert!(s.available && s.credentials_known && s.has_existing_credentials);
        assert_eq!(s.existing_providers, vec![SignInProvider::ChatGpt]);
        assert_eq!(
            s.unavailable_reason(SignInProvider::Claude),
            Some("Reserved for a later release.")
        );
        assert!(s.can_sign_in(SignInProvider::ChatGpt));
        assert!(!s.can_sign_in(SignInProvider::Claude));
        // No reason given: still unavailable, with a generic reason.
        let bare = sign_in(&AuthSnapshot {
            providers: vec![provider("anthropic", false, false, None)],
            ..AuthSnapshot::default()
        });
        assert!(bare.unavailable_reason(SignInProvider::Claude).is_some());
    }

    #[test]
    fn the_engine_manual_code_prompt_reaches_the_desktop_input() {
        let prompt = pipkin_engine::auth::oauth::Prompt::manual_code(
            "Paste the code Anthropic shows after you sign in:",
            "code#state",
        );
        let snapshot = AuthSnapshot {
            attempt: Some("claude-attempt".into()),
            provider: Some("anthropic".into()),
            status: AuthStatus::Prompt,
            challenge: Some(AuthChallenge {
                id: "claude-code".into(),
                kind: prompt.kind.as_str().into(),
                message: prompt.message.clone(),
                placeholder: prompt.placeholder.clone(),
                options: prompt.options,
            }),
            ..AuthSnapshot::default()
        };
        let view = sign_in(&snapshot);
        assert_eq!(view.status, SignInStatus::Prompt);
        assert_eq!(view.provider, Some(SignInProvider::Claude));
        assert_eq!(view.attempt, snapshot.attempt);
        let challenge = view.challenge.expect("manual-code input must be shown");
        assert_eq!(challenge.kind, "manual_code");
        assert_eq!(challenge.id, "claude-code");
        assert_eq!(challenge.message, prompt.message);
        assert_eq!(challenge.placeholder, prompt.placeholder);
    }

    #[test]
    fn sign_in_progress_maps_and_unsafe_values_are_dropped() {
        let snapshot = AuthSnapshot {
            providers: vec![],
            credential_lookup_failed: true,
            attempt: Some("a1".into()),
            provider: Some("openai-codex".into()),
            status: AuthStatus::Prompt,
            message: "Paste the code".into(),
            url: Some("javascript:alert(1)".into()),
            device_code: Some("ABCD".into()),
            challenge: Some(AuthChallenge {
                id: "ch".into(),
                kind: "code".into(),
                message: "Paste the redirect URL".into(),
                placeholder: None,
                options: vec![],
            }),
        };
        let s = sign_in(&snapshot);
        assert_eq!(s.status, SignInStatus::Prompt);
        assert_eq!(s.provider, Some(SignInProvider::ChatGpt));
        assert!(s.credential_lookup_failed && !s.credentials_known);
        assert_eq!(s.url, None);
        assert_eq!(s.device_code.as_deref(), Some("ABCD"));
        assert_eq!(s.challenge.as_ref().unwrap().kind, "manual_code");
        let https = sign_in(&AuthSnapshot {
            url: Some("https://auth.example/x".into()),
            ..AuthSnapshot::default()
        });
        assert_eq!(https.url.as_deref(), Some("https://auth.example/x"));
        assert!(!format!("{https:?}").contains("auth.example"));
    }

    #[test]
    fn questions_and_notices_map() {
        let view = UiView {
            questions: vec![QuestionView {
                id: "q".into(),
                kind: QuestionKind::Select,
                title: "Pick".into(),
                message: None,
                options: vec![QuestionOption {
                    value: "a".into(),
                    label: "A".into(),
                    description: None,
                }],
                placeholder: None,
                default_value: None,
                deadline_ms: Some(5),
                form: None,
                source: None,
            }],
            status: vec![("z".into(), "2".into()), ("a".into(), "1".into())],
            notices: vec![NoticeView {
                id: "n".into(),
                level: pipkin_engine_protocol::NoticeLevel::Warning,
                message: "careful".into(),
            }],
        };
        let (requests, status, notices) = ui_state(&view);
        assert_eq!(requests[0].kind, UiRequestKind::Select);
        assert_eq!(requests[0].items[0].label, "A");
        assert_eq!(requests[0].deadline, Some(5));
        assert_eq!(status[0].0, "a");
        assert_eq!(notices[0].level, UiNoticeLevel::Warning);
        assert_eq!(
            answer(UiAnswer::Confirm(true)),
            Answer::Confirm { value: true }
        );
    }

    fn row(child: u64, status: &str) -> SubagentRow {
        SubagentRow {
            child,
            task_id: 8,
            call_id: "call-1".into(),
            task: "x".repeat(1000),
            status: status.into(),
            session: "s".into(),
            profile: None,
            origin: "model".into(),
            background: false,
            delivery: "inline".into(),
            note: None,
            has_result: false,
        }
    }

    #[test]
    fn subagent_statuses_use_the_names_the_core_expects() {
        let list = subagents(&SubagentsView {
            enabled: true,
            children: vec![
                row(1, "completed"),
                row(2, "stopped"),
                row(3, "waiting"),
                row(4, "done"),
                row(5, "aborted"),
            ],
            truncated: false,
        })
        .unwrap();
        let statuses: Vec<_> = list.iter().map(|c| c.status.as_str()).collect();
        assert_eq!(statuses, ["done", "aborted", "waiting", "done", "aborted"]);
        assert_eq!(list[0].task.chars().count(), MAX_TASK_CHARS);
        assert!(
            subagents(&SubagentsView {
                enabled: true,
                children: vec![row(1, "mystery")],
                truncated: false
            })
            .is_err()
        );
    }

    #[test]
    fn outcomes_map_to_settlement() {
        assert_eq!(outcome(Some(&RunOutcome::Completed)), EventKind::Completed);
        assert_eq!(outcome(None), EventKind::Completed);
        assert_eq!(outcome(Some(&RunOutcome::Aborted)), EventKind::Cancelled);
        for o in [
            RunOutcome::ModelError {
                detail: "429".into(),
            },
            RunOutcome::NoModel {
                detail: String::new(),
            },
            RunOutcome::Interrupted {
                detail: String::new(),
            },
            RunOutcome::Failed { detail: "x".into() },
        ] {
            assert!(
                matches!(outcome(Some(&o)), EventKind::Failed { message } if !message.is_empty())
            );
        }
        assert!(definite_refusal(ErrorCode::Busy));
        assert!(!definite_refusal(ErrorCode::Internal));
        assert!(!definite_refusal(ErrorCode::ShuttingDown));
        assert!(!definite_refusal(ErrorCode::Cancelled));
    }

    fn msg(id: u64, message: Message) -> Entry {
        Entry {
            id,
            at_ms: 0,
            run: None,
            retired: false,
            kind: EntryKind::Message {
                message,
                mode: Default::default(),
                request: None,
                partial: false,
            },
        }
    }

    fn call(id: u64, call_id: &str, name: &str, path: &str) -> Entry {
        msg(
            id,
            Message::Assistant {
                content: vec![Block::tool_call(call_id, name, json!({ "path": path }))],
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Default::default(),
                stop_reason: Default::default(),
                error_message: None,
                timestamp: 0,
                extra: Map::new(),
            },
        )
    }

    fn result(id: u64, call_id: &str, is_error: bool) -> Entry {
        msg(
            id,
            Message::ToolResult {
                tool_call_id: call_id.into(),
                tool_name: String::new(),
                content: vec![],
                is_error,
                timestamp: 0,
                extra: Map::new(),
            },
        )
    }

    #[test]
    fn changed_files_are_successful_session_edits_after_the_last_clean_scan() {
        let entries = vec![
            call(1, "c1", "write", "a.rs"),
            result(2, "c1", false),
            call(3, "c2", "edit", "b.rs"),
            result(4, "c2", true),
            call(5, "c3", "read", "c.rs"),
            result(6, "c3", false),
            call(7, "c4", "edit", "d.rs"),
            result(8, "c4", false),
        ];
        assert_eq!(changed_paths(&entries, 0), ["a.rs", "d.rs"]);
        assert_eq!(
            changed_paths(&entries, 2),
            ["d.rs"],
            "committed calls retire"
        );
        assert!(changed_paths(&entries, 8).is_empty());
        assert!(is_change_evidence(&entries[0]) && is_change_evidence(&entries[1]));
        assert!(!is_change_evidence(&msg(9, Message::user_text("x", 0))));
    }
}

// ------------------------------------------------------------------ context usage

pub fn context(view: &pipkin_engine_protocol::ContextUsageView) -> pipkin_core::ContextUsage {
    use pipkin_core::ContextSource;
    pipkin_core::ContextUsage {
        tokens: view.tokens,
        source: match view.source.as_str() {
            "reported" => ContextSource::Reported,
            "estimated" => ContextSource::Estimated,
            _ => ContextSource::Unavailable,
        },
        capacity: view.capacity,
        reserve: view.reserve,
        remaining: view.remaining,
        model: view.model.clone(),
        stale: view.stale,
        compaction: view.compaction.clone(),
        revision: view.revision,
    }
}

// ------------------------------------------------------------------ background jobs

pub fn jobs(view: &pipkin_engine_protocol::JobsView) -> pipkin_core::JobsSync {
    use pipkin_core::{Job, JobOrigin, JobState};
    pipkin_core::JobsSync {
        jobs: view
            .jobs
            .iter()
            .map(|j| Job {
                id: j.id,
                name: j.name.clone(),
                command: j.command.clone(),
                cwd: j.cwd.clone(),
                origin: if j.origin == "user" {
                    JobOrigin::User
                } else {
                    JobOrigin::Model
                },
                state: match j.state {
                    pipkin_engine_protocol::JobState::Starting => JobState::Starting,
                    pipkin_engine_protocol::JobState::Running => JobState::Running,
                    pipkin_engine_protocol::JobState::Cancelling => JobState::Cancelling,
                    pipkin_engine_protocol::JobState::Succeeded => JobState::Succeeded,
                    pipkin_engine_protocol::JobState::Failed => JobState::Failed,
                    pipkin_engine_protocol::JobState::TimedOut => JobState::TimedOut,
                    pipkin_engine_protocol::JobState::Cancelled => JobState::Cancelled,
                    pipkin_engine_protocol::JobState::Interrupted => JobState::Interrupted,
                    pipkin_engine_protocol::JobState::Unknown => JobState::Unknown,
                },
                exit_code: j.exit_code,
                reason: j.reason.clone(),
                call_id: j.call_id.clone(),
                task: j.task,
                wake: j.wake,
                delivery: j.delivery.clone(),
                log_bytes: j.log_bytes,
                log_truncated: j.log_truncated,
                created_at_ms: j.created_at_ms,
                started_at_ms: j.started_at_ms,
                ended_at_ms: j.ended_at_ms,
            })
            .collect(),
        active: view.active,
        truncated: view.truncated,
    }
}

pub fn job_log(view: &pipkin_engine_protocol::JobLogView) -> pipkin_core::JobLogTail {
    pipkin_core::JobLogTail {
        id: view.id,
        offset: view.offset,
        text: view.text.clone(),
        next_offset: view.next_offset,
        total_bytes: view.total_bytes,
        truncated: view.truncated,
        done: view.done,
    }
}

// ------------------------------------------------------------------ MCP servers

/// The engine's server view as the desktop shows it. Secrets never arrive: the engine
/// redacts literals and never sends tokens.
pub fn mcp_snapshot(view: &pipkin_engine_protocol::McpServersView) -> pipkin_core::McpSnapshot {
    use pipkin_core::{
        McpAuth, McpAuthStatus, McpKind, McpPrompt, McpScope, McpServer, McpServerState, McpTool,
    };
    use pipkin_engine_protocol::AuthStatus;
    let servers = view
        .servers
        .iter()
        .map(|s| McpServer {
            name: s.name.clone(),
            scope: if s.scope == "project" {
                McpScope::Project
            } else {
                McpScope::User
            },
            kind: if s.kind == "http" {
                McpKind::Http
            } else {
                McpKind::Stdio
            },
            enabled: s.enabled,
            state: match s.state.as_str() {
                "connecting" => McpServerState::Connecting,
                "ready" => McpServerState::Ready,
                "authRequired" => McpServerState::AuthRequired,
                "failed" => McpServerState::Failed,
                _ => McpServerState::Stopped,
            },
            error: s.error.clone(),
            tools: s.tools,
            catalog: s
                .catalog
                .iter()
                .map(|t| McpTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    enabled: t.enabled,
                })
                .collect(),
            disabled_tools: s.disabled_tools.clone(),
            resources: s.resources,
            prompts: s.prompts,
            capabilities: s.capabilities.clone(),
            command: s.command.clone(),
            env: s.env.clone(),
            keep_alive: s.keep_alive,
            idle_seconds: s.idle_seconds,
            in_flight: s.in_flight,
            auth: McpAuth {
                supported: s.auth.supported,
                signed_in: s.auth.signed_in,
                required: s.auth.required,
                status: match s.auth.status {
                    AuthStatus::Idle => McpAuthStatus::Idle,
                    AuthStatus::Connecting => McpAuthStatus::Connecting,
                    AuthStatus::Waiting => McpAuthStatus::Waiting,
                    AuthStatus::Prompt => McpAuthStatus::Prompt,
                    AuthStatus::Done => McpAuthStatus::Done,
                    AuthStatus::Failed => McpAuthStatus::Failed,
                },
                message: s.auth.message.clone(),
                url: s.auth.url.clone(),
                prompt: s.auth.prompt.as_ref().map(|p| McpPrompt {
                    id: p.id.clone(),
                    kind: p.kind.clone(),
                    message: p.message.clone(),
                    placeholder: p.placeholder.clone(),
                }),
            },
        })
        .collect();
    pipkin_core::McpSnapshot {
        servers,
        warnings: view.warnings.clone(),
        project_available: view.project_available,
    }
}

pub fn mcp_test(view: &pipkin_engine_protocol::McpTestView) -> pipkin_core::McpTestResult {
    pipkin_core::McpTestResult {
        name: view.name.clone(),
        ok: view.ok,
        message: view.message.clone(),
        tools: view.tools.clone(),
    }
}

pub fn mcp_import(
    text: &str,
    view: &pipkin_engine_protocol::McpImportView,
) -> pipkin_core::McpImportPreview {
    pipkin_core::McpImportPreview {
        text: text.to_owned(),
        entries: view
            .entries
            .iter()
            .map(|e| pipkin_core::McpImportEntry {
                name: e.name.clone(),
                kind: e.kind.clone(),
                summary: e
                    .spec
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        let command = e.spec.get("command")?.as_str()?;
                        let args: Vec<&str> = e
                            .spec
                            .get("args")
                            .and_then(|a| a.as_array())
                            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        Some(
                            std::iter::once(command)
                                .chain(args)
                                .collect::<Vec<_>>()
                                .join(" "),
                        )
                    })
                    .unwrap_or_default(),
                warnings: e.warnings.clone(),
                exists: e.exists,
                valid: e.valid,
                selected: e.valid,
            })
            .collect(),
        warnings: view.warnings.clone(),
    }
}

pub fn mcp_scope(scope: pipkin_core::McpScope) -> pipkin_engine_protocol::McpScope {
    match scope {
        pipkin_core::McpScope::User => pipkin_engine_protocol::McpScope::User,
        pipkin_core::McpScope::Project => pipkin_engine_protocol::McpScope::Project,
    }
}

/// A command line into its words: whitespace-separated, with double or single quotes
/// grouping words (no escapes).
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for ch in line.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                in_word = true;
            }
            None if ch.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                current.push(ch);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

/// The add form's input as the engine's settings entry.
pub fn mcp_spec(input: &pipkin_core::McpServerInput) -> serde_json::Value {
    let mut spec = serde_json::Map::new();
    let values: serde_json::Map<String, serde_json::Value> = input
        .values
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    match input.kind {
        pipkin_core::McpKind::Stdio => {
            let words = split_command_line(input.target.trim());
            spec.insert(
                "command".into(),
                serde_json::Value::String(words.first().cloned().unwrap_or_default()),
            );
            if words.len() > 1 {
                spec.insert("args".into(), serde_json::Value::from(words[1..].to_vec()));
            }
            if !values.is_empty() {
                spec.insert("env".into(), serde_json::Value::Object(values));
            }
        }
        pipkin_core::McpKind::Http => {
            spec.insert(
                "url".into(),
                serde_json::Value::String(input.target.trim().to_owned()),
            );
            if !values.is_empty() {
                spec.insert("headers".into(), serde_json::Value::Object(values));
            }
        }
    }
    serde_json::Value::Object(spec)
}

#[cfg(test)]
mod mcp_tests {
    use super::*;

    #[test]
    fn command_lines_split_and_inputs_become_specs() {
        assert_eq!(
            split_command_line(r#"npx -y "my server" 'a b'  c"#),
            ["npx", "-y", "my server", "a b", "c"]
        );
        assert_eq!(split_command_line("  "), Vec::<String>::new());
        let input = pipkin_core::McpServerInput {
            scope: pipkin_core::McpScope::User,
            name: "fs".into(),
            kind: pipkin_core::McpKind::Stdio,
            target: "npx -y srv".into(),
            values: vec![("T".into(), "$TOK".into())],
        };
        assert_eq!(
            mcp_spec(&input),
            serde_json::json!({"command": "npx", "args": ["-y", "srv"], "env": {"T": "$TOK"}})
        );
        let http = pipkin_core::McpServerInput {
            kind: pipkin_core::McpKind::Http,
            target: " https://h/mcp ".into(),
            values: vec![],
            ..input
        };
        assert_eq!(mcp_spec(&http), serde_json::json!({"url": "https://h/mcp"}));
        assert_eq!(
            pipkin_core::McpServerInput::parse_values("A=1\n\nbad\n B = $X "),
            vec![("A".into(), "1".into()), ("B".into(), "$X".into())]
        );
    }
}

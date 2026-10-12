//! Pi sessions as Pipkin conversations: stable local ids, titles, and the catalogue built from
//! the replicated `pi.session-directory` state. A session's working directory (reported by
//! current servers) is its project, so the project list is the set of directories the user's
//! sessions run in. Titles are still derived (Pi has no session titles yet).

use std::collections::{HashMap, HashSet};

use pipkin_core::{
    Bootstrap, ConversationId, ModelInfo, Project, ProjectId, UiNotice, UiNoticeLevel, UiRequest,
    UiRequestItem, UiRequestKind, project_id_for_path, project_name_for_path, stable_id,
};
use serde_json::Value;

/// The project of a session whose server does not report a working directory.
pub const SESSIONS_PROJECT: ProjectId = ProjectId(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    /// Unix seconds.
    pub created_at: i64,
    /// The directory the session's agent works in. Absent from servers that do not report it.
    pub cwd: Option<String>,
}

/// Assign each session a stable `ConversationId` derived from its id. A collision (two live
/// sessions hashing alike) is resolved by probing in a deterministic order, so the same set of
/// sessions always gets the same ids.
pub fn assign_ids_with(
    sessions: &[Session],
    hash: impl Fn(&str) -> u64,
) -> Vec<(ConversationId, Session)> {
    let mut ordered: Vec<&Session> = sessions.iter().collect();
    ordered.sort_by(|a, b| (a.created_at, &a.session_id).cmp(&(b.created_at, &b.session_id)));
    let mut taken = HashSet::new();
    let mut out = Vec::new();
    for session in ordered {
        let mut id = hash(&session.session_id);
        while !taken.insert(id) {
            id = id.wrapping_add(1).max(1);
        }
        out.push((ConversationId(id), session.clone()));
    }
    out
}

pub fn assign_ids(sessions: &[Session]) -> Vec<(ConversationId, Session)> {
    assign_ids_with(sessions, stable_id)
}

/// Parse the directory state value, ignoring malformed rows rather than failing the catalogue.
pub fn parse_directory(state: &Value) -> Vec<Session> {
    state
        .get("sessions")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let session_id = row.get("sessionId")?.as_str().filter(|s| !s.is_empty())?;
                    let created = row.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
                    // Millisecond timestamps are far above any plausible second count.
                    let created_at = if created > 100_000_000_000 {
                        created / 1000
                    } else {
                        created
                    };
                    let cwd = row
                        .get("cwd")
                        .and_then(Value::as_str)
                        .filter(|c| c.starts_with('/'))
                        .map(str::to_owned);
                    Some(Session {
                        session_id: session_id.to_owned(),
                        created_at,
                        cwd,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `YYYY-MM-DD HH:MM` in UTC, without a date library.
pub fn format_time(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let secs = unix_seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

pub fn title(session: &Session) -> String {
    let short: String = session.session_id.chars().take(8).collect();
    format!("Session {short} \u{b7} {}", format_time(session.created_at))
}

/// The project a session belongs to: its working directory, or a shared fallback for servers
/// that do not report one.
fn project_of(session: &Session) -> ProjectId {
    session
        .cwd
        .as_deref()
        .map_or(SESSIONS_PROJECT, project_id_for_path)
}

/// The catalogue the core consumes: one project per distinct working directory (plus the
/// fallback when some session has none), conversations in the given order.
pub fn catalog(
    sessions: &[(ConversationId, Session)],
    models: Vec<ModelInfo>,
    location: &str,
    now: i64,
) -> Bootstrap {
    let mut projects: Vec<Project> = Vec::new();
    for (_, session) in sessions {
        let id = project_of(session);
        if projects.iter().any(|p| p.id == id) {
            continue;
        }
        projects.push(match &session.cwd {
            Some(cwd) => Project {
                id,
                name: project_name_for_path(cwd),
                path: cwd.clone(),
            },
            None => Project {
                id,
                name: "Pi sessions".into(),
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
            .map(|(id, s)| (*id, project_of(s), title(s), s.created_at))
            .collect(),
        now,
    }
}

/// The model the engine reports as selected, as `provider/modelId`.
pub fn selected_model(state: &Value) -> Option<String> {
    let model = state
        .get("configuration")
        .and_then(|c| c.get("model"))
        .filter(|m| m.is_object())?;
    Some(format!(
        "{}/{}",
        model.get("provider")?.as_str()?,
        model.get("modelId")?.as_str()?
    ))
}

pub fn selected_thinking(state: &Value) -> Option<String> {
    state
        .get("configuration")?
        .get("thinkingLevel")?
        .as_str()
        .map(str::to_owned)
}

/// Split a `provider/modelId` id at the first slash (model ids may themselves contain slashes).
pub fn split_model_id(id: &str) -> Option<(&str, &str)> {
    let (provider, model) = id.split_once('/')?;
    (!provider.is_empty() && !model.is_empty()).then_some((provider, model))
}

/// The questions extensions are waiting on, their status lines and notices, from `pi.ui-requests`
/// state. A question of a kind this build cannot show becomes a warning notice, so it is said
/// that something was asked rather than the extension appearing to hang.
pub fn parse_ui_state(state: &Value) -> (Vec<UiRequest>, Vec<(String, String)>, Vec<UiNotice>) {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let mut requests = Vec::new();
    let mut notices = Vec::new();
    for r in state
        .get("requests")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(title)) = (text(r, "id"), text(r, "title")) else {
            continue;
        };
        let kind = match r.get("kind").and_then(Value::as_str) {
            Some("select") => UiRequestKind::Select,
            Some("confirm") => UiRequestKind::Confirm,
            Some("input") => UiRequestKind::Input,
            other => {
                notices.push(UiNotice {
                    id: format!("unsupported-{id}"),
                    level: UiNoticeLevel::Warning,
                    message: format!(
                        "An extension asked \"{title}\" ({}), which this version of Pipkin cannot show. It will time out.",
                        other.unwrap_or("unknown kind")
                    ),
                });
                continue;
            }
        };
        requests.push(UiRequest {
            id,
            kind,
            title,
            message: text(r, "message"),
            items: r
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|i| {
                    Some(UiRequestItem {
                        value: text(i, "value")?,
                        label: text(i, "label")?,
                        description: text(i, "description"),
                    })
                })
                .collect(),
            placeholder: text(r, "placeholder"),
            default_value: text(r, "defaultValue"),
            deadline: r.get("deadline").and_then(Value::as_i64),
        });
    }
    let mut status: Vec<(String, String)> = state
        .get("status")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
        .collect();
    status.sort();
    notices.extend(
        state
            .get("notices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|n| {
                Some(UiNotice {
                    id: text(n, "id")?,
                    level: match n.get("level").and_then(Value::as_str) {
                        Some("error") => UiNoticeLevel::Error,
                        Some("warning") => UiNoticeLevel::Warning,
                        _ => UiNoticeLevel::Info,
                    },
                    message: text(n, "message")?,
                })
            }),
    );
    (requests, status, notices)
}

/// What the engine said went wrong the last time it re-read its providers (expired login,
/// unreachable provider, a bad key), as one line. `None` when the refresh was clean.
pub fn refresh_warning(state: &Value) -> Option<String> {
    let refresh = state.get("refresh")?;
    if refresh.get("status").and_then(Value::as_str) != Some("warning") {
        return None;
    }
    let mut problems: Vec<String> = refresh
        .get("errors")
        .and_then(Value::as_object)?
        .iter()
        .map(|(provider, message)| format!("{provider}: {}", message.as_str().unwrap_or("failed")))
        .collect();
    problems.sort();
    Some(format!(
        "Some providers could not be read ({}). Check their sign-in or API key with Pi, then refresh the models.",
        problems.join("; ")
    ))
}

/// Map `pi.models` state. The configured model comes first so it is the default selection.
pub fn parse_models(state: &Value) -> Vec<ModelInfo> {
    let configured = selected_model(state);
    let mut models: Vec<ModelInfo> = state
        .get("catalog")
        .and_then(|c| c.get("availableModels"))
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let provider = row.get("provider")?.as_str()?;
                    let model_id = row.get("modelId")?.as_str()?;
                    let name = row.get("name").and_then(Value::as_str).unwrap_or(model_id);
                    let reasoning = row
                        .get("reasoning")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some(ModelInfo {
                        id: format!("{provider}/{model_id}"),
                        name: name.to_owned(),
                        note: if reasoning {
                            format!("{provider} \u{b7} reasoning")
                        } else {
                            provider.to_owned()
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(configured) = configured
        && let Some(at) = models.iter().position(|m| m.id == configured)
    {
        let model = models.remove(at);
        models.insert(0, model);
    }
    models
}

/// Reverse index from local conversation id to the session.
pub fn index(sessions: &[(ConversationId, Session)]) -> HashMap<ConversationId, Session> {
    sessions.iter().map(|(id, s)| (*id, s.clone())).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn s(id: &str, at: i64) -> Session {
        Session {
            session_id: id.into(),
            created_at: at,
            cwd: None,
        }
    }

    fn in_dir(id: &str, at: i64, cwd: &str) -> Session {
        Session {
            cwd: Some(cwd.into()),
            ..s(id, at)
        }
    }

    #[test]
    fn ids_are_stable_for_a_session_regardless_of_the_others() {
        let a = assign_ids(&[s("alpha", 1)]);
        let b = assign_ids(&[s("zeta", 0), s("alpha", 1), s("mid", 5)]);
        let id_of = |v: &[(ConversationId, Session)], name: &str| {
            v.iter().find(|(_, x)| x.session_id == name).unwrap().0
        };
        assert_eq!(id_of(&a, "alpha"), id_of(&b, "alpha"));
        assert!(b.iter().all(|(id, _)| id.0 > 0 && id.0 < (1 << 52)));
    }

    #[test]
    fn collisions_probe_deterministically() {
        let constant = |_: &str| 7u64;
        let first = assign_ids_with(&[s("b", 2), s("a", 1), s("c", 3)], constant);
        let ids: Vec<u64> = first.iter().map(|(i, _)| i.0).collect();
        assert_eq!(ids, [7, 8, 9]);
        // The oldest session keeps the base id however the input is ordered.
        let again = assign_ids_with(&[s("c", 3), s("a", 1), s("b", 2)], constant);
        assert_eq!(first, again);
        assert_eq!(first[0].1.session_id, "a");
    }

    #[test]
    fn parses_directory_rows_and_ignores_malformed_ones() {
        let state = json!({ "revision": 2, "sessions": [
            { "serverId": "x", "sessionId": "one", "createdAt": 1_700_000_000_000i64, "cwd": "/work/app" },
            { "sessionId": "two", "createdAt": 1_700_000_001 },
            { "sessionId": "three", "createdAt": 5, "cwd": "relative/dir" },
            { "sessionId": "", "createdAt": 1 },
            { "createdAt": 1 },
            7,
        ]});
        let rows = parse_directory(&state);
        assert_eq!(
            rows,
            vec![
                in_dir("one", 1_700_000_000, "/work/app"),
                s("two", 1_700_000_001),
                s("three", 5), // a relative cwd is not trusted
            ]
        );
        assert!(parse_directory(&json!(null)).is_empty());
        assert!(parse_directory(&json!({ "sessions": "no" })).is_empty());
    }

    #[test]
    fn formats_utc_times() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00"); // leap day
        assert_eq!(format_time(-1), "1969-12-31 23:59");
    }

    #[test]
    fn sessions_group_into_projects_by_working_directory() {
        let sessions = assign_ids(&[
            in_dir("a1", 1, "/work/app"),
            in_dir("a2", 2, "/work/app"),
            in_dir("b1", 3, "/work/zeta"),
        ]);
        let boot = catalog(&sessions, vec![], "/home/u/.pi/server", 5);
        let names: Vec<(&str, &str)> = boot
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.path.as_str()))
            .collect();
        assert_eq!(names, [("app", "/work/app"), ("zeta", "/work/zeta")]);
        let app = project_id_for_path("/work/app");
        assert_eq!(boot.conversations.iter().filter(|c| c.1 == app).count(), 2);
    }

    #[test]
    fn a_session_without_a_directory_goes_to_the_fallback_project() {
        let sessions = assign_ids(&[s("0123456789abcdef", 1_700_000_000)]);
        let boot = catalog(&sessions, vec![], "/home/u/.pi/server", 5);
        assert_eq!(boot.projects.len(), 1);
        assert_eq!(boot.projects[0].id, SESSIONS_PROJECT);
        assert_eq!(boot.projects[0].path, "/home/u/.pi/server");
        assert_eq!(
            boot.conversations[0].2,
            "Session 01234567 \u{b7} 2023-11-14 22:13"
        );
        assert_eq!(
            index(&sessions).get(&sessions[0].0).unwrap().session_id,
            "0123456789abcdef"
        );
    }

    #[test]
    fn models_put_the_configured_one_first_and_the_selection_is_reported() {
        let state = json!({
            "catalog": { "revision": 1, "availableModels": [
                { "provider": "anthropic", "modelId": "a", "name": "A", "reasoning": false },
                { "provider": "openai", "modelId": "b", "name": "B", "reasoning": true },
                { "provider": "x" },
            ]},
            "configuration": { "model": { "provider": "openai", "modelId": "b" }, "thinkingLevel": "off" },
        });
        let models = parse_models(&state);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["openai/b", "anthropic/a"]
        );
        assert!(models[0].note.contains("reasoning"));
        assert_eq!(selected_model(&state).as_deref(), Some("openai/b"));
        assert!(parse_models(&json!({})).is_empty());
        // No model configured: no selection to report.
        let none = json!({ "configuration": { "model": null } });
        assert_eq!(selected_model(&none), None);
        // A configured model absent from the catalogue changes nothing about the order.
        let absent = json!({ "catalog": { "availableModels": [{ "provider": "p", "modelId": "m", "name": "M" }] },
            "configuration": { "model": { "provider": "q", "modelId": "z" } } });
        assert_eq!(parse_models(&absent).len(), 1);
    }

    #[test]
    fn model_ids_split_at_the_first_slash() {
        assert_eq!(split_model_id("openai/gpt"), Some(("openai", "gpt")));
        assert_eq!(
            split_model_id("openrouter/anthropic/claude"),
            Some(("openrouter", "anthropic/claude"))
        );
        assert_eq!(split_model_id("noslash"), None);
        assert_eq!(split_model_id("/x"), None);
        assert_eq!(split_model_id("x/"), None);
    }
}

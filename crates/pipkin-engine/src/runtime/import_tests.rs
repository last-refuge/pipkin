//! The import calls through the client contract, against the real reference fixtures in
//! `fixtures/pi-sessions/` (copied into a temporary agent directory first).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::{Value, json};

use super::super::tests::{Engine, Fixture};
use super::{RESERVED_KEPT, agent_dir, utc_stamp};
use crate::config::Env;
use crate::provider::scripted::Scripted;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pi-sessions")
}

/// Fixture names (each a reference session directory with `meta.json` and `session.sqlite`).
fn fixture_ids() -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(fixtures_root())
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.path().join("meta.json").is_file())
                .filter(|e| e.path().join("session.sqlite").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// `<root>/agent/experimental/sessions/<id>/{meta.json,session.sqlite}` for each fixture.
fn agent_with(root: &Path, ids: &[String]) -> PathBuf {
    let agent = root.join("agent");
    for id in ids {
        let dest = agent.join("experimental/sessions").join(id);
        std::fs::create_dir_all(&dest).unwrap();
        for file in ["meta.json", "session.sqlite", "session.sqlite-wal"] {
            let source = fixtures_root().join(id).join(file);
            if source.is_file() {
                std::fs::copy(&source, dest.join(file)).unwrap();
            }
        }
    }
    std::fs::create_dir_all(agent.join("experimental/sessions")).unwrap();
    agent
}

fn report(e: &mut Engine, agent: &Path) -> ImportReport {
    match e.call(Call::ImportDiscover {
        agent_dir: Some(agent.display().to_string()),
    }) {
        Ok(Reply::ImportReport(r)) => r,
        other => panic!("{other:?}"),
    }
}

fn import(e: &mut Engine, agent: &Path, ids: &[String]) -> ImportedView {
    match e.call(Call::ImportSessions {
        agent_dir: Some(agent.display().to_string()),
        sessions: ids.to_vec(),
    }) {
        Ok(Reply::Imported(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn outcome<'a>(view: &'a ImportedView, id: &str) -> &'a ImportSessionOutcome {
    &view
        .sessions
        .iter()
        .find(|s| s.source == id)
        .unwrap_or_else(|| panic!("no result for {id}"))
        .outcome
}

/// (role, text, tool names) of a native message, as the importer's own fixture test projects.
fn project_native(m: &Message) -> (String, String, Vec<String>) {
    let m = serde_json::to_value(m).unwrap();
    let role = match m["role"].as_str().unwrap_or("") {
        "toolResult" => "tool",
        other => other,
    };
    let blocks = m["content"].as_array().cloned().unwrap_or_default();
    let text = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let tools = blocks
        .iter()
        .filter(|b| b["type"] == "toolCall")
        .map(|b| b["name"].as_str().unwrap_or("").to_owned())
        .collect();
    (role.to_owned(), text, tools)
}

/// The same projection of an OpenAI chat-completions request message.
fn project_request(m: &Value) -> (String, String, Vec<String>) {
    let text = match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect(),
        _ => String::new(),
    };
    let tools = m["tool_calls"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|t| t["function"]["name"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default();
    (m["role"].as_str().unwrap_or("").to_owned(), text, tools)
}

/// The same projection of a reference history entry's model message (`history.json`).
fn project_history(m: &Value) -> (String, String) {
    let role = match m["role"].as_str().unwrap_or("") {
        "toolResult" => "tool",
        other => other,
    };
    let text = match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect(),
        _ => String::new(),
    };
    (role.to_owned(), text)
}

type Projected = (String, String, Vec<String>);

/// The reference's newest tool-carrying request whose first message is `first` (the root
/// conversation's opening prompt), without its system/developer message.
fn last_reference_request(id: &str, first: &Projected) -> Option<Vec<Projected>> {
    let path = fixtures_root().join(id).join("requests.jsonl");
    let text = std::fs::read_to_string(path).ok()?;
    let mut last = None;
    for line in text.lines() {
        let request: Value = serde_json::from_str(line).unwrap();
        if request["body"]["tools"].is_null() {
            continue; // a summarization request
        }
        let messages: Vec<Projected> = request["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] != "system" && m["role"] != "developer")
            .map(project_request)
            .collect();
        if messages.first() == Some(first) {
            last = Some(messages);
        }
    }
    last
}

/// Every message of a session's history, oldest first: the snapshot window plus older pages.
fn full_history(e: &mut Engine, session: &str) -> Vec<Entry> {
    let snapshot = e.snapshot(session);
    let mut entries = snapshot.entries;
    let mut has_older = snapshot.has_older;
    while has_older {
        let before = entries.first().map(|x| x.id);
        match e.call(Call::HistoryPage {
            session: session.into(),
            before,
            limit: 200,
        }) {
            Ok(Reply::Page {
                entries: older,
                has_older: more,
            }) => {
                assert!(!older.is_empty(), "a page with older history is not empty");
                entries.splice(0..0, older);
                has_older = more;
            }
            other => panic!("{other:?}"),
        }
    }
    entries
}

/// Import every reference fixture, open each imported conversation, check its transcript is
/// complete, then prompt it natively: the next provider request must start with exactly the
/// messages the reference sent last, followed by the reference's answer and the new prompt.
#[test]
fn reference_fixtures_import_open_and_continue_equivalently() {
    let ids = fixture_ids();
    if ids.is_empty() {
        eprintln!("no reference fixtures at {}", fixtures_root().display());
        return;
    }
    let fx = Fixture::new();
    let agent = agent_with(fx.dir.path(), &ids);
    let provider = Scripted::echo("Continued natively.");
    let mut e = fx.start(&provider);

    let found = report(&mut e, &agent);
    assert_eq!(found.agent_dir, agent.display().to_string());
    for id in &ids {
        let s = found.sessions.iter().find(|s| &s.id == id).unwrap();
        assert_eq!(s.status, ImportSessionStatus::Importable, "{id}");
    }

    let sources: Vec<Vec<u8>> = ids
        .iter()
        .map(|id| {
            std::fs::read(
                agent
                    .join("experimental/sessions")
                    .join(id)
                    .join("session.sqlite"),
            )
            .unwrap()
        })
        .collect();
    e.events.clear();
    let view = import(&mut e, &agent, &ids);
    for id in &ids {
        assert!(
            matches!(outcome(&view, id), ImportSessionOutcome::Imported { native, entries, .. } if native == id && *entries > 0),
            "{id}: {:?}",
            outcome(&view, id)
        );
    }
    assert!(matches!(
        outcome(&view, "f-subagent"),
        ImportSessionOutcome::Imported { children: 1, .. }
    ));
    let backup = PathBuf::from(view.backup_dir.clone().expect("copies were kept"));
    assert!(backup.starts_with(fx.dir.path().join("data/imports")));
    for id in &ids {
        assert!(backup.join(id).join("session.sqlite").is_file(), "{id}");
    }
    e.until("the catalog changed", |ev| {
        ev.iter().any(|x| matches!(x, Event::SessionsChanged))
    });
    // The legacy sources are untouched.
    for (id, before) in ids.iter().zip(&sources) {
        let after = std::fs::read(
            agent
                .join("experimental/sessions")
                .join(id)
                .join("session.sqlite"),
        )
        .unwrap();
        assert!(after == *before, "{id}: the source changed");
    }
    let Ok(Reply::Sessions { sessions }) = e.call(Call::ListSessions) else {
        panic!()
    };
    for id in &ids {
        let s = sessions.iter().find(|s| &s.id == id).expect("listed");
        assert_eq!(
            s.origin,
            Origin::Imported {
                engine: "pi".into(),
                source: id.clone()
            }
        );
    }

    let mut report_lines = Vec::new();
    for (n, id) in ids.iter().enumerate() {
        // The transcript is complete: every reference message, in order.
        let history_file = fixtures_root().join(id).join("history.json");
        let reference: Vec<(String, String)> =
            serde_json::from_str::<Value>(&std::fs::read_to_string(history_file).unwrap())
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .filter(|x| {
                    matches!(
                        x["kind"].as_str(),
                        Some("pi.user" | "pi.assistant" | "pi.tool-result")
                    ) && x["conversationId"] == json!(1)
                })
                .flat_map(|x| x["model"].as_array().cloned().unwrap_or_default())
                .map(|m| project_history(&m))
                .collect();
        let native: Vec<(String, String)> = full_history(&mut e, id)
            .iter()
            .filter_map(|x| match &x.kind {
                EntryKind::Message { message, .. } => {
                    let (role, text, _) = project_native(message);
                    Some((role, text))
                }
                _ => None,
            })
            .collect();
        assert_eq!(native, reference, "{id}: the opened transcript is complete");

        // Continue natively with the scripted model.
        e.call(Call::SetModel {
            session: id.clone(),
            model: "scripted/scripted".into(),
        })
        .unwrap();
        let key = format!("continue-{n}");
        let Ok(Reply::Admitted { run: Some(_), .. }) = e.prompt(id, &key, "Please continue.")
        else {
            panic!("{id}: the prompt was not admitted")
        };
        assert_eq!(e.settled(&key), RunOutcome::Completed, "{id}");
        let requests = provider.requests();
        let sent: Vec<Projected> = requests
            .last()
            .unwrap()
            .messages
            .iter()
            .map(project_native)
            .collect();
        assert_eq!(
            sent.last().unwrap(),
            &("user".to_owned(), "Please continue.".to_owned(), vec![]),
            "{id}"
        );
        let reference = last_reference_request(id, &sent[0])
            .unwrap_or_else(|| panic!("{id}: no reference request starts like this context"));
        assert!(
            sent.len() > reference.len() + 1 && sent[..reference.len()] == reference[..],
            "{id}: the native request {sent:#?} does not continue the reference's last request {reference:#?}"
        );
        report_lines.push(format!(
            "{id}: continues the reference's last request ({} messages) with {} later message(s) and the new prompt",
            reference.len(),
            sent.len() - reference.len() - 1
        ));
        let snapshot = e.snapshot(id);
        let last = snapshot.entries.last().unwrap();
        assert!(
            matches!(
                &last.kind,
                EntryKind::Message {
                    message: Message::Assistant { .. },
                    ..
                }
            ),
            "{id}"
        );
    }
    for line in report_lines {
        eprintln!("{line}");
    }
}

#[test]
fn a_rerun_is_a_no_op_that_returns_the_mapping() {
    let ids = fixture_ids();
    let Some(id) = ids.first().cloned() else {
        return;
    };
    let fx = Fixture::new();
    let agent = agent_with(fx.dir.path(), std::slice::from_ref(&id));
    let mut e = fx.start(&Scripted::echo("x"));
    let first = import(&mut e, &agent, &[id.clone(), id.clone()]);
    assert_eq!(first.sessions.len(), 1, "duplicates are imported once");
    assert!(matches!(
        outcome(&first, &id),
        ImportSessionOutcome::Imported { .. }
    ));
    let imports = fx.dir.path().join("data/imports");
    let backups = std::fs::read_dir(&imports).unwrap().count();
    std::thread::sleep(Duration::from_millis(5));
    let again = import(&mut e, &agent, std::slice::from_ref(&id));
    assert_eq!(
        outcome(&again, &id),
        &ImportSessionOutcome::AlreadyImported { native: id.clone() }
    );
    assert_eq!(again.backup_dir, None, "nothing was copied");
    assert_eq!(std::fs::read_dir(&imports).unwrap().count(), backups);
    let found = report(&mut e, &agent);
    assert_eq!(
        found.sessions[0].status,
        ImportSessionStatus::AlreadyImported { native: id.clone() }
    );
    let Ok(Reply::Sessions { sessions }) = e.call(Call::ListSessions) else {
        panic!()
    };
    assert_eq!(sessions.len(), 1);
}

#[test]
fn locked_blocked_and_unknown_sessions_are_reported_and_nothing_is_written() {
    let ids = fixture_ids();
    let Some(id) = ids.first().cloned() else {
        return;
    };
    let fx = Fixture::new();
    let agent = agent_with(fx.dir.path(), std::slice::from_ref(&id));
    let sessions_dir = agent.join("experimental/sessions");
    // A live writer's fresh lock directory.
    std::fs::create_dir(sessions_dir.join(format!("{id}.lock"))).unwrap();
    // A session that was never opened.
    let empty = sessions_dir.join("never-opened");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(empty.join("meta.json"), r#"{"createdAt": 1, "cwd": "/x"}"#).unwrap();

    let mut e = fx.start(&Scripted::echo("x"));
    let found = report(&mut e, &agent);
    let status = |name: &str| {
        found
            .sessions
            .iter()
            .find(|s| s.id == name)
            .unwrap()
            .status
            .clone()
    };
    assert!(matches!(
        status(&id),
        ImportSessionStatus::Locked { detail: Some(_) }
    ));
    assert!(
        matches!(status("never-opened"), ImportSessionStatus::Blocked { reasons } if reasons[0].contains("never opened"))
    );
    assert_eq!(
        found.sessions.len(),
        2,
        "the lock directory is not a session"
    );

    e.events.clear();
    let view = import(
        &mut e,
        &agent,
        &[
            id.clone(),
            "never-opened".into(),
            "missing".into(),
            "../etc".into(),
        ],
    );
    assert!(matches!(
        outcome(&view, &id),
        ImportSessionOutcome::Locked { .. }
    ));
    assert!(matches!(
        outcome(&view, "never-opened"),
        ImportSessionOutcome::Blocked { .. }
    ));
    assert!(
        matches!(outcome(&view, "missing"), ImportSessionOutcome::Failed { detail } if detail.contains("not found"))
    );
    assert!(
        matches!(outcome(&view, "../etc"), ImportSessionOutcome::Failed { detail } if detail.contains("not a session id"))
    );
    assert_eq!(view.backup_dir, None);
    let Ok(Reply::Sessions { sessions }) = e.call(Call::ListSessions) else {
        panic!()
    };
    assert!(sessions.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    e.call(Call::ListSessions).unwrap();
    assert!(
        !e.events.iter().any(|x| matches!(x, Event::SessionsChanged)),
        "nothing changed"
    );

    // Once the writer is gone (its lock went stale), the same call imports it.
    std::fs::remove_dir(sessions_dir.join(format!("{id}.lock"))).unwrap();
    let view = import(&mut e, &agent, std::slice::from_ref(&id));
    assert!(matches!(
        outcome(&view, &id),
        ImportSessionOutcome::Imported { .. }
    ));
}

const NATIVE_AUTH: &str = "{\n  \"groq\": {\"type\":\"api_key\",   \"key\": \"gsk-NATIVE\"},\n  \"custom\": {\"type\": \"bespoke\", \"x\": [1,2,3]}\n}";

#[test]
fn credentials_merge_without_overwriting_and_keep_reserved_ones_where_they_are() {
    let fx = Fixture::new();
    let agent = agent_with(fx.dir.path(), &[]);
    let pi_auth = r#"{
  "openrouter": {
    "type": "api_key",
    "key": "sk-or-SECRET"
  },
  "openai-codex": {
    "type": "oauth",
    "refresh": "r-SECRET",
    "access": "tok-SECRET",
    "expires": 1700000000000,
    "accountId": "acct"
  },
  "anthropic": {
    "type": "oauth",
    "refresh": "r",
    "access": "sub-SECRET",
    "expires": 1
  },
  "groq": {
    "type": "api_key",
    "key": "gsk-PI-SECRET"
  }
}"#;
    std::fs::write(agent.join("auth.json"), pi_auth).unwrap();
    let config = fx.dir.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("auth.json"), NATIVE_AUTH).unwrap();

    let mut e = fx.start(&Scripted::echo("x"));
    let found = report(&mut e, &agent);
    assert!(!format!("{found:?}").contains("SECRET"));
    assert!(!serde_json::to_string(&found).unwrap().contains("SECRET"));
    let cred = |p: &str| found.credentials.iter().find(|c| c.provider == p).unwrap();
    assert_eq!(cred("anthropic").reserved.as_deref(), Some(RESERVED_KEPT));
    assert!(cred("groq").native_exists);
    assert!(!cred("openrouter").native_exists);
    assert_eq!(cred("openai-codex").kind, "oauth");

    e.events.clear();
    let reply = e
        .call(Call::ImportCredentials {
            agent_dir: Some(agent.display().to_string()),
            providers: vec![
                "openrouter".into(),
                "openai-codex".into(),
                "anthropic".into(),
                "groq".into(),
                "missing".into(),
                "openrouter".into(),
            ],
        })
        .unwrap();
    assert!(!serde_json::to_string(&reply).unwrap().contains("SECRET"));
    let Reply::CredentialsImported(view) = reply else {
        panic!()
    };
    let got: HashMap<&str, &CredentialImportOutcome> = view
        .providers
        .iter()
        .map(|p| (p.provider.as_str(), &p.outcome))
        .collect();
    assert_eq!(view.providers.len(), 5);
    assert_eq!(got["openrouter"], &CredentialImportOutcome::Imported);
    assert_eq!(got["openai-codex"], &CredentialImportOutcome::Imported);
    assert_eq!(
        got["anthropic"],
        &CredentialImportOutcome::Reserved {
            reason: RESERVED_KEPT.into()
        }
    );
    assert_eq!(got["groq"], &CredentialImportOutcome::KeptExisting);
    assert_eq!(got["missing"], &CredentialImportOutcome::NotFound);
    e.until("the models were refreshed", |ev| {
        ev.iter().any(|x| matches!(x, Event::ModelsChanged))
    });

    let saved = std::fs::read_to_string(config.join("auth.json")).unwrap();
    // Existing native entries keep their bytes, including the one this engine does not know.
    assert!(
        saved.contains(r#""groq": {"type":"api_key",   "key": "gsk-NATIVE"}"#),
        "{saved}"
    );
    assert!(
        saved.contains(r#""custom": {"type": "bespoke", "x": [1,2,3]}"#),
        "{saved}"
    );
    let saved: Value = serde_json::from_str(&saved).unwrap();
    let source: Value = serde_json::from_str(pi_auth).unwrap();
    assert_eq!(saved["openrouter"], source["openrouter"]);
    assert_eq!(saved["openai-codex"], source["openai-codex"]);
    assert_eq!(
        serde_json::to_string_pretty(&saved["openai-codex"]).unwrap(),
        serde_json::to_string_pretty(&source["openai-codex"]).unwrap(),
        "the imported entry keeps its field order"
    );
    assert!(
        saved.get("anthropic").is_none(),
        "the reserved credential is not imported"
    );
    assert_eq!(saved["groq"]["key"], "gsk-NATIVE");
    // The Pi profile is untouched.
    assert_eq!(
        std::fs::read_to_string(agent.join("auth.json")).unwrap(),
        pi_auth
    );
    // The imported credential is usable: its provider now reports saved credentials.
    let Ok(Reply::Auth(auth)) = e.call(Call::AuthState) else {
        panic!()
    };
    let codex = auth.providers.iter().find(|p| p.id == "openai-codex");
    if let Some(codex) = codex {
        assert!(codex.has_credentials);
    }

    // Again: nothing to change.
    let Ok(Reply::CredentialsImported(again)) = e.call(Call::ImportCredentials {
        agent_dir: Some(agent.display().to_string()),
        providers: vec!["openrouter".into()],
    }) else {
        panic!()
    };
    assert_eq!(
        again.providers[0].outcome,
        CredentialImportOutcome::KeptExisting
    );

    // An unreadable Pi credential file is refused without quoting it.
    std::fs::write(agent.join("auth.json"), "\"sk-SECRET\"").unwrap();
    let err = e
        .call(Call::ImportCredentials {
            agent_dir: Some(agent.display().to_string()),
            providers: vec!["openrouter".into()],
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    assert!(!err.message.contains("SECRET"));
}

#[test]
fn configuration_is_copied_without_overwriting_native_files() {
    let fx = Fixture::new();
    let agent = agent_with(fx.dir.path(), &[]);
    std::fs::write(agent.join("models.json"), r#"{"providers": {}}"#).unwrap();
    std::fs::write(agent.join("AGENTS.md"), "# pi rules").unwrap();
    std::fs::write(
        agent.join("auth.json"),
        r#"{"x": {"type": "api_key", "key": "k"}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(agent.join("skills/review")).unwrap();
    std::fs::write(agent.join("skills/review/SKILL.md"), "review").unwrap();
    std::fs::write(agent.join("skills/notes.md"), "notes").unwrap();
    let config = fx.dir.path().join("config");
    std::fs::create_dir_all(config.join("skills")).unwrap();
    std::fs::write(config.join("AGENTS.md"), "# native rules").unwrap();
    std::fs::write(config.join("skills/notes.md"), "native notes").unwrap();

    let mut e = fx.start(&Scripted::echo("x"));
    let found = report(&mut e, &agent);
    assert_eq!(found.config, vec!["models.json", "AGENTS.md", "skills"]);
    let Ok(Reply::ConfigImported(view)) = e.call(Call::ImportConfig {
        agent_dir: Some(agent.display().to_string()),
        items: vec![
            "models.json".into(),
            "AGENTS.md".into(),
            "skills".into(),
            "auth.json".into(),
            "../etc".into(),
        ],
    }) else {
        panic!()
    };
    let got: HashMap<&str, &ConfigItemOutcome> = view
        .items
        .iter()
        .map(|i| (i.name.as_str(), &i.outcome))
        .collect();
    assert_eq!(got["models.json"], &ConfigItemOutcome::Copied { files: 1 });
    assert_eq!(got["AGENTS.md"], &ConfigItemOutcome::KeptExisting);
    assert_eq!(
        got["skills"],
        &ConfigItemOutcome::Partial { files: 1, kept: 1 }
    );
    assert_eq!(
        got["auth.json"],
        &ConfigItemOutcome::NotFound,
        "never through config"
    );
    assert_eq!(got["../etc"], &ConfigItemOutcome::NotFound);
    assert_eq!(
        std::fs::read_to_string(config.join("models.json")).unwrap(),
        r#"{"providers": {}}"#
    );
    assert_eq!(
        std::fs::read_to_string(config.join("AGENTS.md")).unwrap(),
        "# native rules"
    );
    assert_eq!(
        std::fs::read_to_string(config.join("skills/review/SKILL.md")).unwrap(),
        "review"
    );
    assert_eq!(
        std::fs::read_to_string(config.join("skills/notes.md")).unwrap(),
        "native notes"
    );
    assert!(!config.join("auth.json").exists());
}

#[test]
fn the_agent_directory_defaults_like_the_reference() {
    let env = |vars: &[(&str, &str)]| {
        Env::isolated(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    };
    let home = env(&[("HOME", "/home/u")]);
    assert_eq!(
        agent_dir(&home, None).unwrap(),
        PathBuf::from("/home/u/.pi/agent")
    );
    assert_eq!(
        agent_dir(&home, Some("/given".into())).unwrap(),
        PathBuf::from("/given")
    );
    assert_eq!(
        agent_dir(&home, Some("  ".into())).unwrap(),
        PathBuf::from("/home/u/.pi/agent")
    );
    let set = env(&[("HOME", "/home/u"), ("PI_CODING_AGENT_DIR", "~/custom")]);
    assert_eq!(
        agent_dir(&set, None).unwrap(),
        PathBuf::from("/home/u/custom")
    );
    let abs = env(&[("PI_CODING_AGENT_DIR", "/srv/agent")]);
    assert_eq!(agent_dir(&abs, None).unwrap(), PathBuf::from("/srv/agent"));
    assert_eq!(
        agent_dir(&env(&[]), None).unwrap_err().code,
        ErrorCode::Invalid
    );
    assert_eq!(
        agent_dir(&home, Some("relative".into())).unwrap_err().code,
        ErrorCode::Invalid
    );
}

#[test]
fn backup_directories_are_named_by_utc_time() {
    assert_eq!(utc_stamp(0), "19700101T000000.000Z");
    assert_eq!(utc_stamp(1_791_750_696_257), "20261011T203136.257Z");
    assert_eq!(utc_stamp(951_782_400_000), "20000229T000000.000Z");
}

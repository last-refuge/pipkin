//! MCP-3: resources, prompt templates and elicitation against the stdio fixture. Resources
//! and prompts are listed, searched and read through the model's tools with bounds and
//! cached until the server reports a change; a server's request for input becomes native
//! questions answered by the person, with decline, cancel and absence handled honestly.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::mcp_tests::{calling_from_prompt, run, server, write_settings};
use super::tests::{Engine, Fixture};
use crate::provider::scripted::Scripted;

fn open_question(e: &mut Engine, s: &str) -> QuestionView {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snap = e.snapshot(s);
        if let Some(q) = snap.ui.questions.first() {
            return q.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no question from the server; notices: {:?}",
            snap.ui
                .notices
                .iter()
                .map(|n| n.message.clone())
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait for the next question whose title differs from `previous`.
fn next_question(e: &mut Engine, s: &str, previous: &str) -> QuestionView {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snap = e.snapshot(s);
        if let Some(q) = snap.ui.questions.first()
            && q.title != previous
        {
            return q.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no question after {previous:?}: {:?}",
            snap.ui.questions
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn answer(e: &mut Engine, s: &str, q: &QuestionView, answer: Answer) {
    e.call(Call::AnswerQuestion {
        session: s.into(),
        question: q.id.clone(),
        answer,
    })
    .unwrap();
}

fn start(fx: &Fixture, provider: &Scripted) -> (Engine, String) {
    write_settings(fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let mut e = fx.start(provider);
    let s = e.create(&fx.project, "c");
    e.snapshot(&s);
    (e, s)
}

#[test]
fn resources_are_listed_searched_read_in_slices_and_refreshed_on_change() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let (mut e, s) = start(&fx, &provider);
    let (is_error, text, details) = run(&mut e, &s, "r1", r#"mcp_resources {}"#);
    assert!(!is_error, "{text}");
    assert!(text.starts_with("3 MCP resource(s)"), "{text}");
    assert!(
        text.contains("- fx fixture://readme (Read me) text/plain 40 bytes: The fixture's notes"),
        "{text}"
    );
    assert!(
        text.contains("- fx fixture://logo (logo) image/png: A tiny binary"),
        "{text}"
    );
    assert!(
        text.contains("- fx fixture://files/{name} (file) [template] text/plain"),
        "{text}"
    );
    assert_eq!(details["outcome"], "resources");
    assert_eq!(details["resources"][2]["template"], true);
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r2",
        r#"mcp_resources {"query": "LOGO", "server": "fx"}"#,
    );
    assert!(
        !is_error && text.starts_with("1 MCP resource(s) matching \"LOGO\" on fx"),
        "{text}"
    );
    // Text in slices.
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_read {"server": "fx", "uri": "fixture://readme"}"#,
    );
    assert!(!is_error, "{text}");
    assert!(
        text.contains("fixture://readme text/plain — characters 0–47 of 47"),
        "{text}"
    );
    assert!(
        text.contains("Line one of the readme.\nLine two of the readme."),
        "{text}"
    );
    assert_eq!(details["outcome"], "read");
    assert_eq!(details["more"], false);
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r4",
        r#"mcp_read {"server": "fx", "uri": "fixture://big", "offset": 10, "limit": 5}"#,
    );
    assert!(!is_error, "{text}");
    assert!(
        text.contains("characters 10–15 of 20000 (continue with offset 15)"),
        "{text}"
    );
    assert!(text.ends_with("xxxxx"), "{text}");
    assert_eq!(details["more"], true);
    // Binary by size, templates by filled URI, unknown URIs as the server's error.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r5",
        r#"mcp_read {"server": "fx", "uri": "fixture://logo"}"#,
    );
    assert!(
        !is_error
            && text.contains("fixture://logo image/png — binary content, 18 bytes (not shown)"),
        "{text}"
    );
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r6",
        r#"mcp_read {"server": "fx", "uri": "fixture://files/notes.txt"}"#,
    );
    assert!(
        !is_error && text.contains("contents of notes.txt"),
        "{text}"
    );
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r7",
        r#"mcp_read {"server": "fx", "uri": "fixture://nope"}"#,
    );
    assert!(
        is_error && text.contains("fx could not read fixture://nope"),
        "{text}"
    );
    assert_eq!(details["kind"], "invalid_arguments");
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r8",
        r#"mcp_read {"server": "nope", "uri": "x"}"#,
    );
    assert!(is_error, "{text}");
    assert_eq!(details["kind"], "unknown_server");
    // The listing is cached until the server says it changed.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r9",
        r#"mcp_call {"tool": "fx/change_resources"}"#,
    );
    assert!(!is_error && text == "changed", "{text}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut n = 0;
    loop {
        n += 1;
        let (is_error, text, _) = run(
            &mut e,
            &s,
            &format!("c{n}"),
            r#"mcp_resources {"query": "added"}"#,
        );
        assert!(!is_error, "{text}");
        if text.contains("fixture://added") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "resources never refreshed: {text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The view carries counts and declared capabilities.
    let view = match e.call(Call::McpServers {
        session: Some(s.clone()),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    let fx_view = &view.servers[0];
    assert_eq!(fx_view.resources, Some(4));
    assert_eq!(fx_view.capabilities, ["tools", "resources", "prompts"]);
}

#[test]
fn prompts_are_listed_and_rendered_with_required_arguments_checked() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let (mut e, s) = start(&fx, &provider);
    let (is_error, text, details) = run(&mut e, &s, "r1", r#"mcp_prompts {}"#);
    assert!(!is_error, "{text}");
    assert!(text.starts_with("2 MCP prompt(s)"), "{text}");
    assert!(text.contains("- fx/greet(): Say hello"), "{text}");
    assert!(
        text.contains("- fx/summarize(topic, tone?): Summarize a topic"),
        "{text}"
    );
    assert_eq!(
        details["prompts"][0]["arguments"][0],
        json!({"name": "topic", "required": true, "description": "What to summarize"})
    );
    let (is_error, text, _) = run(&mut e, &s, "r2", r#"mcp_prompts {"query": "hello"}"#);
    assert!(
        !is_error && text.starts_with("1 MCP prompt(s) matching \"hello\""),
        "{text}"
    );
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_prompt {"prompt": "fx/summarize", "arguments": {"topic": "MCP", "tone": "casual"}}"#,
    );
    assert!(!is_error, "{text}");
    assert_eq!(
        text,
        "fx/summarize: rendered by the fixture\n\n[user]\nSummarize MCP in a casual tone.\n\n[assistant]\nCertainly. Which length?"
    );
    assert_eq!(details["messages"], 2);
    let (is_error, text, details) =
        run(&mut e, &s, "r4", r#"mcp_prompt {"prompt": "fx/summarize"}"#);
    assert!(
        is_error && text.contains("needs the argument \"topic\""),
        "{text}"
    );
    assert_eq!(details["kind"], "invalid_arguments");
    let (is_error, text, details) = run(&mut e, &s, "r5", r#"mcp_prompt {"prompt": "fx/nope"}"#);
    assert!(is_error && text.contains("has no tool \"nope\""), "{text}");
    assert_eq!(details["kind"], "unknown_tool");
    let (is_error, text, _) = run(&mut e, &s, "r6", r#"mcp_prompt {"prompt": "greet"}"#);
    assert!(is_error && text.contains("server/tool"), "{text}");
    let view = match e.call(Call::McpServers {
        session: Some(s.clone()),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    assert_eq!(view.servers[0].prompts, Some(2));
    // Pure reads are recoverable after a restart; the recovery registry says so.
    let shared = e.handle.shared();
    for tool in ["mcp_resources", "mcp_read", "mcp_prompts", "mcp_prompt"] {
        assert!(shared.recoverable_tool(tool).is_some(), "{tool}");
    }
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    for capability in ["mcpResources", "mcpPrompts", "mcpElicitation"] {
        assert!(
            welcome.capabilities.iter().any(|c| c == capability),
            "{capability}"
        );
    }
}

#[test]
fn a_server_without_a_feature_is_reported_rather_than_failing_the_listing() {
    let http = super::mcp2_tests::HttpMcp::start(false);
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"mcp": {"servers": {"fx": server(json!({})), "remote": {"url": http.url()}}}}),
    );
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let (is_error, text, details) = run(&mut e, &s, "r1", r#"mcp_resources {}"#);
    assert!(!is_error, "{text}");
    assert!(text.contains("- fx fixture://readme"), "{text}");
    assert!(
        text.contains("! remote: MCP server remote does not offer resources"),
        "{text}"
    );
    assert_eq!(details["problems"][0]["server"], "remote");
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r2",
        r#"mcp_read {"server": "remote", "uri": "x"}"#,
    );
    assert!(
        is_error && text.contains("does not offer resources"),
        "{text}"
    );
    assert_eq!(details["kind"], "unsupported");
    let (is_error, text, _) = run(&mut e, &s, "r3", r#"mcp_prompts {"server": "remote"}"#);
    assert!(
        !is_error && text.contains("! remote: MCP server remote does not offer prompts"),
        "{text}"
    );
    let view = match e.call(Call::McpServers {
        session: Some(s.clone()),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    let remote = view.servers.iter().find(|v| v.name == "remote").unwrap();
    assert_eq!(remote.capabilities, ["tools"]);
    assert_eq!(remote.resources, None);
}

#[test]
fn elicitation_becomes_native_questions_with_typed_answers() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let (mut e, s) = start(&fx, &provider);
    e.prompt(
        &s,
        "r1",
        r#"mcp_call {"tool": "fx/ask_person", "arguments": {"mode": "form"}}"#,
    )
    .unwrap();
    // Fields arrive in the schema's (alphabetical) order: age, color, name, ok.
    let q = open_question(&mut e, &s);
    assert_eq!(q.title, "age (optional)");
    assert_eq!(
        q.message.as_deref(),
        Some("fx asks: Tell me about yourself")
    );
    assert_eq!(q.kind, QuestionKind::Input);
    answer(
        &mut e,
        &s,
        &q,
        Answer::Text {
            value: "forty".into(),
        },
    );
    // A non-number is asked again with the reason, then accepted.
    let q = next_question(&mut e, &s, "");
    assert_eq!(q.title, "age (optional)");
    assert_eq!(
        q.message.as_deref(),
        Some("forty is not a whole number; try again.")
    );
    answer(
        &mut e,
        &s,
        &q,
        Answer::Text {
            value: " 40 ".into(),
        },
    );
    let q = next_question(&mut e, &s, "age (optional)");
    assert_eq!(q.title, "color");
    assert_eq!(q.kind, QuestionKind::Select);
    assert_eq!(
        q.options
            .iter()
            .map(|o| o.value.as_str())
            .collect::<Vec<_>>(),
        ["red", "blue"]
    );
    answer(
        &mut e,
        &s,
        &q,
        Answer::Choice {
            value: "blue".into(),
        },
    );
    let q = next_question(&mut e, &s, "color");
    assert_eq!(q.title, "name");
    assert_eq!(q.kind, QuestionKind::Input);
    answer(
        &mut e,
        &s,
        &q,
        Answer::Text {
            value: "Ada".into(),
        },
    );
    let q = next_question(&mut e, &s, "name");
    assert_eq!(q.title, "ok");
    assert_eq!(q.kind, QuestionKind::Confirm);
    answer(&mut e, &s, &q, Answer::Confirm { value: true });
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let result = snap
        .entries
        .iter()
        .find_map(|en| match &en.kind {
            EntryKind::Message {
                message: m @ Message::ToolResult { tool_name, .. },
                ..
            } if tool_name == "mcp_call" => Some(m.text()),
            _ => None,
        })
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["action"], "accept");
    assert_eq!(
        parsed["content"],
        json!({"name": "Ada", "age": 40, "ok": true, "color": "blue"})
    );
    assert!(snap.ui.questions.is_empty());
}

#[test]
fn declining_cancelling_and_unsupported_requests_are_honest() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let (mut e, s) = start(&fx, &provider);
    // Declining a field declines the request; the tool still answers.
    e.prompt(
        &s,
        "r1",
        r#"mcp_call {"tool": "fx/ask_person", "arguments": {"mode": "form"}}"#,
    )
    .unwrap();
    let q = open_question(&mut e, &s);
    e.call(Call::CancelQuestion {
        session: s.clone(),
        question: q.id,
    })
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let text = last_result(&e.snapshot(&s));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["action"],
        "decline"
    );
    // An empty form is a yes/no on the message.
    e.prompt(
        &s,
        "r2",
        r#"mcp_call {"tool": "fx/ask_person", "arguments": {"mode": "empty"}}"#,
    )
    .unwrap();
    let q = open_question(&mut e, &s);
    assert_eq!(q.title, "fx asks: Proceed with the operation?");
    assert_eq!(q.kind, QuestionKind::Confirm);
    answer(&mut e, &s, &q, Answer::Confirm { value: true });
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let parsed: serde_json::Value = serde_json::from_str(&last_result(&e.snapshot(&s))).unwrap();
    assert_eq!(parsed["action"], "accept");
    assert_eq!(parsed["content"], json!({}));
    // URL elicitation is not advertised and is declined without a question.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_call {"tool": "fx/ask_person", "arguments": {"mode": "url"}}"#,
    );
    assert!(!is_error, "{text}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["action"],
        "decline"
    );
    assert!(e.snapshot(&s).ui.questions.is_empty());
    // Stop while a question is open: the call is aborted and the question withdrawn.
    e.prompt(
        &s,
        "r4",
        r#"mcp_call {"tool": "fx/ask_person", "arguments": {"mode": "form"}}"#,
    )
    .unwrap();
    open_question(&mut e, &s);
    e.call(Call::Stop { session: s.clone() }).unwrap();
    assert_eq!(e.settled("r4"), RunOutcome::Aborted);
    assert!(e.snapshot(&s).ui.questions.is_empty());
}

fn last_result(snap: &SessionSnapshot) -> String {
    snap.entries
        .iter()
        .rev()
        .find_map(|en| match &en.kind {
            EntryKind::Message {
                message: m @ Message::ToolResult { tool_name, .. },
                ..
            } if tool_name == "mcp_call" => Some(m.text()),
            _ => None,
        })
        .unwrap()
}

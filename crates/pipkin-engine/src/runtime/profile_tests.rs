//! Subagent profiles (SUB-1) through the client contract: tool restrictions enforced in the
//! child's catalog and at invocation, instructions as a system section, model validation
//! without substitution, inherited context, caps, the frozen launch record across a settings
//! edit and a restart, and the profile on directory rows.

use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::profiles::{ContextMode, LaunchRecord};
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use crate::store::Store;

fn last_is_user(req: &ProviderRequest) -> bool {
    matches!(
        req.messages
            .iter()
            .rev()
            .find(|m| !matches!(m, Message::System(_))),
        Some(Message::User { .. })
    )
}

fn user_text(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::User { .. } => Some(m.text()),
            _ => None,
        })
        .unwrap_or_default()
}

fn last_tool_result(req: &ProviderRequest) -> Option<String> {
    req.messages.iter().rev().find_map(|m| match m {
        Message::ToolResult { .. } => Some(m.text()),
        _ => None,
    })
}

/// The parent delegates when its user message starts with `delegate`, passing the rest of
/// the line as the subagent arguments (JSON). A child (user text starting with `child:`)
/// tries the tool named after the colon, then reports what it saw.
fn script(child_hang: Option<Gate>) -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        let text = user_text(req);
        let is_child = req
            .messages
            .iter()
            .any(|m| matches!(m, Message::User { .. }) && m.text().starts_with("child:"));
        if is_child {
            if last_is_user(req) {
                let first = req
                    .messages
                    .iter()
                    .find_map(|m| match m {
                        Message::User { .. } => Some(m.text()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let tool = first
                    .trim_start_matches("child:")
                    .split_whitespace()
                    .next()
                    .unwrap_or("none")
                    .to_owned();
                let reply = if tool == "none" {
                    R::Text("child done".into())
                } else {
                    R::Tools(
                        None,
                        vec![(
                            tool,
                            json!({"path": "made-by-child.txt", "content": "x", "command": "echo hi", "ops": [{"op": "create", "title": "t"}], "url": "http://example.com/", "query": "q", "source": "s1"}),
                        )],
                    )
                };
                match &child_hang {
                    Some(gate) => R::Gated(gate.clone(), Box::new(reply)),
                    None => reply,
                }
            } else {
                R::Text(format!(
                    "child saw: {}",
                    last_tool_result(req).unwrap_or_default()
                ))
            }
        } else if last_is_user(req) && text.starts_with("delegate") {
            let args: serde_json::Value =
                serde_json::from_str(text.trim_start_matches("delegate").trim()).unwrap();
            R::Tools(None, vec![("subagent".into(), args)])
        } else if last_is_user(req) {
            R::Text(format!("parent echo: {text}"))
        } else {
            R::Text(format!(
                "parent saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    }
}

fn delegate(task: &str, extra: serde_json::Value) -> String {
    let mut args = json!({"task": task});
    if let (Some(a), Some(e)) = (args.as_object_mut(), extra.as_object()) {
        a.extend(e.clone());
    }
    format!("delegate {args}")
}

fn child_requests(p: &Scripted) -> Vec<ProviderRequest> {
    p.requests()
        .into_iter()
        .filter(|r| {
            r.messages
                .iter()
                .any(|m| matches!(m, Message::User { .. }) && m.text().starts_with("child:"))
        })
        .collect()
}

fn subagents(e: &mut Engine, s: &str) -> SubagentsView {
    match e.call(Call::Subagents { session: s.into() }) {
        Ok(Reply::Subagents(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn parent_result(snap: &SessionSnapshot) -> (bool, String) {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    },
                ..
            } if tool_name == "subagent" => Some((*is_error, m.text())),
            _ => None,
        })
        .next_back()
        .unwrap()
}

fn tool_names(req: &ProviderRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.clone()).collect()
}

fn write_settings(fx: &Fixture, body: serde_json::Value) {
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(fx.dir.path().join("config/settings.json"), body.to_string()).unwrap();
}

#[test]
fn a_reviewer_is_offered_only_reads_and_its_write_is_refused() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(None));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(
        &s,
        "r1",
        &delegate("child:write the file", json!({"profile": "reviewer"})),
    )
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let child = child_requests(&provider);
    assert!(!child.is_empty());
    assert_eq!(tool_names(&child[0]), ["read", "plan_read", "web_read"]);
    let resolved = child[0].resolve_systems(false);
    assert!(
        resolved
            .system
            .contains("You are reviewing, not implementing"),
        "{}",
        resolved.system
    );
    assert!(
        !fx.project.join("made-by-child.txt").exists(),
        "the write never ran"
    );
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(!is_error);
    assert!(text.contains("Tool write is not available"), "{text}");
    let view = subagents(&mut e, &s);
    assert_eq!(view.children.len(), 1);
    assert_eq!(view.children[0].profile.as_deref(), Some("reviewer"));
    assert_eq!(view.children[0].status, "done");
    // The worker keeps everything a child can have; bash included, subagent and ask never.
    e.prompt(&s, "r2", &delegate("child:none", json!({})))
        .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let child = child_requests(&provider).into_iter().last().unwrap();
    let names = tool_names(&child);
    assert!(names.contains(&"bash".into()) && names.contains(&"write".into()));
    assert!(!names.contains(&"subagent".into()) && !names.contains(&"ask_user_question".into()));
    assert!(
        !child
            .resolve_systems(false)
            .system
            .contains("You are reviewing")
    );
    assert_eq!(
        subagents(&mut e, &s).children[1].profile.as_deref(),
        Some("worker")
    );
}

#[test]
fn a_scout_reaches_the_web_but_not_the_workspace() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(None));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(
        &s,
        "r1",
        &delegate("child:bash run", json!({"profile": "scout"})),
    )
    .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let child = child_requests(&provider);
    assert_eq!(
        tool_names(&child[0]),
        ["read", "plan_read", "web_search", "web_fetch", "web_read"]
    );
    let (_, text) = parent_result(&e.snapshot(&s));
    assert!(text.contains("Tool bash is not available"), "{text}");
}

#[test]
fn unknown_profiles_bad_context_and_unavailable_models_fail_without_creating_a_child() {
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"subagents": {"profiles": {"fancy": {"model": "nope/model"}, "deep": {"thinking": "galactic"}}}}),
    );
    let provider = Scripted::new(script(None));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    for (key, extra, needle) in [
        (
            "r1",
            json!({"profile": "ghost"}),
            "Unknown subagent profile \"ghost\"",
        ),
        (
            "r2",
            json!({"context": "shared"}),
            "context must be \"fresh\" or \"inherit\"",
        ),
        (
            "r3",
            json!({"profile": "fancy"}),
            "Model nope/model (profile \"fancy\") is not available",
        ),
        (
            "r4",
            json!({"profile": "deep"}),
            "Thinking level \"galactic\"",
        ),
    ] {
        e.prompt(&s, key, &delegate("child:none", extra)).unwrap();
        assert_eq!(e.settled(key), RunOutcome::Completed);
        let (is_error, text) = parent_result(&e.snapshot(&s));
        assert!(is_error, "{key}: {text}");
        assert!(text.contains(needle), "{key}: {text}");
    }
    assert!(
        subagents(&mut e, &s).children.is_empty(),
        "no child was created"
    );
    assert!(child_requests(&provider).is_empty());
    // Misconfigured profiles refuse every launch rather than silently narrowing.
    write_settings(
        &fx,
        json!({"subagents": {"profiles": {"x": {"tools": ["grep"]}}}}),
    );
    e.prompt(&s, "r5", &delegate("child:none", json!({})))
        .unwrap();
    assert_eq!(e.settled("r5"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(
        is_error && text.contains("misconfigured") && text.contains("unknown tool grep"),
        "{text}"
    );
}

#[test]
fn inherited_context_is_a_bounded_labelled_excerpt_and_fresh_is_the_default() {
    let fx = Fixture::new();
    let provider = Scripted::new(script(None));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r0", "remember the magic word is pelican")
        .unwrap();
    assert_eq!(e.settled("r0"), RunOutcome::Completed);
    e.prompt(&s, "r1", &delegate("child:none fresh", json!({})))
        .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let fresh = child_requests(&provider);
    assert!(
        !user_text(&fresh[0]).contains("pelican"),
        "{}",
        user_text(&fresh[0])
    );
    assert!(!user_text(&fresh[0]).contains("<parent_context"));
    e.prompt(
        &s,
        "r2",
        &delegate("child:none inherit", json!({"context": "inherit"})),
    )
    .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let inherited = child_requests(&provider).into_iter().last().unwrap();
    let text = user_text(&inherited);
    assert!(
        text.starts_with("child:none inherit\n\n<parent_context revision="),
        "{text}"
    );
    assert!(
        text.contains("User: remember the magic word is pelican"),
        "{text}"
    );
    assert!(
        text.contains("Assistant: parent echo: remember the magic word is pelican"),
        "{text}"
    );
    assert!(text.ends_with("</parent_context>"), "{text}");
    // The child is still not offered delegation or questions, and the record says what
    // was inherited.
    assert!(!tool_names(&inherited).contains(&"subagent".into()));
    let view = subagents(&mut e, &s);
    let child_session = view.children[1].session.clone();
    let task = e
        .handle
        .shared()
        .store
        .read(|c| Store::task_by_session(c, &child_session))
        .unwrap()
        .unwrap();
    let launch = LaunchRecord::from_json(task.launch.as_deref().unwrap()).unwrap();
    assert_eq!(launch.context, ContextMode::Inherit);
    assert!(launch.parent_revision.is_some());
    assert!(launch.context_messages >= 2, "{launch:?}");
    assert_eq!(launch.tools, None);
    assert_eq!(launch.profile, "worker");
}

#[test]
fn caps_bound_total_and_concurrent_children() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"subagents": {"maxTotal": 1}}));
    let provider = Scripted::new(script(None));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", &delegate("child:none", json!({})))
        .unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert!(!parent_result(&e.snapshot(&s)).0);
    e.prompt(&s, "r2", &delegate("child:none", json!({})))
        .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let (is_error, text) = parent_result(&e.snapshot(&s));
    assert!(
        is_error && text.contains("already has 1 subagents (the limit is 1)"),
        "{text}"
    );
    assert_eq!(subagents(&mut e, &s).children.len(), 1);

    // Concurrency: two calls in one round with a limit of one; the first child is held by a
    // gate until the second call has been refused.
    let fx = Fixture::new();
    write_settings(&fx, json!({"subagents": {"maxConcurrent": 1}}));
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, n| {
        if last_is_user(req) && user_text(req) == "two" {
            R::Tools(
                None,
                vec![
                    ("subagent".into(), json!({"task": "child:none a"})),
                    ("subagent".into(), json!({"task": "child:none b"})),
                ],
            )
        } else {
            script(Some(g.clone()))(req, n)
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "two").unwrap();
    let store = e.handle.shared().store.clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let done = store
            .read(|c| Store::tools_in_state(c, &s, "done"))
            .unwrap();
        if done.iter().any(|c| c.name == "subagent") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the refused call did not settle"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let results: Vec<(bool, String)> = snap
        .entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    },
                ..
            } if tool_name == "subagent" => Some((*is_error, m.text())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results.iter().filter(|(e, _)| *e).count(), 1, "{results:?}");
    let refused = results.iter().find(|(e, _)| *e).unwrap();
    assert!(
        refused.1.contains("already running (the limit is 1)"),
        "{}",
        refused.1
    );
    assert_eq!(subagents(&mut e, &s).children.len(), 1);
}

#[test]
fn an_edited_profile_does_not_change_a_recovering_child() {
    let fx = Fixture::new();
    let s;
    let gate = Gate::new();
    {
        let provider = Scripted::new(script(Some(gate.clone())));
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.prompt(
            &s,
            "r1",
            &delegate("child:write it", json!({"profile": "reviewer"})),
        )
        .unwrap();
        e.until("the child asked its model", |_| {
            !child_requests(&provider).is_empty()
        });
        assert_eq!(
            tool_names(&child_requests(&provider)[0]),
            ["read", "plan_read", "web_read"]
        );
        e.handle.shutdown(Duration::from_millis(200));
    }
    // The reviewer profile now allows writing; the recovering child must not pick that up.
    write_settings(
        &fx,
        json!({"subagents": {"profiles": {"reviewer": {"tools": ["read", "write", "edit", "bash"]}}}}),
    );
    let provider = Scripted::new(script(None));
    let mut e = fx.start(&provider);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        e.snapshot(&s);
        if let Ok(Reply::Lookup(LookupResult::Known {
            open: false,
            outcome: Some(outcome),
            ..
        })) = e.call(Call::Lookup {
            session: s.clone(),
            request: "r1".into(),
        }) {
            assert_eq!(outcome, RunOutcome::Completed);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "r1 did not settle after the restart"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let resumed = child_requests(&provider);
    assert!(!resumed.is_empty(), "the same child continued");
    for req in &resumed {
        assert_eq!(
            tool_names(req),
            ["read", "plan_read", "web_read"],
            "frozen policy"
        );
    }
    assert!(!fx.project.join("made-by-child.txt").exists());
    let view = subagents(&mut e, &s);
    assert_eq!(view.children.len(), 1, "no second child");
    assert_eq!(view.children[0].profile.as_deref(), Some("reviewer"));
    // A new child launched now does get the edited profile.
    e.prompt(
        &s,
        "r2",
        &delegate("child:none", json!({"profile": "reviewer"})),
    )
    .unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let latest = child_requests(&provider).into_iter().last().unwrap();
    assert_eq!(tool_names(&latest), ["read", "write", "edit", "bash"]);
}

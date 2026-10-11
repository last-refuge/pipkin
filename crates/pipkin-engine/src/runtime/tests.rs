//! Engine behavior through the client contract, with the scripted provider.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::json;

use super::{EngineHandle, registry_with};
use crate::config::{EngineConfig, Env};
use crate::host::Connection;
use crate::provider::ProviderRequest;
use crate::provider::scripted::{self, Gate, Reply as R, Scripted};

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub project: PathBuf,
}

impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        Fixture { dir, project }
    }

    pub fn config(&self) -> EngineConfig {
        let mut c = EngineConfig::new(self.dir.path().join("data"), self.dir.path().join("config"));
        c.env = Env::isolated(Default::default());
        c.extra_models = vec![scripted::model()];
        c.default_model = Some("scripted/scripted".into());
        c
    }

    pub fn start(&self, provider: &Scripted) -> Engine {
        let handle = EngineHandle::start_with(
            self.config(),
            registry_with(Arc::new(provider.clone())),
            crate::tools::builtin_tools(),
        )
        .unwrap();
        let conn = handle.connect("test");
        let mut e = Engine {
            handle,
            conn,
            next: 0,
            events: vec![],
        };
        match e.call(Call::Hello {
            protocol: PROTOCOL_VERSION,
            client: "test".into(),
        }) {
            Ok(Reply::Welcome(_)) => {}
            other => panic!("{other:?}"),
        }
        e
    }
}

pub struct Engine {
    pub handle: EngineHandle,
    pub conn: Connection,
    next: u64,
    pub events: Vec<Event>,
}

impl Engine {
    pub fn call(&mut self, call: Call) -> Result<Reply, ProtoError> {
        self.next += 1;
        let id = self.next;
        self.conn
            .send(ClientFrame {
                id,
                lane: call.lane(),
                call,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match self
                .conn
                .recv_timeout(deadline - Instant::now())
                .expect("reply in time")
            {
                ServerFrame::Reply { id: got, result } if got == id => return result,
                ServerFrame::Reply { .. } => {}
                ServerFrame::Event { event, .. } => self.events.push(event),
            }
        }
    }

    /// Pump events until `done` sees what it wants.
    pub fn until(&mut self, what: &str, done: impl Fn(&[Event]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(&self.events) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out waiting for {what}; events: {:#?}",
                self.events
            );
            if let Ok(ServerFrame::Event { event, .. }) =
                self.conn.recv_timeout(left.min(Duration::from_millis(100)))
            {
                self.events.push(event);
            }
        }
    }

    pub fn create(&mut self, cwd: &Path, key: &str) -> String {
        match self.call(Call::CreateSession {
            cwd: cwd.display().to_string(),
            request: key.into(),
        }) {
            Ok(Reply::Session { summary }) => summary.id,
            other => panic!("{other:?}"),
        }
    }

    pub fn prompt(&mut self, session: &str, key: &str, text: &str) -> Result<Reply, ProtoError> {
        self.call(Call::Prompt {
            session: session.into(),
            request: key.into(),
            text: text.into(),
            images: vec![],
            model: None,
        })
    }

    pub fn settled(&mut self, request: &str) -> RunOutcome {
        let req = request.to_owned();
        self.until(&format!("settlement of {request}"), |ev| {
            ev.iter().any(|e| matches!(e, Event::Session { change: SessionChange::RunSettled { request: Some(r), .. }, .. } if *r == req))
        });
        self.events
            .iter()
            .find_map(|e| match e {
                Event::Session {
                    change:
                        SessionChange::RunSettled {
                            request: Some(r),
                            outcome,
                            ..
                        },
                    ..
                } if *r == req => Some(outcome.clone()),
                _ => None,
            })
            .unwrap()
    }

    pub fn snapshot(&mut self, session: &str) -> SessionSnapshot {
        match self.call(Call::OpenSession {
            session: session.into(),
            window: None,
        }) {
            Ok(Reply::Snapshot(s)) => *s,
            other => panic!("{other:?}"),
        }
    }
}

pub fn texts(snapshot: &SessionSnapshot) -> Vec<(String, String, bool)> {
    snapshot
        .entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message { message, .. } => {
                let role = match message {
                    Message::User { .. } => "user",
                    Message::Assistant { .. } => "assistant",
                    Message::ToolResult { .. } => "tool",
                };
                Some((role.to_owned(), message.text(), e.retired))
            }
            _ => None,
        })
        .collect()
}

fn user_texts(req: &ProviderRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter(|m| matches!(m, Message::User { .. }))
        .map(|m| m.text())
        .collect()
}

#[test]
fn sessions_are_created_once_per_key_and_listed() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("hi"));
    let a = e.create(&fx.project, "k1");
    let b = e.create(&fx.project, "k1");
    assert_eq!(a, b);
    let Ok(Reply::Sessions { sessions }) = e.call(Call::ListSessions) else {
        panic!()
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].model.as_deref(), Some("scripted/scripted"));
    assert!(
        e.call(Call::CreateSession {
            cwd: "relative".into(),
            request: "k2".into()
        })
        .is_err()
    );
}

#[test]
fn a_prompt_is_admitted_durably_streamed_settled_and_looked_up() {
    let fx = Fixture::new();
    let provider = Scripted::echo("hello there, friend").paced(Duration::from_millis(15));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let Ok(Reply::Admitted { run: Some(_), .. }) = e.prompt(&s, "r1", "say hi") else {
        panic!()
    };
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    // Streaming progress was committed (deltas published) before the final replacement.
    assert!(e.events.iter().any(|ev| matches!(
        ev,
        Event::Session {
            change: SessionChange::EntryDelta { .. },
            ..
        }
    )));
    let snap = e.snapshot(&s);
    assert_eq!(
        texts(&snap),
        vec![
            ("user".into(), "say hi".into(), false),
            ("assistant".into(), "hello there, friend".into(), false)
        ]
    );
    assert!(snap.run.is_none());
    assert_eq!(snap.usage.models[0].0, "scripted/scripted");
    let Ok(Reply::Lookup(LookupResult::Known {
        open: false,
        outcome: Some(RunOutcome::Completed),
        ..
    })) = e.call(Call::Lookup {
        session: s.clone(),
        request: "r1".into(),
    })
    else {
        panic!()
    };
    let Ok(Reply::Lookup(LookupResult::Unknown)) = e.call(Call::Lookup {
        session: s.clone(),
        request: "nope".into(),
    }) else {
        panic!()
    };
    // The same key again is the original admission, not a second run.
    let Ok(Reply::Admitted { .. }) = e.prompt(&s, "r1", "say hi") else {
        panic!()
    };
    let conflict = e.prompt(&s, "r1", "something else").unwrap_err();
    assert_eq!(conflict.code, ErrorCode::Conflict);
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn a_busy_session_refuses_a_second_prompt_without_recording_it() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| {
        if n == 0 {
            R::Gated(g.clone(), Box::new(R::Text("done".into())))
        } else {
            R::Text("x".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "first").unwrap();
    let busy = e.prompt(&s, "r2", "second").unwrap_err();
    assert_eq!(busy.code, ErrorCode::Busy);
    let Ok(Reply::Lookup(LookupResult::Unknown)) = e.call(Call::Lookup {
        session: s.clone(),
        request: "r2".into(),
    }) else {
        panic!()
    };
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
}

#[test]
fn stop_settles_aborted_and_retires_the_instruction_from_the_next_context() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, n| {
        if n == 0 {
            R::Hang
        } else {
            R::Text("fresh".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "delete everything").unwrap();
    e.until("the model was asked", |_| !provider.requests().is_empty());
    let Ok(Reply::Stopping { run: Some(_) }) = e.call(Call::Stop { session: s.clone() }) else {
        panic!()
    };
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    let snap = e.snapshot(&s);
    assert_eq!(
        texts(&snap)[0],
        ("user".into(), "delete everything".into(), true),
        "kept in history, retired"
    );
    e.prompt(&s, "r2", "say hello").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let next = &provider.requests()[1];
    assert_eq!(user_texts(next), vec!["say hello".to_owned()]);
    // A second Stop with nothing running is harmless.
    let Ok(Reply::Stopping { run: None }) = e.call(Call::Stop { session: s.clone() }) else {
        panic!()
    };
}

#[test]
fn stop_withdraws_queued_inputs_and_follow_ups_run_after_completion() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Gated(g.clone(), Box::new(R::Text("one".into()))),
        _ => R::Text("two".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "first").unwrap();
    let Ok(Reply::Admitted {
        entry: Some(_),
        run: None,
    }) = e.call(Call::Enqueue {
        session: s.clone(),
        request: "f1".into(),
        mode: InputMode::FollowUp,
        text: "then this".into(),
        images: vec![],
    })
    else {
        panic!()
    };
    assert_eq!(e.snapshot(&s).queue.len(), 1);
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert_eq!(e.settled("f1"), RunOutcome::Completed);
    assert_eq!(
        user_texts(&provider.requests()[1]),
        vec!["first".to_owned(), "then this".to_owned()]
    );

    // Stop withdraws what is still queued.
    let provider2 = Scripted::new(|_, _| R::Hang);
    let fx2 = Fixture::new();
    let mut e2 = fx2.start(&provider2);
    let s2 = e2.create(&fx2.project, "c");
    e2.prompt(&s2, "a", "x").unwrap();
    e2.call(Call::Enqueue {
        session: s2.clone(),
        request: "q".into(),
        mode: InputMode::FollowUp,
        text: "later".into(),
        images: vec![],
    })
    .unwrap();
    e2.call(Call::Stop {
        session: s2.clone(),
    })
    .unwrap();
    assert_eq!(e2.settled("a"), RunOutcome::Aborted);
    let Ok(Reply::Lookup(LookupResult::Known {
        open: false,
        outcome: Some(RunOutcome::Aborted),
        ..
    })) = e2.call(Call::Lookup {
        session: s2.clone(),
        request: "q".into(),
    })
    else {
        panic!()
    };
    assert!(e2.snapshot(&s2).queue.is_empty());
}

#[test]
fn queued_inputs_can_be_withdrawn_before_they_run() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| {
        if n == 0 {
            R::Gated(g.clone(), Box::new(R::Text("a".into())))
        } else {
            R::Text("b".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "first").unwrap();
    let Ok(Reply::Admitted {
        entry: Some(entry), ..
    }) = e.call(Call::Enqueue {
        session: s.clone(),
        request: "f1".into(),
        mode: InputMode::FollowUp,
        text: "never".into(),
        images: vec![],
    })
    else {
        panic!()
    };
    let Ok(Reply::CancelQueued {
        outcome: CancelQueuedOutcome::Cancelled,
    }) = e.call(Call::CancelQueued {
        session: s.clone(),
        entry,
    })
    else {
        panic!()
    };
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        provider.requests().len(),
        1,
        "the withdrawn input never reached the model"
    );
}

#[test]
fn an_engine_restart_mid_run_resumes_the_run_without_duplicating_the_prompt() {
    let fx = Fixture::new();
    let hang = Scripted::new(|_, _| R::Hang);
    let s;
    {
        let mut e = fx.start(&hang);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "keep going").unwrap();
        e.until("the model was asked", |_| !hang.requests().is_empty());
        e.handle.shutdown(Duration::from_millis(100));
    }
    let answer = Scripted::echo("resumed answer");
    let mut e = fx.start(&answer);
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let snap = e.snapshot(&s);
    let users: Vec<_> = texts(&snap).into_iter().filter(|t| t.0 == "user").collect();
    assert_eq!(users.len(), 1);
    assert_eq!(answer.requests().len(), 1);
    assert_eq!(
        user_texts(&answer.requests()[0]),
        vec!["keep going".to_owned()]
    );
}

#[test]
fn a_durable_stop_survives_a_restart_and_settles_aborted() {
    let fx = Fixture::new();
    let s;
    {
        let hang = Scripted::new(|_, _| R::Hang);
        let mut e = fx.start(&hang);
        s = e.create(&fx.project, "c");
        e.prompt(&s, "r1", "x").unwrap();
        e.until("asked", |_| !hang.requests().is_empty());
        // Record Stop intent directly, as if the engine died right after committing it.
        e.handle.shared().store.request_stop(&s, 1).unwrap();
        e.handle.shutdown(Duration::from_millis(100));
    }
    let never = Scripted::echo("must not run");
    let mut e = fx.start(&never);
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    assert!(never.requests().is_empty());
}

#[test]
fn a_storage_failure_fails_the_session_closed_until_it_is_reopened() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.handle.shared().store.fail_next_write();
    let err = e.prompt(&s, "r1", "x").unwrap_err();
    assert_eq!(err.code, ErrorCode::Storage);
    let again = e.prompt(&s, "r2", "y").unwrap_err();
    assert_eq!(again.code, ErrorCode::Storage, "fails closed");
    let Ok(Reply::Lookup(LookupResult::Unknown)) = e.call(Call::Lookup {
        session: s.clone(),
        request: "r1".into(),
    }) else {
        panic!("nothing was admitted")
    };
    e.snapshot(&s); // reopen reconciles
    e.prompt(&s, "r3", "z").unwrap();
    assert_eq!(e.settled("r3"), RunOutcome::Completed);
}

#[test]
fn model_errors_retry_then_settle_and_missing_models_are_reported() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, n| {
        if n == 0 {
            R::Error("503: overloaded".into())
        } else {
            R::Text("recovered".into())
        }
    });
    let mut cfg_settings = fx.config();
    std::fs::create_dir_all(&cfg_settings.config_dir).unwrap();
    std::fs::write(
        cfg_settings.config_dir.join("settings.json"),
        r#"{"retry": {"baseDelayMs": 1}}"#,
    )
    .unwrap();
    cfg_settings.default_model = Some("scripted/scripted".into());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "x").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert_eq!(provider.requests().len(), 2);

    let fatal = Scripted::new(|_, _| R::Error("401: invalid api key".into()));
    let fx2 = Fixture::new();
    let mut e2 = fx2.start(&fatal);
    let s2 = e2.create(&fx2.project, "c");
    e2.prompt(&s2, "r1", "x").unwrap();
    assert_eq!(
        e2.settled("r1"),
        RunOutcome::ModelError {
            detail: "401: invalid api key".into()
        }
    );
    assert_eq!(fatal.requests().len(), 1);

    let mut e3 = Fixture::new();
    let fx3 = std::mem::replace(&mut e3, Fixture::new());
    let mut cfg = fx3.config();
    cfg.default_model = None;
    cfg.extra_models.clear();
    let handle =
        EngineHandle::start_with(cfg, registry_with(Arc::new(Scripted::echo("x"))), vec![])
            .unwrap();
    let conn = handle.connect("t");
    let mut e3 = Engine {
        handle,
        conn,
        next: 0,
        events: vec![],
    };
    let s3 = e3.create(&fx3.project, "c");
    e3.prompt(&s3, "r1", "x").unwrap();
    assert_eq!(
        e3.settled("r1"),
        RunOutcome::NoModel {
            detail: "No model is configured".into()
        }
    );
}

#[test]
fn thinking_and_model_selection_are_validated_and_persisted() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("x"));
    let s = e.create(&fx.project, "c");
    assert_eq!(e.snapshot(&s).thinking.level, "medium");
    let Ok(Reply::Thinking(t)) = e.call(Call::SetThinking {
        session: s.clone(),
        level: "high".into(),
    }) else {
        panic!()
    };
    assert_eq!(t.level, "high");
    assert!(
        e.call(Call::SetThinking {
            session: s.clone(),
            level: "max".into()
        })
        .is_err()
    );
    assert_eq!(
        e.call(Call::SetModel {
            session: s.clone(),
            model: "nope/x".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    e.call(Call::SetModel {
        session: s.clone(),
        model: "scripted/scripted".into(),
    })
    .unwrap();
    let saved = std::fs::read_to_string(fx.dir.path().join("config/settings.json")).unwrap();
    assert!(saved.contains("\"defaultModel\": \"scripted\""), "{saved}");
}

#[test]
fn inputs_are_bounded_before_anything_is_recorded() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("x"));
    let s = e.create(&fx.project, "c");
    let big = "x".repeat(limits::MAX_PROMPT_BYTES + 1);
    assert_eq!(
        e.prompt(&s, "r", &big).unwrap_err().code,
        ErrorCode::TooLarge
    );
    assert_eq!(
        e.prompt(&s, "r", "  ").unwrap_err().code,
        ErrorCode::Invalid
    );
    assert_eq!(e.prompt(&s, "", "x").unwrap_err().code, ErrorCode::Invalid);
    assert_eq!(
        e.prompt("missing", "r", "x").unwrap_err().code,
        ErrorCode::NotFound
    );
    let _ = json!({});
}

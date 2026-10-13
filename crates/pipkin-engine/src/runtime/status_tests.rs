//! Context occupancy through the client contract: unavailable before any turn, reported
//! from the last response's usage after a turn, estimated once the person typed again, and
//! published as `ContextChanged` after settlement and after a compaction.

use pipkin_engine_protocol::*;

use super::tests::Fixture;
use crate::provider::scripted::{Reply as R, Scripted};

#[test]
fn context_is_reported_after_a_turn_and_published_on_change() {
    let fx = Fixture::new();
    // Keep little recent context so a short conversation can be compacted.
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        r#"{"compaction": {"keepRecentTokens": 5}}"#,
    )
    .unwrap();
    let provider = Scripted::new(|_, _| R::Text("a short answer".into()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let before = e.snapshot(&s).context;
    assert_eq!(before.source, "unavailable");
    assert_eq!(before.tokens, None);
    assert_eq!(before.model.as_deref(), Some("scripted/scripted"));
    e.prompt(&s, "r1", "hello").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let s2 = s.clone();
    e.until("a context change was published", |ev| {
        ev.iter().any(|ev| matches!(ev, Event::Session { session, change: SessionChange::ContextChanged { context }, .. } if *session == s2 && context.source == "reported"))
    });
    let after = e.snapshot(&s).context;
    // The scripted provider reports 10 input + 5 output tokens per reply.
    assert_eq!(after.source, "reported");
    assert_eq!(after.tokens, Some(15));
    assert!(after.measured_entry.is_some());
    assert_eq!(after.measured_model.as_deref(), Some("scripted/scripted"));
    assert!(!after.stale);
    assert_eq!(after.compaction, "idle");
    assert!(after.revision > 0);
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "contextUsage"));
    // Another turn: still reported, and larger.
    e.prompt(&s, "r2", "and again").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let again = e.snapshot(&s).context;
    assert_eq!(again.source, "reported");
    assert_eq!(
        again.tokens,
        Some(15),
        "each scripted reply reports the same request size"
    );
    assert!(again.measured_entry > after.measured_entry);
    // A manual compaction publishes a fresh figure: nothing measured after the summary, so it
    // is an estimate. A few more turns first, so there is something to summarize.
    for i in 3..12 {
        let key = format!("r{i}");
        e.prompt(
            &s,
            &key,
            "more text to keep the conversation going for a while",
        )
        .unwrap();
        assert_eq!(e.settled(&key), RunOutcome::Completed);
    }
    let provider_calls = provider.requests().len();
    match e.call(Call::Compact {
        session: s.clone(),
        instructions: None,
    }) {
        Ok(_) => {}
        Err(e) => panic!("{e:?}"),
    }
    let s3 = s.clone();
    e.until("the compaction moved the boundary", |ev| {
        ev.iter().any(|ev| matches!(ev, Event::Session { session, change: SessionChange::BoundaryMoved { .. }, .. } if *session == s3))
    });
    let compacted = e.snapshot(&s).context;
    assert_eq!(compacted.source, "estimated", "{compacted:?}");
    assert!(compacted.tokens.is_some());
    assert!(
        provider.requests().len() > provider_calls,
        "the summary asked the model"
    );
}

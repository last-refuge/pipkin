//! The activity directory through the client contract: groups, counts, keyset paging,
//! refetch, store-only observation (no runtime is built), child rows and the guarded stop.

use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;

use super::super::tests::{Engine, Fixture};
use crate::provider::scripted::{Gate, Reply as R, Scripted};

fn page(
    e: &mut Engine,
    filter: ActivityFilter,
    group: ActivityGroup,
    after: Option<ActivityCursor>,
    limit: u32,
) -> ActivityPage {
    match e.call(Call::Activity {
        filter,
        group,
        after,
        limit,
    }) {
        Ok(Reply::Activity(p)) => p,
        other => panic!("{other:?}"),
    }
}

fn all(e: &mut Engine) -> ActivityPage {
    page(e, ActivityFilter::default(), ActivityGroup::All, None, 200)
}

fn row(e: &mut Engine, session: &str) -> ActivityRow {
    all(e)
        .rows
        .into_iter()
        .find(|r| r.session == session)
        .unwrap_or_else(|| panic!("no row for {session}"))
}

/// Poll the directory (which publishes nothing itself) until `done` holds.
fn until_row(
    e: &mut Engine,
    session: &str,
    what: &str,
    done: impl Fn(&ActivityRow) -> bool,
) -> ActivityRow {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let r = row(e, session);
        if done(&r) {
            return r;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {r:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn live(e: &Engine) -> usize {
    e.handle.shared().sessions.lock().unwrap().live.len()
}

#[test]
fn welcome_advertises_the_directory_and_follow_up() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let Ok(Reply::Welcome(w)) = e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    }) else {
        panic!()
    };
    for cap in [
        CAPABILITY_ACTIVITY_DIRECTORY,
        CAPABILITY_ACTIVITY_OBSERVE,
        CAPABILITY_GUARDED_STOP,
    ] {
        assert!(w.capabilities.iter().any(|c| c == cap), "{cap}");
    }
    assert!(
        w.capabilities
            .iter()
            .any(|c| c == CAPABILITY_SUBAGENT_FOLLOW_UP)
    );
    let s = e.create(&fx.project, "s");
    let err = e
        .call(Call::FollowUpSubagent {
            session: s,
            child: 1,
            request: "k".into(),
            text: "t".into(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn groups_and_counts_put_each_worker_in_exactly_one_place() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.messages.iter().any(|m| m.text().contains("hang")) {
            R::Gated(g.clone(), Box::new(R::Text("late".into())))
        } else {
            R::Text("done".into())
        }
    });
    let mut e = fx.start(&provider);
    let idle = e.create(&fx.project, "idle");
    let done = e.create(&fx.project, "done");
    let busy = e.create(&fx.project, "busy");
    e.prompt(&done, "d1", "finish").unwrap();
    assert_eq!(e.settled("d1"), RunOutcome::Completed);
    e.prompt(&busy, "b1", "hang please").unwrap();
    until_row(&mut e, &busy, "running", |r| r.state == WorkState::Running);

    let p = all(&mut e);
    assert_eq!(p.counts.total, 3);
    assert_eq!(p.counts.running, 1);
    assert_eq!(p.counts.finished_recent, 1);
    assert_eq!(p.counts.attention, 0);
    assert_eq!(p.counts.failed_recent, 0);
    assert!(p.coverage.roots && p.coverage.children && p.coverage.live_attention);
    assert!(p.through > 0);
    assert_eq!(row(&mut e, &idle).state, WorkState::Ready);
    let finished = row(&mut e, &done);
    assert_eq!(finished.state, WorkState::Ready);
    assert_eq!(
        finished.run.as_ref().and_then(|r| r.outcome.clone()),
        Some(RunOutcome::Completed)
    );
    let running = page(
        &mut e,
        ActivityFilter::default(),
        ActivityGroup::Running,
        None,
        10,
    );
    assert_eq!(
        running.rows.iter().map(|r| &r.session).collect::<Vec<_>>(),
        vec![&busy]
    );
    let open = running.rows[0].run.clone().unwrap();
    assert_eq!(open.phase, Some(RunPhase::Running));
    assert_eq!(running.rows[0].sort_at_ms, open.started_at_ms);
    let fin = page(
        &mut e,
        ActivityFilter::default(),
        ActivityGroup::Finished,
        None,
        10,
    );
    assert_eq!(
        fin.rows.iter().map(|r| &r.session).collect::<Vec<_>>(),
        vec![&done],
        "a root that never ran is not finished"
    );
    // Counts are the same whichever group is paged.
    assert_eq!(fin.counts, running.counts);
    gate.open();
    assert_eq!(e.settled("b1"), RunOutcome::Completed);
}

#[test]
fn pages_are_stable_keysets_and_cursors_are_bound_to_their_group() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let mut created = Vec::new();
    for i in 0..5 {
        created.push(e.create(&fx.project, &format!("k{i}")));
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let p = page(
            &mut e,
            ActivityFilter::default(),
            ActivityGroup::All,
            after,
            2,
        );
        assert!(p.rows.len() <= 2);
        seen.extend(p.rows.iter().map(|r| r.session.clone()));
        // A session created between pages lands before the cursor and never duplicates.
        if seen.len() == 2 {
            e.create(&fx.project, "late");
        }
        match p.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    created.reverse();
    assert_eq!(seen, created, "newest first, each exactly once");

    let p = page(
        &mut e,
        ActivityFilter::default(),
        ActivityGroup::All,
        None,
        1,
    );
    let err = e
        .call(Call::Activity {
            filter: ActivityFilter::default(),
            group: ActivityGroup::Running,
            after: p.next,
            limit: 1,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    let err = e
        .call(Call::Activity {
            filter: ActivityFilter::default(),
            group: ActivityGroup::All,
            after: Some(ActivityCursor("garbage".into())),
            limit: 1,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    // The limit is clamped, not refused.
    assert_eq!(
        page(
            &mut e,
            ActivityFilter::default(),
            ActivityGroup::All,
            None,
            0
        )
        .rows
        .len(),
        1
    );
}

#[test]
fn filters_narrow_rows_and_counts_together() {
    let fx = Fixture::new();
    let other = fx.dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let a = e.create(&fx.project, "a");
    let b = e.create(&other, "b");
    e.call(Call::RenameSession {
        session: a.clone(),
        title: "Refactor the Parser".into(),
    })
    .unwrap();
    let cwd = row(&mut e, &b).cwd;
    let p = page(
        &mut e,
        ActivityFilter {
            cwd: Some(cwd),
            ..Default::default()
        },
        ActivityGroup::All,
        None,
        10,
    );
    assert_eq!(
        p.rows.iter().map(|r| &r.session).collect::<Vec<_>>(),
        vec![&b]
    );
    assert_eq!(p.counts.total, 1);
    let p = page(
        &mut e,
        ActivityFilter {
            text: Some("  parser ".into()),
            ..Default::default()
        },
        ActivityGroup::All,
        None,
        10,
    );
    assert_eq!(
        p.rows.iter().map(|r| &r.session).collect::<Vec<_>>(),
        vec![&a]
    );
    let p = page(
        &mut e,
        ActivityFilter {
            kind: Some(WorkerKind::Child),
            ..Default::default()
        },
        ActivityGroup::All,
        None,
        10,
    );
    assert!(p.rows.is_empty());
    assert_eq!(p.counts.total, 0);
    let p = page(
        &mut e,
        ActivityFilter {
            backend: Some(ExecutionBackend::NativeAgent),
            ..Default::default()
        },
        ActivityGroup::All,
        None,
        10,
    );
    assert_eq!(p.counts.total, 2);
    let err = e
        .call(Call::Activity {
            filter: ActivityFilter {
                text: Some("x".repeat(257)),
                ..Default::default()
            },
            group: ActivityGroup::All,
            after: None,
            limit: 10,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TooLarge);
}

#[test]
fn refetch_reports_removed_sessions_and_is_bounded() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let a = e.create(&fx.project, "a");
    let gone = e.create(&fx.project, "gone");
    e.call(Call::RemoveSession {
        session: gone.clone(),
    })
    .unwrap();
    let Ok(Reply::ActivityRows {
        rows,
        missing,
        through,
    }) = e.call(Call::ActivityRows {
        sessions: vec![a.clone(), gone.clone(), a.clone(), "never".into()],
    })
    else {
        panic!()
    };
    assert_eq!(
        rows.iter().map(|r| &r.session).collect::<Vec<_>>(),
        vec![&a]
    );
    assert_eq!(missing, vec![gone, "never".to_owned()]);
    assert!(through > 0);
    let err = e
        .call(Call::ActivityRows {
            sessions: (0..65).map(|i| i.to_string()).collect(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TooLarge);
}

#[test]
fn a_refetched_row_has_a_newer_revision_than_the_invalidation_that_caused_it() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    let before = all(&mut e);
    let held = before
        .rows
        .iter()
        .find(|r| r.session == s)
        .unwrap()
        .revision;
    e.prompt(&s, "r1", "hello").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let Ok(Reply::Events(events)) = e.call(Call::CatchUp {
        after: before.through,
        limit: 256,
    }) else {
        panic!()
    };
    let newest = events
        .events
        .iter()
        .filter(|ev| ev.session == s)
        .map(|ev| ev.revision)
        .max()
        .expect("the run invalidated the session");
    assert!(newest > held);
    let Ok(Reply::ActivityRows { rows, through, .. }) = e.call(Call::ActivityRows {
        sessions: vec![s.clone()],
    }) else {
        panic!()
    };
    assert!(rows[0].revision >= newest);
    assert!(through >= events.through);
}

#[test]
fn observation_after_a_restart_builds_no_session_runtime() {
    let fx = Fixture::new();
    let provider = Scripted::echo("hi there");
    let (root, child) = {
        let mut e = fx.start(&provider);
        let root = e.create(&fx.project, "root");
        e.prompt(&root, "r1", "hello").unwrap();
        assert_eq!(e.settled("r1"), RunOutcome::Completed);
        let Ok(Reply::Subagent(c)) = e.call(Call::LaunchSubagent {
            session: root.clone(),
            request: "l1".into(),
            task: "look around".into(),
            profile: None,
            context: None,
        }) else {
            panic!()
        };
        until_row(&mut e, &c.session, "child completion", |r| {
            r.state == WorkState::Completed
        });
        (root, c)
    };

    let mut e = fx.start(&provider);
    assert_eq!(live(&e), 0);
    let p = all(&mut e);
    assert_eq!(p.counts.total, 2);
    let r = p.rows.iter().find(|r| r.session == child.session).unwrap();
    assert_eq!(r.kind, WorkerKind::Child);
    assert_eq!(r.root, root);
    assert_eq!(r.parent.as_deref(), Some(root.as_str()));
    assert_eq!(r.child, Some(child.child));
    assert_eq!(r.origin.as_deref(), Some("user"));
    assert_eq!(r.raw_status.as_deref(), Some("done"));
    assert!(r.has_result);
    assert_eq!(r.task_excerpt.as_deref(), Some("look around"));
    let Ok(Reply::ActivityRows { .. }) = e.call(Call::ActivityRows {
        sessions: vec![root.clone(), child.session.clone()],
    }) else {
        panic!()
    };
    let Ok(Reply::Page { entries, .. }) = e.call(Call::ObserveHistory {
        session: root.clone(),
        before: None,
        limit: 50,
    }) else {
        panic!()
    };
    assert!(entries.len() >= 2, "the prompt and the answer");
    let Ok(Reply::Page { entries, .. }) = e.call(Call::ObserveHistory {
        session: child.session.clone(),
        before: None,
        limit: 50,
    }) else {
        panic!()
    };
    assert!(!entries.is_empty(), "a child's own transcript");
    let Ok(Reply::ObservedChildren { rows, next, .. }) = e.call(Call::ObserveChildren {
        session: root.clone(),
        after: None,
        limit: 10,
    }) else {
        panic!()
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session, child.session);
    assert_eq!(next, None);
    let Ok(Reply::ObservedChildren { rows, .. }) = e.call(Call::ObserveChildren {
        session: root.clone(),
        after: Some(child.child),
        limit: 10,
    }) else {
        panic!()
    };
    assert!(rows.is_empty());
    assert_eq!(
        live(&e),
        0,
        "observation never materialises a session runtime"
    );
    let err = e
        .call(Call::ObserveHistory {
            session: "never".into(),
            before: None,
            limit: 10,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn a_parent_counts_its_active_children_without_being_counted_as_running() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |req, _| {
        if req.messages.iter().any(|m| m.text().contains("slow task")) {
            R::Gated(g.clone(), Box::new(R::Text("child done".into())))
        } else {
            R::Text("ok".into())
        }
    });
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    let Ok(Reply::Subagent(c)) = e.call(Call::LaunchSubagent {
        session: root.clone(),
        request: "l1".into(),
        task: "slow task".into(),
        profile: None,
        context: None,
    }) else {
        panic!()
    };
    until_row(&mut e, &c.session, "child running", |r| {
        r.state == WorkState::Running
    });
    let parent = row(&mut e, &root);
    assert_eq!(parent.children_active, 1);
    assert_eq!(parent.state, WorkState::Ready);
    let p = all(&mut e);
    assert_eq!(p.counts.running, 1, "only the child is running");
    gate.open();
    until_row(&mut e, &c.session, "child completion", |r| {
        r.state == WorkState::Completed
    });
    assert_eq!(row(&mut e, &root).children_active, 0);
    assert_eq!(all(&mut e).counts.finished_recent, 1);
}

#[test]
fn a_guarded_stop_never_reaches_another_run() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, _| R::Hang);
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "r1", "work").unwrap();
    let r = until_row(&mut e, &s, "running", |r| r.state == WorkState::Running);
    let run = r.run.unwrap().id;

    let err = e
        .call(Call::StopRun {
            session: s.clone(),
            expected_run: run + 1000,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(
        row(&mut e, &s).state,
        WorkState::Running,
        "nothing was stopped"
    );

    let Ok(Reply::Stopping { run: Some(stopped) }) = e.call(Call::StopRun {
        session: s.clone(),
        expected_run: run,
    }) else {
        panic!()
    };
    assert_eq!(stopped, run);
    assert_eq!(e.settled("r1"), RunOutcome::Aborted);
    assert_eq!(row(&mut e, &s).state, WorkState::Stopped);

    let err = e
        .call(Call::StopRun {
            session: s.clone(),
            expected_run: run,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict, "that run already ended");
    let err = e
        .call(Call::StopRun {
            session: "never".into(),
            expected_run: 1,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn a_failed_run_is_finished_and_findable_as_a_failure() {
    let fx = Fixture::new();
    let provider = Scripted::new(|_, _| R::Error("boom".into()));
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "s");
    e.prompt(&s, "r1", "work").unwrap();
    let outcome = e.settled("r1");
    assert!(!matches!(outcome, RunOutcome::Completed), "{outcome:?}");
    let r = row(&mut e, &s);
    assert!(matches!(r.state, WorkState::Failed { .. }), "{:?}", r.state);
    let p = page(
        &mut e,
        ActivityFilter {
            failed: true,
            ..Default::default()
        },
        ActivityGroup::Finished,
        None,
        10,
    );
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.counts.failed_recent, 1);
}

/// Contracts §9: the directory over 10,000 sessions builds no runtime. Children carry
/// realistic payloads (a 2 KB task, launch instructions, a 16 KB result) so the measurement
/// includes what a lean read must skip. The timing is printed for the record (`--nocapture`);
/// the bound only catches a pathological regression.
#[test]
fn ten_thousand_workers_page_without_building_runtimes() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let cwd = fx.project.display().to_string();
    let shared = e.handle.shared().clone();
    shared
        .store
        .tx(|tx| {
            let now = crate::util::now_ms();
            let task = "Investigate the failing build. ".repeat(68);
            let result = "Found it: the cache key changed. ".repeat(500);
            let launch = serde_json::json!({
                "version": 1, "profile": "worker", "context": "fresh", "createdAtMs": now,
                "instructions": "Be careful and thorough. ".repeat(40),
            })
            .to_string();
            for i in 0..5_000 {
                let root = format!("root-{i:05}");
                let child = format!("child-{i:05}");
                tx.execute(
                    "INSERT INTO sessions(id, cwd, title, created_at, updated_at, origin)
                     VALUES (?1, ?2, ?4, ?3, ?3, '{\"kind\":\"native\"}')",
                    rusqlite::params![root, cwd, now - i, format!("Conversation {i}")],
                )?;
                tx.execute(
                    "INSERT INTO runs(session, state, outcome, started_at, ended_at)
                     VALUES (?1, 'settled', '{\"reason\":\"completed\"}', ?2, ?2)",
                    rusqlite::params![root, now - i],
                )?;
                tx.execute(
                    "INSERT INTO sessions(id, cwd, created_at, updated_at, origin, parent, root)
                     VALUES (?1, ?2, ?3, ?3, '{\"kind\":\"native\"}', ?4, ?4)",
                    rusqlite::params![child, cwd, now - i, root],
                )?;
                tx.execute(
                    "INSERT INTO subagent_tasks(root, child, parent_session, parent_call,
                         child_session, task, status, result, launch, created_at, updated_at)
                     VALUES (?1, 1, ?1, 'call', ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    rusqlite::params![
                        root,
                        child,
                        task,
                        if i % 2 == 0 { "done" } else { "running" },
                        (i % 2 == 0).then_some(&result),
                        launch,
                        now - i
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let started = Instant::now();
    let p = page(
        &mut e,
        ActivityFilter::default(),
        ActivityGroup::Running,
        None,
        200,
    );
    let elapsed = started.elapsed();
    println!("activity page over 10,000 workers: {elapsed:?}");
    assert_eq!(p.counts.total, 10_000);
    assert_eq!(p.counts.running, 2_500);
    assert_eq!(p.counts.finished_recent, 7_500);
    assert_eq!(p.rows.len(), 200);
    let Ok(Reply::ObservedChildren { rows, .. }) = e.call(Call::ObserveChildren {
        session: "root-00001".into(),
        after: None,
        limit: 10,
    }) else {
        panic!()
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(live(&e), 0);
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

#[test]
fn finished_child_follow_up_keeps_full_history_original_result_and_receipt() {
    let fx = Fixture::new();
    let provider = Scripted::echo("original result");
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    let Ok(Reply::Subagent(old)) = e.call(Call::LaunchSubagent {
        session: root.clone(),
        request: "original".into(),
        task: "inspect existing project".into(),
        profile: Some("reviewer".into()),
        context: None,
    }) else {
        panic!()
    };
    until_row(&mut e, &old.session, "original completion", |r| {
        r.state == WorkState::Completed
    });
    let before = e
        .handle
        .shared()
        .store
        .tasks(&root, 100)
        .unwrap()
        .0
        .remove(0);
    let call = Call::FollowUpSubagent {
        session: root.clone(),
        child: old.child,
        request: "next".into(),
        text: "inspect what remains".into(),
    };
    let Ok(Reply::Subagent(next)) = e.call(call.clone()) else {
        panic!("follow-up refused")
    };
    assert_ne!(next.session, old.session);
    until_row(&mut e, &next.session, "follow-up completion", |r| {
        r.state == WorkState::Completed
    });
    let activity = row(&mut e, &next.session);
    assert_eq!(activity.continues, Some(old.child));
    assert_eq!(activity.parent.as_deref(), Some(root.as_str()));
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    let text = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(text.contains("original result"));
    assert!(text.contains("inspect existing project"));
    assert!(text.contains("inspect what remains"));
    let again = e.call(call).unwrap();
    assert!(matches!(again,Reply::Subagent(r) if r.session==next.session));
    assert_eq!(provider.requests().len(), 2);
    let lookup = e
        .call(Call::LookupFollowUpSubagent {
            session: root.clone(),
            child: old.child,
            request: "next".into(),
        })
        .unwrap();
    assert!(matches!(lookup,Reply::Subagent(r) if r.session==next.session));
    let stored = e.handle.shared().store.tasks(&root, 100).unwrap().0;
    assert_eq!(
        stored.iter().find(|t| t.child == old.child).unwrap(),
        &before
    );
    assert!(
        e.call(Call::FollowUpSubagent {
            session: root.clone(),
            child: old.child,
            request: "next".into(),
            text: "changed".into()
        })
        .is_err()
    );
    assert_eq!(provider.requests().len(), 2);
    e.handle
        .shared()
        .store
        .tx(|tx| {
            tx.execute(
                "UPDATE subagent_tasks SET result=?2 WHERE child_session=?1",
                rusqlite::params![
                    old.session,
                    "x".repeat(pipkin_engine_protocol::limits::MAX_PROMPT_BYTES)
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let error = e
        .call(Call::FollowUpSubagent {
            session: root.clone(),
            child: old.child,
            request: "oversize".into(),
            text: "continue".into(),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    assert_eq!(
        e.handle.shared().store.tasks(&root, 100).unwrap().0.len(),
        2,
        "an oversized combined context must not create a stranded child"
    );
    assert_eq!(provider.requests().len(), 2);
}

#[test]
fn dashboard_children_beyond_the_old_directory_cap_are_observable() {
    let fx = Fixture::new();
    let provider = Scripted::echo("unused");
    let mut e = fx.start(&provider);
    let root = e.create(&fx.project, "root");
    e.handle.shared().store.tx(|tx| {
        for i in 1..=4_100i64 {
            let child = format!("child-{i:05}");
            tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,parent,root,origin) VALUES (?1,?2,?3,?3,?4,?4,'{\"kind\":\"native\"}')", rusqlite::params![child,fx.project.display().to_string(),i,root])?;
            tx.execute("INSERT INTO subagent_tasks(root,child,parent_session,parent_call,child_session,task,status,created_at,updated_at) VALUES (?1,?2,?1,?3,?3,'done','done',?2,?2)",rusqlite::params![root,i,child])?;
        }
        Ok(())
    }).unwrap();
    let mut after = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let Ok(Reply::ObservedChildren { rows, next, .. }) = e.call(Call::ObserveChildren {
            session: root.clone(),
            after,
            limit: 500,
        }) else {
            panic!()
        };
        for r in rows {
            assert!(seen.insert(r.session));
        }
        after = next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 4100);
    let mut after = None;
    let mut listed = std::collections::HashSet::new();
    loop {
        let p = page(
            &mut e,
            ActivityFilter {
                kind: Some(WorkerKind::Child),
                ..Default::default()
            },
            ActivityGroup::All,
            after,
            200,
        );
        assert_eq!(p.counts.total, 4100);
        for r in p.rows {
            assert!(listed.insert(r.session));
        }
        after = p.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen, listed);
    assert!(provider.requests().is_empty());
}

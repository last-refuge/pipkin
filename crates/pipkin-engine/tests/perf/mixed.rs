//! Sustained mixed workload: one worker, eight attached conversations, two concurrent turns.
use super::*;

// Also stop the sampler if any assertion unwinds before the explicit join.
struct StopSampler(Arc<AtomicBool>);
impl Drop for StopSampler {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

const SESSIONS: usize = 8;
const REPLY_BYTES: usize = 64 * 1024;
const TOOL_BYTES: usize = 1024 * 1024;

fn verify_latest(e: &mut Engine, sessions: &[String], latest: &[Option<usize>]) {
    for (session, turn) in sessions.iter().zip(latest) {
        let Some(turn) = turn else { continue };
        let (snapshot, _, _) = e.open(session, Some(5));
        let message = snapshot
            .entries
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::Message {
                    message: m @ Message::Assistant { .. },
                    partial: false,
                    ..
                } => Some(m),
                _ => None,
            })
            .expect("saved assistant reply");
        let text = message.text();
        assert_eq!(text.len(), REPLY_BYTES);
        assert_eq!(
            markers_in(&text),
            (turn * 64..turn * 64 + 64)
                .map(|n| n as u64)
                .collect::<Vec<_>>()
        );
        let key = format!("mixed-turn-{turn}");
        // Replay a settled admission after restart. It must never launch another tool/model call.
        assert!(matches!(
            e.call(Call::Prompt {
                session: session.clone(),
                request: key.clone(),
                text: key.clone(),
                images: vec![],
                model: None,
            }),
            Reply::Admitted { .. }
        ));
        assert!(matches!(
            e.call(Call::Lookup {
                session: session.clone(),
                request: key
            }),
            Reply::Lookup(LookupResult::Known {
                open: false,
                outcome: Some(RunOutcome::Completed),
                ..
            })
        ));
    }
    e.backlog.clear();
}

#[test]
#[ignore = "benchmark: release sustained mixed stream/tool/session/restart workload"]
fn mixed_soak() {
    let _g = serial();
    let turns = env_usize("PERF_MIXED_TURNS", 720);
    let restart_every = env_usize("PERF_MIXED_RESTART_EVERY", 240);
    assert!(turns > 0 && turns.is_multiple_of(2));
    assert!(restart_every > 0 && restart_every.is_multiple_of(2));
    let stub = Stub::start(|n, body| {
        let content = &body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["role"] == "user")
            .expect("user prompt")["content"];
        let prompt = content.as_str().map(str::to_owned).unwrap_or_else(|| {
            content
                .as_array()
                .expect("text content blocks")
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect()
        });
        let turn: usize = prompt
            .strip_prefix("mixed-turn-")
            .expect("mixed prompt")
            .parse()
            .unwrap();
        if last_role(body) == "tool" {
            marked_plan(64, 1024, Duration::from_millis(10), turn as u64 * 64)
        } else {
            tool_plan(
                n,
                &format!(
                    "printf '{turn}\\n' >> invocations.log; head -c {TOOL_BYTES} /dev/zero | tr '\\000' x; printf '\\nfinished-{turn}\\n'"
                ),
            )
        }
    });
    let p = Profile::new("mixed", &stub.base_url());
    let spills = p.root.join("spills");
    std::fs::create_dir(&spills).unwrap();
    let spill_dir = spills.to_str().unwrap();
    let spawn = || Engine::spawn_with(&p, &[("TMPDIR", spill_dir)]);
    let mut e = spawn();
    e.hello();
    let mut sessions = Vec::new();
    for i in 0..SESSIONS {
        let Reply::Session { summary } = e.call(Call::CreateSession {
            cwd: p.project.display().to_string(),
            request: format!("mixed-session-{i}"),
        }) else {
            panic!("session creation")
        };
        e.open(&summary.id, Some(5));
        sessions.push(summary.id);
    }
    let mut latest = vec![None; SESSIONS];
    let mut samples = vec![];
    let mut restarts = vec![];
    let mut latencies = vec![];
    let mut epoch = 0;
    let initial = mem_sample(e.pid);
    let watched_pid = Arc::new(AtomicU64::new(e.pid as u64));
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_exit = StopSampler(stop.clone());
    let sampler = {
        let pid = watched_pid.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let (mut rss, mut fds, mut descendants) = (0, 0, 0);
            while !stop.load(Ordering::SeqCst) {
                let pid = pid.load(Ordering::SeqCst) as u32;
                if pid != 0 {
                    rss = rss.max(procfs::rss_kib(pid));
                    fds = fds.max(procfs::fds(pid));
                    descendants = descendants.max(procfs::descendants(pid).len());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            (rss, fds, descendants)
        })
    };
    let started = Instant::now();
    for base in (0..turns).step_by(2) {
        if base > 0 && base % restart_every == 0 {
            watched_pid.store(0, Ordering::SeqCst);
            let before = mem_sample(e.pid);
            let exit = e.shutdown();
            e = spawn();
            e.hello();
            watched_pid.store(e.pid as u64, Ordering::SeqCst);
            let calls = stub.state.count.load(Ordering::SeqCst);
            verify_latest(&mut e, &sessions, &latest);
            assert_eq!(
                stub.state.count.load(Ordering::SeqCst),
                calls,
                "restart replay repeated a provider call"
            );
            epoch += 1;
            restarts.push(json!({"afterTurns":base,"exitMs":ms(exit),"before":before,"after":mem_sample(e.pid)}));
        }
        let began = Instant::now();
        let mut running = HashMap::new();
        for turn in base..base + 2 {
            let slot = turn % SESSIONS;
            let key = format!("mixed-turn-{turn}");
            let run = e.prompt(&sessions[slot], &key, &key);
            running.insert(run, turn);
            latest[slot] = Some(turn);
        }
        e.until(
            Duration::from_secs(120),
            |_| {},
            |a| {
                if let ServerFrame::Event {
                    event:
                        Event::Session {
                            change: SessionChange::RunSettled { run, outcome, .. },
                            ..
                        },
                    ..
                } = &a.frame
                    && let Some(turn) = running.remove(run)
                {
                    assert_eq!(*outcome, RunOutcome::Completed, "mixed turn {turn}");
                }
                running.is_empty()
            },
        );
        e.backlog.clear();
        latencies.push(ms(began.elapsed()));
        if (base + 2) % 16 == 0 || base + 2 == turns {
            std::thread::sleep(Duration::from_millis(100));
            let descendants = procfs::descendants(e.pid);
            assert!(
                descendants.is_empty(),
                "settled tools left descendants: {descendants:?}"
            );
            let sample = json!({"turns":base+2,"epoch":epoch,"mem":mem_sample(e.pid)});
            println!("mixed-soak {sample}");
            samples.push(sample);
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    let final_mem = mem_sample(e.pid);
    assert_eq!(final_mem["fds"], initial["fds"], "idle descriptors grew");
    watched_pid.store(0, Ordering::SeqCst);
    let exit = e.shutdown();
    e = spawn();
    e.hello();
    verify_latest(&mut e, &sessions, &latest);
    assert_eq!(stub.state.count.load(Ordering::SeqCst), (turns * 2) as u64);
    let final_reopen = mem_sample(e.pid);
    let _ = e.shutdown();
    stop.store(true, Ordering::SeqCst);
    let (peak, max_fds, max_descendants) = sampler.join().unwrap();
    let mut invoked: Vec<usize> = std::fs::read_to_string(p.project.join("invocations.log"))
        .unwrap()
        .lines()
        .map(|v| v.parse().unwrap())
        .collect();
    invoked.sort_unstable();
    assert_eq!(
        invoked,
        (0..turns).collect::<Vec<_>>(),
        "tool effect repeated or missing"
    );
    let mut preserved = Vec::new();
    let mut spill_bytes = 0u64;
    for dir in std::fs::read_dir(&spills).unwrap() {
        for file in std::fs::read_dir(dir.unwrap().path()).unwrap() {
            let bytes = std::fs::read(file.unwrap().path()).unwrap();
            assert!(bytes.len() > TOOL_BYTES && bytes[..TOOL_BYTES].iter().all(|b| *b == b'x'));
            let suffix = std::str::from_utf8(&bytes[TOOL_BYTES..]).unwrap();
            let turn: usize = suffix
                .trim()
                .strip_prefix("finished-")
                .unwrap()
                .parse()
                .unwrap();
            preserved.push(turn);
            spill_bytes += bytes.len() as u64;
        }
    }
    preserved.sort_unstable();
    assert_eq!(
        preserved,
        (0..turns).collect::<Vec<_>>(),
        "complete spill output missing"
    );
    let conn = rusqlite::Connection::open_with_flags(
        p.data.join("engine.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, u64>(0)).unwrap();
    assert_eq!(
        count("SELECT count(*) FROM tool_calls WHERE state='done'"),
        turns as u64
    );
    assert_eq!(count("SELECT count(*) FROM runs WHERE state!='settled'"), 0);
    assert_eq!(count("SELECT count(*) FROM entry_chunks"), 0);
    // Validate every stored final reply, not just the latest window after a restart.
    let mut answers = vec![false; turns];
    let mut stmt = conn.prepare("SELECT r.key, e.body FROM requests r JOIN entries e ON e.session=r.session AND e.run=r.run WHERE json_extract(e.body, '$.message.role')='assistant' AND json_extract(e.body, '$.message.stopReason')='stop'").unwrap();
    for row in stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
    {
        let (key, raw) = row.unwrap();
        let turn: usize = key.strip_prefix("mixed-turn-").unwrap().parse().unwrap();
        assert!(!answers[turn], "duplicate final answer");
        answers[turn] = true;
        let EntryKind::Message { message, .. } = serde_json::from_str(&raw).unwrap() else {
            panic!("message")
        };
        let text = message.text();
        assert_eq!(text.len(), REPLY_BYTES);
        assert_eq!(
            markers_in(&text),
            (turn * 64..turn * 64 + 64)
                .map(|n| n as u64)
                .collect::<Vec<_>>()
        );
    }
    assert!(answers.iter().all(|v| *v), "a final answer is missing");
    let mut links = std::collections::HashSet::new();
    let mut stmt = conn.prepare("SELECT e.body FROM tool_calls t JOIN entries e ON e.session=t.session AND e.id=t.result_entry").unwrap();
    for raw in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        let EntryKind::Message {
            message:
                message @ Message::ToolResult {
                    is_error: false, ..
                },
            ..
        } = serde_json::from_str(&raw.unwrap()).unwrap()
        else {
            panic!("successful tool result")
        };
        let text = message.text();
        let link = text
            .lines()
            .find_map(|line| {
                line.split_once("Full output: ")
                    .map(|(_, path)| PathBuf::from(path))
            })
            .expect("complete output link in saved result");
        assert!(link.starts_with(&spills) && link.is_file());
        assert!(
            links.insert(link),
            "tool results must retain distinct output files"
        );
    }
    assert_eq!(links.len(), turns);
    // Full tool output is deliberately retained in SQLite as well as the spill file.
    // Attribute database growth and verify those durable bytes against the real effect.
    let mut retained_outputs = vec![false; turns];
    let mut stmt = conn.prepare("SELECT r.key, o.text, o.retained, o.dropped FROM requests r JOIN tool_calls t ON t.session=r.session AND t.run=r.run JOIN tool_outputs o ON o.session=t.session AND o.call_id=t.call_id").unwrap();
    for row in stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, u64>(3)?,
            ))
        })
        .unwrap()
    {
        let (key, output, retained, dropped) = row.unwrap();
        let turn: usize = key.strip_prefix("mixed-turn-").unwrap().parse().unwrap();
        assert!(!retained_outputs[turn], "duplicate durable tool output");
        retained_outputs[turn] = true;
        assert_eq!(
            output,
            format!("{}\nfinished-{turn}\n", "x".repeat(TOOL_BYTES))
        );
        assert_eq!(retained, output.len() as u64);
        assert_eq!(dropped, 0);
    }
    assert!(retained_outputs.iter().all(|v| *v));
    let database = json!({
        "pageSize":count("PRAGMA page_size"),
        "pages":count("PRAGMA page_count"),
        "freePages":count("PRAGMA freelist_count"),
        "retainedToolBytes":count("SELECT sum(retained) FROM tool_outputs"),
        "entryJsonBytes":count("SELECT sum(length(CAST(body AS BLOB))) FROM entries"),
        "outboxRows":count("SELECT count(*) FROM event_outbox"),
        "finalizedChunks":count("SELECT count(*) FROM entry_chunks"),
    });
    let mut out = header("mixed-soak");
    out["summary"] = json!({"turns":turns,"sessions":SESSIONS,"concurrentTurns":2,
        "replyBytesPerTurn":REPLY_BYTES,"toolBytesPerTurn":TOOL_BYTES,"replyChunkGapMs":10,
        "restarts":restarts.len()+1,"providerRequests":stub.state.count.load(Ordering::SeqCst),
        "uniqueToolEffects":invoked.len(),"verifiedFinalReplies":answers.len(),"verifiedOutputLinks":links.len(),"verifiedCompleteSpillFiles":preserved.len(),"verifiedRetainedToolOutputs":retained_outputs.len(),"spillBytes":spill_bytes,
        "providerRequestBytesTotal":stub.state.request_bytes.load(Ordering::Relaxed),
        "initial":initial,"final":final_mem,"finalReopen":final_reopen,"rssKiBPeakSampled50ms":peak,"maxFds":max_fds,
        "maxToolDescendants":max_descendants,"batchLatencyMs":summary(&latencies),"wallSecs":round3(started.elapsed().as_secs_f64()),
        "lastExitMs":ms(exit),"dbBytes":p.db_bytes().0});
    out["samples"] = json!(samples);
    out["restarts"] = json!(restarts);
    out["database"] = database;
    finish(out, "mixed-soak");
}

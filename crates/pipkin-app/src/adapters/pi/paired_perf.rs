//! Opt-in Linux comparison through both production adapters and actual packaged engines.
//! Profiles live on the configured disk; the test process/provider/UI rendering are excluded.
use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;

pub(crate) trait Desktop {
    fn state(&self) -> &AppState;
    fn dispatch(&mut self, command: Command);
    fn pump(&mut self);
    fn stop(self: Box<Self>);
}

impl Desktop for Harness {
    fn state(&self) -> &AppState {
        &self.state
    }
    fn dispatch(&mut self, command: Command) {
        Harness::dispatch(self, command);
    }
    fn pump(&mut self) {
        Harness::pump(self);
    }
    fn stop(self: Box<Self>) {
        Harness::stop(*self);
    }
}

fn until(client: &mut dyn Desktop, label: &str, done: impl Fn(&AppState) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        client.pump();
        if done(client.state()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: {:?}",
            client.state().notice
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn tagged_pids(marker: &str) -> Vec<u32> {
    let wanted = format!("PIPKIN_PAIRED_ID={marker}");
    let mut pids: Vec<_> = std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|d| {
            let pid = d.file_name().to_str()?.parse::<u32>().ok()?;
            let env = std::fs::read(d.path().join("environ")).ok()?;
            env.split(|b| *b == 0)
                .any(|v| v == wanted.as_bytes())
                .then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

fn keyed_file(path: impl AsRef<Path>) -> BTreeMap<String, u64> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((
                key.to_owned(),
                value.split_whitespace().next()?.parse().ok()?,
            ))
        })
        .collect()
}

fn resources(pids: &[u32], pss: bool) -> Value {
    let mut rss = 0;
    let mut hwm = 0;
    let mut proportional = 0;
    let mut fds = 0;
    let mut ticks = 0;
    let mut write_bytes = 0;
    let mut write_chars = 0;
    for pid in pids {
        let dir = PathBuf::from(format!("/proc/{pid}"));
        let status = keyed_file(dir.join("status"));
        rss += status["VmRSS"];
        hwm += status["VmHWM"];
        fds += std::fs::read_dir(dir.join("fd")).unwrap().count();
        let stat = std::fs::read_to_string(dir.join("stat")).unwrap();
        let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 1..]
            .split_whitespace()
            .collect();
        ticks += fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap();
        let io = keyed_file(dir.join("io"));
        write_bytes += io["write_bytes"];
        write_chars += io["wchar"];
        if pss {
            proportional += keyed_file(dir.join("smaps_rollup"))["Pss"];
        }
    }
    json!({"rssKiB":rss,"sumHighWaterKiB":hwm,"pssKiB":pss.then_some(proportional),
        "fds":fds,"cpuTicks":ticks,"writeBytes":write_bytes,"writeChars":write_chars,
        "processes":pids.len()})
}

struct Sampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    join: Option<thread::JoinHandle<()>>,
}
impl Sampler {
    fn start(pids: Vec<u32>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let (done, high) = (stop.clone(), peak.clone());
        let join = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let rss: u64 = pids
                    .iter()
                    .map(|pid| keyed_file(format!("/proc/{pid}/status"))["VmRSS"])
                    .sum();
                high.fetch_max(rss, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            stop,
            peak,
            join: Some(join),
        }
    }
    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::SeqCst);
        self.join.take().unwrap().join().unwrap();
        self.peak.load(Ordering::SeqCst)
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn measure(native: bool, scratch: &Path, turns: usize) -> Value {
    let reply = "response ".repeat(8192); // 72 KiB, identical in both engines.
    let expected = reply.clone();
    let mut p = super::super::testsupport::prepare(move |_, n| {
        if n % 2 == 0 {
            Reply::Tool {
                lead: None,
                name: "bash".into(),
                args: json!({"command":"printf 'effect\\n' >> effects.txt; head -c 1048576 /dev/zero | tr '\\0' x"}),
            }
        } else {
            Reply::Text(reply.clone())
        }
    });
    // Durable files are on disk; short private runtime socket paths remain in /tmp.
    let disk = tempfile::Builder::new()
        .prefix("pk")
        .tempdir_in(scratch)
        .unwrap();
    let disk_root = disk.path().canonicalize().unwrap();
    let agent = disk_root.join("agent");
    std::fs::create_dir(&agent).unwrap();
    std::fs::copy(
        p.config.agent_dir.as_ref().unwrap().join("models.json"),
        agent.join("models.json"),
    )
    .unwrap();
    p.root = disk_root;
    p.project = p.root.join("project");
    std::fs::create_dir(&p.project).unwrap();
    p.config.agent_dir = Some(agent);
    p.config.cwd = p.root.clone();
    p.config.log_path = p.root.join("engine.log");
    init_project(&p.project);
    std::fs::create_dir(p.root.join("home")).unwrap();
    let marker = p.server_id.clone();
    p.config.env.extend([
        ("PIPKIN_PAIRED_ID".into(), marker.clone()),
        ("HOME".into(), p.root.join("home").display().to_string()),
    ]);
    let mut host = None;
    let started = Instant::now();
    let mut client: Box<dyn Desktop> = if native {
        crate::adapters::native::e2e::paired_performance_client(
            Path::new(&std::env::var("PIPKIN_NATIVE_BINARY").unwrap()),
            &p.root,
            &marker,
        )
    } else {
        let mut pi = EngineHost::new(p.config.clone());
        pi.ensure_running().expect("packaged Pi starts");
        host = Some(pi);
        Box::new(Harness::start(
            direct_config(&p.server_dir, &p.server_id),
            &p.root.join("app.sqlite3"),
        ))
    };
    until(client.as_mut(), "client ready", |s| s.connection.is_ready());
    let readiness_ms = started.elapsed().as_secs_f64() * 1000.0;
    let opened = Instant::now();
    client.dispatch(Command::AddProject(p.project.display().to_string()));
    client.dispatch(Command::NewConversation);
    until(client.as_mut(), "conversation opened", |s| {
        s.current().is_some_and(|c| c.opened)
    });
    client.dispatch(Command::SetModel("stub/scripted".into()));
    until(client.as_mut(), "model selected", |s| {
        s.prefs.model.as_deref() == Some("stub/scripted")
    });
    let open_ms = opened.elapsed().as_secs_f64() * 1000.0;
    thread::sleep(Duration::from_millis(500));
    client.pump();
    let pids = tagged_pids(&marker);
    assert!(!pids.is_empty());
    if native {
        assert_eq!(pids.len(), 1, "native must have one engine worker");
    }
    let idle = resources(&pids, true);
    let sampler = Sampler::start(pids.clone());
    let mut turn_ms = vec![];
    for turn in 1..=turns {
        let start = Instant::now();
        client.dispatch(Command::EditDraft(format!("paired coding turn {turn}")));
        client.dispatch(Command::Submit);
        until(client.as_mut(), "coding turn settled", |s| {
            is_idle(s)
                && assistant_texts(s)
                    .iter()
                    .filter(|t| *t == &expected)
                    .count()
                    == turn
        });
        turn_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(p.provider.requests().len(), turn * 2);
        let last = p.provider.requests().last().unwrap().clone();
        assert!(super::super::testsupport::is_tool_result_turn(&last));
        for message in last["messages"].as_array().unwrap() {
            if message["role"] == "tool" {
                assert_eq!(message["tool_call_id"], "call_1");
            }
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    assert_eq!(call["id"], "call_1");
                }
            }
        }
    }
    let end = resources(&pids, true);
    // SAFETY: sysconf only reads the platform clock tick frequency.
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    assert!(ticks_per_second > 0);
    let cpu_ms = (end["cpuTicks"].as_u64().unwrap() - idle["cpuTicks"].as_u64().unwrap()) as f64
        * 1000.0
        / ticks_per_second as f64;
    let peak = sampler.finish();
    assert_eq!(
        tagged_pids(&marker),
        pids,
        "engine process topology changed"
    );
    assert_eq!(
        std::fs::read_to_string(p.project.join("effects.txt")).unwrap(),
        "effect\n".repeat(turns)
    );
    assert_eq!(
        client
            .state()
            .current()
            .unwrap()
            .items
            .iter()
            .filter(|i| matches!(i.kind, ItemKind::User { .. }))
            .count(),
        turns
    );
    client.stop();
    if let Some(mut pi) = host {
        pi.stop();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !tagged_pids(&marker).is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        tagged_pids(&marker).is_empty(),
        "owned processes left after shutdown"
    );
    json!({"engine":if native {"native"} else {"pi"},"readinessMs":readiness_ms,
        "openMs":open_ms,"idle":idle,"afterCoding":end,"sampledPeakRssKiB":peak,
        "turnMs":turn_ms,"engineCpuMs":cpu_ms,"providerRequests":turns*2,"realBashEffects":turns,
        "exactReplyBytes":expected.len(),"toolOutputBytesPerTurn":1048576,
        "orphanedProcesses":0})
}

fn stats(values: impl Iterator<Item = f64>) -> Value {
    let mut xs: Vec<_> = values.collect();
    xs.sort_by(f64::total_cmp);
    assert!(!xs.is_empty());
    let pct = |p: f64| xs[((p * xs.len() as f64).ceil() as usize).clamp(1, xs.len()) - 1];
    json!({"n":xs.len(),"p50":pct(0.50),"p95":pct(0.95),"min":xs[0],"max":xs[xs.len()-1]})
}

#[test]
#[ignore = "needs two actual release packages; use scripts/bench-paired-engines.sh"]
fn packaged_engines_matched_performance() {
    if cfg!(debug_assertions) {
        panic!("measure release builds only");
    }
    let runs: usize = std::env::var("PERF_PAIRED_RUNS")
        .unwrap_or("20".into())
        .parse()
        .unwrap();
    let turns: usize = std::env::var("PERF_PAIRED_TURNS")
        .unwrap_or("6".into())
        .parse()
        .unwrap();
    assert!(runs >= 2 && turns >= 1);
    let scratch = PathBuf::from(std::env::var("PIPKIN_BENCH_SCRATCH").unwrap());
    let mut rows = vec![];
    for pair in 0..runs {
        for native in [pair % 2 != 0, pair % 2 == 0] {
            let row = measure(native, &scratch, turns);
            println!(
                "pair {pair}: {} ready {:.1}ms idle {}KiB peak {}KiB",
                row["engine"],
                row["readinessMs"].as_f64().unwrap(),
                row["idle"]["rssKiB"],
                row["sampledPeakRssKiB"]
            );
            rows.push(row);
        }
    }
    let summarize = |engine: &str| {
        let selected: Vec<_> = rows.iter().filter(|r| r["engine"] == engine).collect();
        json!({"readinessMs":stats(selected.iter().map(|r| r["readinessMs"].as_f64().unwrap())),
          "openMs":stats(selected.iter().map(|r| r["openMs"].as_f64().unwrap())),
          "engineCpuMs":stats(selected.iter().map(|r| r["engineCpuMs"].as_f64().unwrap())),
          "idleRssKiB":stats(selected.iter().map(|r| r["idle"]["rssKiB"].as_f64().unwrap())),
          "idlePssKiB":stats(selected.iter().map(|r| r["idle"]["pssKiB"].as_f64().unwrap())),
          "sampledPeakRssKiB":stats(selected.iter().map(|r| r["sampledPeakRssKiB"].as_f64().unwrap())),
          "turnMs":stats(selected.iter().flat_map(|r| r["turnMs"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap())))})
    };
    let native = summarize("native");
    let pi = summarize("pi");
    let idle_reduction = 1.0
        - native["idleRssKiB"]["p95"].as_f64().unwrap() / pi["idleRssKiB"]["p95"].as_f64().unwrap();
    let peak_reduction = 1.0
        - native["sampledPeakRssKiB"]["p95"].as_f64().unwrap()
            / pi["sampledPeakRssKiB"]["p95"].as_f64().unwrap();
    let gates = json!({"warmReady250ms":native["readinessMs"]["p95"].as_f64().unwrap()<=250.0,
        "idle40MiB":native["idleRssKiB"]["max"].as_f64().unwrap()<=40.0*1024.0,
        "sampledPeak100MiB":native["sampledPeakRssKiB"]["max"].as_f64().unwrap()<=100.0*1024.0,
        "idleRssReduction80Percent":idle_reduction>=0.80,"peakRssReduction80Percent":peak_reduction>=0.80});
    let passed = gates.as_object().unwrap().values().all(|v| v == true);
    let report = json!({"scope":"Headless production adapters/core/storage and packaged engines; fresh on-disk profiles, warm executable/filesystem cache. Provider/test process, desktop rendering and tool subprocess memory excluded. RSS sums shared pages per process; PSS reported separately. Peak sampled every 5ms over fixed engine PIDs; kernel per-process high-water sums also recorded. Client readiness includes adapter connection/storage startup; turn latency is submit-to-core-idle, not chunk-to-commit latency. No cold boot, GUI or live-provider claim.",
        "runsPerEngine":runs,"turnsPerRun":turns,"native":native,"pi":pi,"rows":rows,
        "idleRssReductionPercent":idle_reduction*100.0,"peakRssReductionPercent":peak_reduction*100.0,
        "gates":gates,"passed":passed});
    let output = PathBuf::from(std::env::var("PIPKIN_BENCH_OUT").unwrap());
    std::fs::write(
        output.join("paired-engines.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
    assert!(passed, "paired performance gate missed: {gates}");
}

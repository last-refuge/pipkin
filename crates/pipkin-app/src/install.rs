//! The installed layout: where the bundled Pi engine lives, whether it is usable, and the
//! `--version` / `--diagnose` reports that say so.
//!
//! An installation is `<prefix>/bin/pipkin` plus `<prefix>/lib/pipkin/engine/`, a self-contained
//! copy of the Pi engine with an `engine.json` manifest. Nothing here reads a source checkout.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_REVISION: &str = env!("PIPKIN_APP_REVISION");
pub const APP_DIRTY: &str = env!("PIPKIN_APP_DIRTY");
pub const ENGINE_DIR_ENV: &str = "PIPKIN_ENGINE_DIR";
pub const MANIFEST_FILE: &str = "engine.json";
/// The oldest Node the engine runs on (it loads TypeScript directly).
pub const NODE_MIN: (u32, u32) = (22, 19);
/// The Chord/Pi protocol revision this build speaks. An engine built for another one is refused.
pub const PROTOCOL: u32 = 8;

/// What `engine.json` says about a bundled engine.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,
    /// The engine's own version (a Pi commit or release).
    pub version: String,
    pub protocol: u32,
    /// The first Pipkin version this engine supports.
    #[serde(default)]
    pub min_client: Option<String>,
    #[serde(default)]
    pub built_at: Option<String>,
}

/// Directories searched for a bundled engine, best first: the environment override, then the
/// layout relative to the running binary (`../lib/pipkin/engine`, so a relocated prefix works),
/// then the system locations.
pub fn engine_search_dirs(exe: Option<&Path>, env: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = env.filter(|d| !d.as_os_str().is_empty()) {
        dirs.push(dir);
    }
    if let Some(prefix) = exe
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
    {
        dirs.push(prefix.join("lib/pipkin/engine"));
    }
    for fixed in ["/usr/lib/pipkin/engine", "/usr/local/lib/pipkin/engine"] {
        let fixed = PathBuf::from(fixed);
        if !dirs.contains(&fixed) {
            dirs.push(fixed);
        }
    }
    dirs
}

/// Why a directory is not a usable engine, or its manifest when it is.
pub fn check_engine(dir: &Path) -> Result<Manifest, String> {
    let manifest_path = dir.join(MANIFEST_FILE);
    // A Pi checkout (what `--pi-repo` takes) has no manifest: it is the development entry point,
    // accepted as it is when it can be launched, and reported as a checkout.
    if !manifest_path.exists()
        && dir.join("pi-test.sh").is_file()
        && dir.join("node_modules").is_dir()
    {
        return Ok(Manifest {
            name: "pi (development checkout)".into(),
            version: "unversioned".into(),
            protocol: PROTOCOL,
            min_client: None,
            built_at: None,
        });
    }
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid manifest: {e}", manifest_path.display()))?;
    if manifest.protocol != PROTOCOL {
        return Err(format!(
            "the engine at {} speaks protocol {}, but this Pipkin needs {PROTOCOL}",
            dir.display(),
            manifest.protocol
        ));
    }
    if let Some(min) = &manifest.min_client
        && version_less(VERSION, min)
    {
        return Err(format!(
            "the engine at {} needs Pipkin {min} or newer; this is {VERSION}",
            dir.display()
        ));
    }
    for needed in ["pi-test.sh", "node_modules"] {
        if !dir.join(needed).exists() {
            return Err(format!(
                "the engine at {} is incomplete: {needed} is missing",
                dir.display()
            ));
        }
    }
    Ok(manifest)
}

/// The first usable bundled engine, or the reasons each candidate was rejected.
pub fn find_engine(candidates: &[PathBuf]) -> Result<(PathBuf, Manifest), Vec<String>> {
    let mut reasons = Vec::new();
    for dir in candidates {
        if !dir.exists() {
            reasons.push(format!("{}: not present", dir.display()));
            continue;
        }
        match check_engine(dir) {
            Ok(manifest) => return Ok((dir.clone(), manifest)),
            Err(reason) => reasons.push(reason),
        }
    }
    Err(reasons)
}

/// Dotted numeric versions compared by component; non-numeric parts count as zero.
fn version_less(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split(['.', '-', '+'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (mut a, mut b) = (parts(a), parts(b));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a < b
}

/// `node --version` as `(major, minor, patch)`.
pub fn node_version(engine: Option<&Path>) -> Result<(u32, u32, u32), String> {
    let bundled = engine
        .map(crate::adapters::pi::engine::bundled_runtime_bin)
        .transpose()
        .map_err(|e| e.to_string())?
        .flatten();
    let node = bundled.map_or_else(|| PathBuf::from("node"), |bin| bin.join("node"));
    let output = Command::new(node)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            format!(
                "node was not found ({e}); install Node.js {}.{} or newer",
                NODE_MIN.0, NODE_MIN.1
            )
        })?;
    parse_node_version(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_node_version(text: &str) -> Result<(u32, u32, u32), String> {
    let mut nums = text
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u32>());
    match (nums.next(), nums.next(), nums.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c))) => Ok((a, b, c)),
        _ => Err(format!(
            "cannot read the Node version from {:?}",
            text.trim()
        )),
    }
}

pub fn node_is_new_enough(version: (u32, u32, u32)) -> bool {
    (version.0, version.1) >= NODE_MIN
}

/// Replace the home directory with `~` so a pasted report does not carry the user name.
pub fn redact_home(text: &str, home: Option<&Path>) -> String {
    match home.and_then(Path::to_str).filter(|h| h.len() > 1) {
        Some(home) => text.replace(home, "~"),
        None => text.to_owned(),
    }
}

/// The last `lines` lines of a log file, or why there are none.
pub fn log_tail(path: &Path, lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(e) => format!("({e})"),
    }
}

/// What the report needs to know; gathered by the caller so the text is a pure function.
pub struct ReportInput {
    pub exe: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub schema_version: u32,
    pub engine: Result<(PathBuf, Manifest), Vec<String>>,
    /// An explicit `--pi-repo` or `PIPKIN_ENGINE_DIR` in use, which wins over the bundled one.
    pub engine_override: Option<PathBuf>,
    pub node: Result<(u32, u32, u32), String>,
    pub wayland_display: bool,
    pub display_vars: Vec<(String, String)>,
    pub database: Result<String, String>,
    pub probe: Option<Result<String, String>>,
    pub engine_log_tail: String,
    pub home: Option<PathBuf>,
    /// A native-candidate install marker next to the binary, if there is one.
    pub native_marker: Option<PathBuf>,
}

fn display_lines(wayland_display: bool, display_vars: &[(String, String)]) -> Vec<String> {
    let mut lines = vec![format!(
        "display: {}",
        if cfg!(target_os = "macos") {
            "macOS native (display not probed)"
        } else if wayland_display {
            "Wayland"
        } else {
            "no WAYLAND_DISPLAY (a window cannot open here)"
        }
    )];
    lines.extend(display_vars.iter().map(|(k, v)| format!("  {k}={v}")));
    lines
}

pub fn report(input: &ReportInput) -> String {
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    line(format!(
        "Pipkin {VERSION} (protocol {PROTOCOL}, schema {})",
        input.schema_version
    ));
    line(format!("app source: {APP_REVISION} (dirty: {APP_DIRTY})"));
    line(format!(
        "binary: {}",
        input
            .exe
            .as_deref()
            .map_or("unknown".into(), |p| p.display().to_string())
    ));
    line(format!("data directory: {}", input.data_dir.display()));
    match &input.database {
        Ok(summary) => line(format!("database: {summary}")),
        Err(error) => line(format!("database: PROBLEM {error}")),
    }
    if let Some(dir) = &input.engine_override {
        line(format!("engine override: {}", dir.display()));
    }
    match &input.engine {
        Ok((dir, m)) => line(format!(
            "engine: {} {} at {} (protocol {})",
            m.name,
            m.version,
            dir.display(),
            m.protocol
        )),
        Err(reasons) => {
            line("engine: NOT FOUND".into());
            for reason in reasons {
                line(format!("  - {reason}"));
            }
        }
    }
    match &input.node {
        Ok(v) if node_is_new_enough(*v) => line(format!("node: {}.{}.{}", v.0, v.1, v.2)),
        Ok(v) => line(format!(
            "node: PROBLEM {}.{}.{} is older than {}.{}",
            v.0, v.1, v.2, NODE_MIN.0, NODE_MIN.1
        )),
        Err(e) => line(format!("node: PROBLEM {e}")),
    }
    for text in display_lines(input.wayland_display, &input.display_vars) {
        line(text);
    }
    if let Some(marker) = input
        .native_marker
        .as_ref()
        .filter(|_| input.engine.is_err())
    {
        line(format!(
            "hint: this installation is a native-engine candidate ({}); run `pipkin --diagnose --engine native`",
            marker.display()
        ));
    }
    match &input.probe {
        Some(Ok(text)) => line(format!("engine probe: OK {text}")),
        Some(Err(e)) => line(format!("engine probe: FAILED {e}")),
        None => {
            line("engine probe: not run (add --probe to start the engine and shake hands)".into())
        }
    }
    if !input.engine_log_tail.is_empty() {
        line("engine log (last lines):".into());
        for l in input.engine_log_tail.lines() {
            line(format!("  {l}"));
        }
    }
    redact_home(&out, input.home.as_deref())
}

/// Gather the facts and render the `--diagnose` report. With `probe`, a throwaway engine is
/// started in a scratch profile (offline, no credentials), asked for a handshake, and stopped;
/// the person's own profile and sessions are not touched.
pub fn diagnose(data_dir: &Path, engine_override: Option<&Path>, probe: bool) -> (String, bool) {
    let exe = std::env::current_exe().ok();
    let mut candidates = engine_search_dirs(
        exe.as_deref(),
        std::env::var_os(ENGINE_DIR_ENV).map(PathBuf::from),
    );
    if let Some(dir) = engine_override {
        candidates.insert(0, dir.to_path_buf());
    }
    let engine = find_engine(&candidates);
    let probe_result = probe.then(|| match &engine {
        Ok((dir, _)) => probe_engine(dir),
        Err(_) => Err("no engine to probe".into()),
    });
    let node = node_version(engine.as_ref().ok().map(|(dir, _)| dir.as_path()));
    let exe_path = exe.clone();
    let input = ReportInput {
        exe,
        data_dir: data_dir.to_path_buf(),
        schema_version: crate::storage::SCHEMA_VERSION,
        engine,
        engine_override: engine_override.map(Path::to_path_buf),
        node,
        wayland_display: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        display_vars: ["WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"]
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect(),
        database: crate::storage::inspect(&crate::platform::db_path(data_dir)),
        probe: probe_result,
        engine_log_tail: Some(log_tail(&data_dir.join("engine.log"), 12))
            .filter(|t| !t.starts_with('('))
            .unwrap_or_default(),
        home: std::env::var_os("HOME").map(PathBuf::from),
        native_marker: native_marker(exe_path.as_deref()),
    };
    let text = report(&input);
    let bad = has_problem(&text);
    (text, bad)
}

/// Start the engine from `dir` in a scratch profile, wait for a trusted handshake, stop it.
fn probe_engine(dir: &Path) -> Result<String, String> {
    use crate::adapters::pi::engine::{Engine, EngineConfig};
    // Unix socket paths are short; keep the scratch profile directly under the temp directory.
    let root = std::env::temp_dir().join(format!("pipkin-probe-{}", std::process::id()));
    let server_dir = root.join("server");
    let agent_dir = root.join("agent");
    std::fs::create_dir_all(&agent_dir).map_err(|e| e.to_string())?;
    let server_id = crate::platform::fresh_server_id().map_err(|e| e.to_string())?;
    let config = EngineConfig {
        pi_repo: dir.to_path_buf(),
        server_dir,
        server_id,
        agent_dir: Some(agent_dir),
        cwd: root.clone(),
        model: None,
        log_path: root.join("engine.log"),
        env: vec![("PI_OFFLINE".into(), "1".into())],
        extensions: vec![],
    };
    let started = Instant::now();
    let result = Engine::spawn(&config)
        .map_err(|e| format!("{e:?}"))
        .and_then(|mut engine| {
            let ready = engine
                .wait_ready(Duration::from_secs(45))
                .map_err(|e| format!("{e:?}"));
            engine.stop();
            ready
        })
        .map(|()| format!("(ready in {:.1}s)", started.elapsed().as_secs_f32()));
    let _ = std::fs::remove_dir_all(&root);
    result
}

/// Whether the report found anything that stops Pipkin working.
pub fn has_problem(report: &str) -> bool {
    report.contains("PROBLEM") || report.contains("NOT FOUND") || report.contains("FAILED")
}

// ---------------------------------------------------------------------------------------------
// The built-in (native) engine: `--engine native`. It is compiled into this binary and runs as
// `pipkin --engine-worker`, so there is no engine directory, manifest, Node or Pi to find.
// ---------------------------------------------------------------------------------------------

/// Written by `scripts/package-native.sh` at `<prefix>/lib/pipkin/native-engine.json`. It only
/// identifies a native-candidate package (for the installer and for a diagnostic hint); it never
/// changes which engine runs. Pi stays the default; native is always selected explicitly.
pub const NATIVE_MARKER: &str = "native-engine.json";

/// The built-in engine's identity, from the constants of the crates linked into this binary.
pub struct NativeIdentity {
    pub version: &'static str,
    pub protocol: u32,
    pub schema: u32,
}

pub fn native_identity() -> NativeIdentity {
    NativeIdentity {
        version: pipkin_engine::runtime::ENGINE_VERSION,
        protocol: pipkin_engine_protocol::PROTOCOL_VERSION,
        schema: pipkin_engine::store::SCHEMA_VERSION,
    }
}

/// `pipkin --version --engine native`. The prefix matches the default `--version` line, so the
/// existing parsers keep working; the engine fields are appended before the desktop schema.
pub fn native_version_line(desktop_schema: u32) -> String {
    let id = native_identity();
    format!(
        "pipkin {VERSION} (source {APP_REVISION}, dirty: {APP_DIRTY}, engine native {}, engine protocol {}, engine schema {}, schema {desktop_schema})",
        id.version, id.protocol, id.schema
    )
}

/// The native-candidate marker installed beside this binary, if any.
pub fn native_marker(exe: Option<&Path>) -> Option<PathBuf> {
    let marker = exe?
        .parent()?
        .parent()?
        .join("lib/pipkin")
        .join(NATIVE_MARKER);
    marker.is_file().then_some(marker)
}

/// What the native report needs; gathered by the caller so the text is a pure function.
pub struct NativeReportInput {
    pub exe: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub schema_version: u32,
    pub engine_data_dir: PathBuf,
    pub engine_config_dir: PathBuf,
    pub isolated: bool,
    pub marker: Option<PathBuf>,
    pub database: Result<String, String>,
    pub wayland_display: bool,
    pub display_vars: Vec<(String, String)>,
    pub probe: Option<Result<String, String>>,
    pub engine_log_tail: String,
    pub home: Option<PathBuf>,
}

pub fn native_report(input: &NativeReportInput) -> String {
    let id = native_identity();
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    line(format!(
        "Pipkin {VERSION} (desktop schema {}) with --engine native",
        input.schema_version
    ));
    line(format!("app source: {APP_REVISION} (dirty: {APP_DIRTY})"));
    line(format!(
        "binary: {}",
        input
            .exe
            .as_deref()
            .map_or("unknown".into(), |p| p.display().to_string())
    ));
    line(format!(
        "engine: native (built into this binary) {} (protocol {}, schema {})",
        id.version, id.protocol, id.schema
    ));
    line("engine runtime: `pipkin --engine-worker`; no Node, npm or Pi engine is used".into());
    line(format!(
        "installed as: {}",
        input.marker.as_deref().map_or_else(
            || "no native-candidate marker (a development or Pi build)".to_owned(),
            |m| format!("native candidate ({})", m.display())
        )
    ));
    line(format!("data directory: {}", input.data_dir.display()));
    match &input.database {
        Ok(summary) => line(format!("database: {summary}")),
        Err(error) => line(format!("database: PROBLEM {error}")),
    }
    let presence = |dir: &Path| {
        if dir.is_dir() {
            "present"
        } else {
            "not created yet"
        }
    };
    line(format!(
        "engine data directory: {} ({})",
        input.engine_data_dir.display(),
        presence(&input.engine_data_dir)
    ));
    line(format!(
        "engine config directory: {} ({})",
        input.engine_config_dir.display(),
        presence(&input.engine_config_dir)
    ));
    line(format!(
        "engine isolation: {}",
        if input.isolated {
            "everything under the explicit data directory (--data-dir or PIPKIN_DATA)"
        } else {
            "default per-user directories, separate from Pi's ~/.pi"
        }
    ));
    line("note: history written by the native engine is native-only; Pi does not read it.".into());
    for text in display_lines(input.wayland_display, &input.display_vars) {
        line(text);
    }
    match &input.probe {
        Some(Ok(text)) => line(format!("engine probe: OK {text}")),
        Some(Err(e)) => line(format!("engine probe: FAILED {e}")),
        None => line(
            "engine probe: not run (add --probe to start a throwaway native worker and shake hands)"
                .into(),
        ),
    }
    if !input.engine_log_tail.is_empty() {
        line("engine log (last lines):".into());
        for l in input.engine_log_tail.lines() {
            line(format!("  {l}"));
        }
    }
    redact_home(&out, input.home.as_deref())
}

/// `pipkin --diagnose [--probe] --engine native`. With `probe`, a throwaway worker is started
/// from this executable in a scratch profile (no credentials, no network, cleared environment),
/// asked for Hello and ListSessions, and stopped; the person's own engine data is not touched.
pub fn diagnose_native(cli_data_dir: Option<&Path>, probe: bool) -> (String, bool) {
    let exe = std::env::current_exe().ok();
    let data_dir = crate::platform::data_dir(cli_data_dir);
    let dirs = crate::platform::engine_dirs(cli_data_dir);
    let probe_result = probe.then(|| {
        let root = scratch_dir("pipkin-native-probe")?;
        let result = crate::adapters::native::supervisor::WorkerSpec::engine(
            &root.join("engine"),
            &root.join("engine-config"),
            &[],
            false,
            &root.join("engine.log"),
        )
        .and_then(|spec| probe_native(&spec, &root, Duration::from_secs(30)));
        let _ = std::fs::remove_dir_all(&root);
        result
    });
    let input = NativeReportInput {
        marker: native_marker(exe.as_deref()),
        exe,
        isolated: cli_data_dir.is_some()
            || std::env::var_os(crate::platform::DATA_DIR_ENV).is_some_and(|v| !v.is_empty()),
        database: crate::storage::inspect(&crate::platform::db_path(&data_dir)),
        engine_log_tail: Some(log_tail(
            &crate::adapters::native::engine_log_path(&data_dir),
            12,
        ))
        .filter(|t| !t.starts_with('('))
        .unwrap_or_default(),
        data_dir,
        schema_version: crate::storage::SCHEMA_VERSION,
        engine_data_dir: dirs.data,
        engine_config_dir: dirs.config,
        wayland_display: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        display_vars: ["WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"]
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect(),
        probe: probe_result,
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    let text = native_report(&input);
    let bad = has_problem(&text);
    (text, bad)
}

/// Diagnose the selected external endpoint. The optional probe performs only Hello and
/// ListSessions, reports a count, and disconnects; it never spawns or stops an engine.
pub fn diagnose_native_attachment(endpoint: &Path, probe: bool) -> (String, bool) {
    use crate::adapters::native::client::Client;
    use crate::adapters::native::supervisor::{HostSpec, Supervisor};
    use pipkin_engine_protocol::{Call, PROTOCOL_VERSION, Reply};
    let mut text = format!(
        "{}\nExternal endpoint: {}\nOwnership: external (disconnect only)\n",
        native_version_line(crate::storage::SCHEMA_VERSION),
        endpoint.display()
    );
    if !probe {
        text.push_str("Probe: not requested\n");
        return (text, false);
    }
    let mut supervisor = Supervisor::new(HostSpec::External(endpoint.to_path_buf()));
    let result = (|| -> Result<String, String> {
        let link = supervisor.start()?;
        let client = Client::start(link, |_| {});
        let result = (|| {
            let welcome = client
                .call(
                    Call::Hello {
                        protocol: PROTOCOL_VERSION,
                        client: "pipkin-diagnose".into(),
                    },
                    Duration::from_secs(5),
                )
                .map_err(|e| e.to_string())?;
            if !matches!(welcome, Reply::Welcome(_)) {
                return Err("unexpected Hello reply".into());
            }
            match client
                .call(Call::ListSessions, Duration::from_secs(5))
                .map_err(|e| e.to_string())?
            {
                Reply::Sessions { sessions } => {
                    Ok(format!("connected; {} sessions", sessions.len()))
                }
                _ => Err("unexpected session-list reply".into()),
            }
        })();
        client.close();
        supervisor.stop();
        client.join_reader(Duration::from_secs(2));
        result
    })();
    match result {
        Ok(detail) => {
            text.push_str(&format!("Probe: OK ({detail})\n"));
            (text, false)
        }
        Err(error) => {
            text.push_str(&format!("Probe: FAILED ({error})\n"));
            (text, true)
        }
    }
}

/// A new private directory under the temp directory; never an existing one.
fn scratch_dir(prefix: &str) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let root = std::env::temp_dir().join(format!("{prefix}-{}-{nanos:x}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&root)
        .map_err(|e| format!("cannot create a scratch profile at {}: {e}", root.display()))?;
    Ok(root)
}

/// Start `worker` (a native engine worker whose directories are inside `root`) with a cleared
/// environment, check Hello and ListSessions, ask it to shut down and wait for it to exit.
pub fn probe_native(
    worker: &crate::adapters::native::supervisor::WorkerSpec,
    root: &Path,
    timeout: Duration,
) -> Result<String, String> {
    use pipkin_engine_protocol::frame;
    use pipkin_engine_protocol::{Call, ClientFrame, Lane, PROTOCOL_VERSION, Reply, ServerFrame};
    use std::io::{BufReader, BufWriter};
    use std::sync::mpsc;

    let started = Instant::now();
    let deadline = started + timeout;
    let home = root.join("home");
    std::fs::create_dir_all(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    let log = std::fs::File::create(&worker.log_path)
        .map_err(|e| format!("{}: {e}", worker.log_path.display()))?;
    // No inherited credentials, proxies or API keys: the probe needs none and must not use any.
    let mut child = Command::new(&worker.program)
        .args(&worker.args)
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", root)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("cannot start the native engine worker: {e}"))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("the native engine worker has no private channel".into());
    };
    let (tx, rx) = mpsc::channel::<Result<ServerFrame, String>>();
    std::thread::spawn(move || {
        let mut input = BufReader::new(stdout);
        loop {
            let frame = frame::read::<ServerFrame>(&mut input).map_err(|e| e.to_string());
            let end = frame.is_err();
            if tx.send(frame).is_err() || end {
                break;
            }
        }
    });
    let mut output = BufWriter::new(stdin);
    let mut send = move |id: u64, lane: Lane, call: Call| {
        frame::write(&mut output, &ClientFrame { id, lane, call })
            .map_err(|e| format!("cannot write to the native engine worker: {e}"))
    };
    let await_reply = |id: u64, deadline: Instant| -> Result<Reply, String> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Ok(ServerFrame::Reply { id: got, result })) if got == id => {
                    return result.map_err(|e| format!("{:?}: {}", e.code, e.message));
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(format!("the worker's output ended ({e})")),
                Err(_) => return Err(format!("no reply within {}s", timeout.as_secs())),
            }
        }
    };
    let exchange = (|| {
        send(
            1,
            Lane::Normal,
            Call::Hello {
                protocol: PROTOCOL_VERSION,
                client: format!("pipkin-diagnose {VERSION}"),
            },
        )?;
        let welcome = match await_reply(1, deadline)? {
            Reply::Welcome(w) => w,
            other => return Err(format!("hello was answered with {other:?}")),
        };
        let id = native_identity();
        if welcome.protocol != id.protocol || welcome.schema != id.schema {
            return Err(format!(
                "the worker speaks protocol {} / schema {}, but this build expects {} / {}",
                welcome.protocol, welcome.schema, id.protocol, id.schema
            ));
        }
        send(2, Lane::Normal, Call::ListSessions)?;
        let sessions = match await_reply(2, deadline)? {
            Reply::Sessions { sessions } => sessions.len(),
            other => return Err(format!("ListSessions was answered with {other:?}")),
        };
        if sessions != 0 {
            return Err(format!(
                "a fresh scratch profile listed {sessions} sessions"
            ));
        }
        // An orderly stop: ask, then close the input (the worker exits when it closes).
        let _ = send(3, Lane::Control, Call::Shutdown);
        let _ = await_reply(3, Instant::now() + Duration::from_secs(5).min(timeout));
        Ok(welcome)
    })();
    drop(send);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline + Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("the worker did not exit after shutdown and was stopped".to_owned());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("cannot check the worker: {e}"));
            }
        }
    };
    let failure = |reason: String| {
        let tail = log_tail(&worker.log_path, 6);
        if tail.trim().is_empty() || tail.starts_with('(') {
            reason
        } else {
            format!("{reason}; worker log: {}", tail.replace('\n', " | "))
        }
    };
    let welcome = exchange.map_err(failure)?;
    match exit {
        Ok(status) if status.success() => {}
        Ok(status) => return Err(failure(format!("the worker exited with {status}"))),
        Err(e) => return Err(failure(e)),
    }
    Ok(format!(
        "(hello: protocol {}, engine {} revision {}, schema {}; sessions in a fresh profile: 0; stopped cleanly in {:.1}s)",
        welcome.protocol,
        welcome.engine_version,
        welcome.engine_revision,
        welcome.schema,
        started.elapsed().as_secs_f32()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_dir(root: &Path, protocol: u32, min_client: Option<&str>) -> PathBuf {
        let dir = root.join("engine");
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("pi-test.sh"), "#!/bin/sh\n").unwrap();
        let min = min_client.map_or(String::new(), |m| format!(r#","minClient":"{m}""#));
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(r#"{{"name":"pi","version":"abc123","protocol":{protocol}{min}}}"#),
        )
        .unwrap();
        dir
    }

    #[test]
    fn search_order_is_override_then_relative_to_the_binary_then_system() {
        let dirs = engine_search_dirs(
            Some(Path::new("/opt/p/bin/pipkin")),
            Some(PathBuf::from("/x")),
        );
        assert_eq!(dirs[0], PathBuf::from("/x"));
        assert_eq!(dirs[1], PathBuf::from("/opt/p/lib/pipkin/engine"));
        assert!(dirs.contains(&PathBuf::from("/usr/lib/pipkin/engine")));
        let none = engine_search_dirs(None, Some(PathBuf::new()));
        assert_eq!(
            none[0],
            PathBuf::from("/usr/lib/pipkin/engine"),
            "an empty override is ignored"
        );
    }

    #[test]
    fn a_complete_engine_with_the_right_protocol_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = engine_dir(tmp.path(), PROTOCOL, None);
        let manifest = check_engine(&dir).unwrap();
        assert_eq!(manifest.version, "abc123");
    }

    #[test]
    fn engines_that_cannot_work_are_rejected_with_a_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let wrong = engine_dir(&tmp.path().join("a"), PROTOCOL + 1, None);
        assert!(check_engine(&wrong).unwrap_err().contains("protocol"));
        let newer = engine_dir(&tmp.path().join("b"), PROTOCOL, Some("999.0.0"));
        assert!(
            check_engine(&newer)
                .unwrap_err()
                .contains("needs Pipkin 999.0.0")
        );
        let incomplete = engine_dir(&tmp.path().join("c"), PROTOCOL, None);
        std::fs::remove_dir_all(incomplete.join("node_modules")).unwrap();
        assert!(
            check_engine(&incomplete)
                .unwrap_err()
                .contains("node_modules is missing")
        );
        assert!(check_engine(&tmp.path().join("none")).is_err());
        // A checkout without a manifest is accepted as a development engine.
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir_all(checkout.join("node_modules")).unwrap();
        std::fs::write(checkout.join("pi-test.sh"), "#!/bin/sh\n").unwrap();
        assert!(check_engine(&checkout).unwrap().name.contains("checkout"));
        std::fs::write(tmp.path().join("bad").with_extension("json"), "x").unwrap();
        let broken = tmp.path().join("d");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(MANIFEST_FILE), "{not json").unwrap();
        assert!(
            check_engine(&broken)
                .unwrap_err()
                .contains("not a valid manifest")
        );
    }

    #[test]
    fn the_first_usable_candidate_wins_and_rejections_are_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let good = engine_dir(&tmp.path().join("g"), PROTOCOL, None);
        let bad = engine_dir(&tmp.path().join("w"), 1, None);
        let missing = tmp.path().join("missing");
        let found = find_engine(&[missing.clone(), bad.clone(), good.clone()]).unwrap();
        assert_eq!(found.0, good);
        let reasons = find_engine(&[missing, bad]).unwrap_err();
        assert_eq!(reasons.len(), 2);
        assert!(reasons[0].contains("not present"));
    }

    #[test]
    fn versions_compare_by_component_not_text() {
        assert!(version_less("0.9.0", "0.10.0"));
        assert!(version_less("0.0.1", "0.0.2"));
        assert!(!version_less("1.0", "1.0.0"));
        assert!(!version_less("2.0.0", "1.9.9"));
    }

    #[test]
    fn node_versions_parse_and_are_compared_to_the_minimum() {
        assert_eq!(parse_node_version("v22.19.0\n").unwrap(), (22, 19, 0));
        assert!(parse_node_version("nonsense").is_err());
        assert!(node_is_new_enough((22, 19, 0)));
        assert!(node_is_new_enough((26, 8, 1)));
        assert!(!node_is_new_enough((22, 18, 9)));
        assert!(!node_is_new_enough((20, 0, 0)));
    }

    #[test]
    fn a_report_hides_the_home_directory_and_flags_problems() {
        let input = ReportInput {
            exe: Some(PathBuf::from("/home/ann/bin/pipkin")),
            data_dir: PathBuf::from("/home/ann/.local/share/pipkin"),
            schema_version: 7,
            engine: Err(vec!["/usr/lib/pipkin/engine: not present".into()]),
            engine_override: None,
            node: Ok((20, 1, 0)),
            wayland_display: false,
            display_vars: vec![],
            database: Ok("ok".into()),
            probe: None,
            engine_log_tail: String::new(),
            home: Some(PathBuf::from("/home/ann")),
            native_marker: Some(PathBuf::from("/home/ann/lib/pipkin/native-engine.json")),
        };
        let text = report(&input);
        assert!(text.contains("--diagnose --engine native"), "{text}");
        assert!(!text.contains("/home/ann"), "{text}");
        assert!(text.contains("~/.local/share/pipkin"));
        assert!(text.contains("engine: NOT FOUND"));
        assert!(text.contains("older than 22.19"));
        assert!(has_problem(&text));
        assert!(!has_problem("Pipkin 0.0.1\nengine probe: OK"));
    }

    #[test]
    fn the_log_tail_keeps_only_the_last_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log");
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(log_tail(&path, 2), "c\nd");
        assert!(log_tail(&tmp.path().join("none"), 2).starts_with('('));
    }

    #[test]
    fn the_native_version_line_keeps_the_default_prefix_and_names_the_engine() {
        let line = native_version_line(9);
        let id = native_identity();
        assert!(
            line.starts_with(&format!(
                "pipkin {VERSION} (source {APP_REVISION}, dirty: {APP_DIRTY}, "
            )),
            "{line}"
        );
        assert!(line.contains(&format!(
            "engine native {}, engine protocol {}, engine schema {}, schema 9)",
            id.version, id.protocol, id.schema
        )));
        assert_eq!(id.protocol, pipkin_engine_protocol::PROTOCOL_VERSION);
    }

    #[test]
    fn the_native_marker_is_found_beside_the_binary_only() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("usr/bin/pipkin");
        assert_eq!(native_marker(Some(&exe)), None);
        let marker = tmp.path().join("usr/lib/pipkin").join(NATIVE_MARKER);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "{}").unwrap();
        assert_eq!(native_marker(Some(&exe)), Some(marker));
        assert_eq!(native_marker(None), None);
    }

    fn native_input(probe: Option<Result<String, String>>) -> NativeReportInput {
        NativeReportInput {
            exe: Some(PathBuf::from("/home/ann/.local/bin/pipkin")),
            data_dir: PathBuf::from("/home/ann/.local/share/pipkin"),
            schema_version: 9,
            engine_data_dir: PathBuf::from("/home/ann/.local/share/pipkin/engine"),
            engine_config_dir: PathBuf::from("/home/ann/.config/pipkin/engine"),
            isolated: false,
            marker: None,
            database: Ok("ok".into()),
            wayland_display: true,
            display_vars: vec![],
            probe,
            engine_log_tail: "line one\nline two".into(),
            home: Some(PathBuf::from("/home/ann")),
        }
    }

    #[test]
    fn a_native_report_names_the_engine_and_its_directories_without_node_or_pi() {
        let text = native_report(&native_input(Some(Ok("(fine)".into()))));
        let id = native_identity();
        assert!(!text.contains("/home/ann"), "{text}");
        assert!(text.contains(&format!(
            "engine: native (built into this binary) {} (protocol {}, schema {})",
            id.version, id.protocol, id.schema
        )));
        assert!(text.contains("engine data directory: ~/.local/share/pipkin/engine"));
        assert!(text.contains("engine config directory: ~/.config/pipkin/engine"));
        assert!(text.contains("no Node, npm or Pi engine is used"));
        assert!(text.contains("native-only"));
        assert!(text.contains("no native-candidate marker"));
        assert!(text.contains("engine probe: OK (fine)"));
        assert!(text.contains("  line two"));
        assert!(
            !text.contains("node: "),
            "no Node check in native mode: {text}"
        );
        assert!(!has_problem(&text), "{text}");
        let failed = native_report(&native_input(Some(Err("no reply".into()))));
        assert!(failed.contains("engine probe: FAILED no reply"));
        assert!(has_problem(&failed));
        let mut not_run = native_input(None);
        not_run.database = Err("schema 99 is newer".into());
        not_run.isolated = true;
        let text = native_report(&not_run);
        assert!(text.contains("engine probe: not run"));
        assert!(text.contains("explicit data directory"));
        assert!(has_problem(&text));
    }

    /// A shell-script worker that answers with fixed frames and records its environment.
    fn fake_worker(
        root: &Path,
        replies: &[pipkin_engine_protocol::ServerFrame],
        tail: &str,
    ) -> crate::adapters::native::supervisor::WorkerSpec {
        let bytes: Vec<u8> = replies
            .iter()
            .flat_map(|r| pipkin_engine_protocol::frame::encode(r).unwrap())
            .collect();
        let printf: String = bytes.iter().map(|b| format!("\\{b:03o}")).collect();
        let script = root.join("worker.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nenv > \"$TMPDIR/env.txt\"\necho 'fake worker up' >&2\nprintf '{printf}'\n{tail}\n"),
        )
        .unwrap();
        crate::adapters::native::supervisor::WorkerSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.into_os_string(), "--engine-worker".into()],
            log_path: root.join("engine.log"),
        }
    }

    fn welcome(protocol: u32) -> pipkin_engine_protocol::ServerFrame {
        let id = native_identity();
        pipkin_engine_protocol::ServerFrame::Reply {
            id: 1,
            result: Ok(pipkin_engine_protocol::Reply::Welcome(
                pipkin_engine_protocol::Welcome {
                    protocol,
                    engine_version: id.version.into(),
                    engine_revision: "fake-rev".into(),
                    schema: id.schema,
                    epoch: 1,
                    capabilities: vec![],
                },
            )),
        }
    }

    fn reply(id: u64, reply: pipkin_engine_protocol::Reply) -> pipkin_engine_protocol::ServerFrame {
        pipkin_engine_protocol::ServerFrame::Reply {
            id,
            result: Ok(reply),
        }
    }

    #[test]
    fn the_native_probe_shakes_hands_lists_sessions_and_clears_the_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let id = native_identity();
        let worker = fake_worker(
            tmp.path(),
            &[
                welcome(id.protocol),
                reply(
                    2,
                    pipkin_engine_protocol::Reply::Sessions { sessions: vec![] },
                ),
                reply(3, pipkin_engine_protocol::Reply::Ok),
            ],
            "cat > /dev/null\nexit 0",
        );
        let text = probe_native(&worker, tmp.path(), Duration::from_secs(20)).unwrap();
        assert!(text.contains("revision fake-rev"), "{text}");
        assert!(text.contains("sessions in a fresh profile: 0"), "{text}");
        let env = std::fs::read_to_string(tmp.path().join("env.txt")).unwrap();
        let allowed = [
            "HOME",
            "TMPDIR",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
            "PATH",
            "PWD",
            "SHLVL",
            "_",
            "OLDPWD",
        ];
        for line in env.lines() {
            let key = line.split('=').next().unwrap();
            assert!(
                allowed.contains(&key),
                "unexpected inherited variable {key}"
            );
        }
        assert!(env.contains(&format!("HOME={}", tmp.path().join("home").display())));
    }

    #[test]
    fn the_native_probe_fails_with_a_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let wrong = fake_worker(
            tmp.path(),
            &[welcome(native_identity().protocol + 1)],
            "cat > /dev/null",
        );
        let error = probe_native(&wrong, tmp.path(), Duration::from_secs(20)).unwrap_err();
        assert!(error.contains("protocol"), "{error}");
        let dead = fake_worker(tmp.path(), &[], "echo 'boom' >&2\nexit 3");
        let error = probe_native(&dead, tmp.path(), Duration::from_secs(20)).unwrap_err();
        assert!(
            error.contains("boom"),
            "the worker log tail is included: {error}"
        );
        let sessions = fake_worker(
            tmp.path(),
            &[
                welcome(native_identity().protocol),
                reply(
                    2,
                    pipkin_engine_protocol::Reply::Sessions { sessions: vec![] },
                ),
            ],
            "sleep 1\nexit 4",
        );
        let error = probe_native(&sessions, tmp.path(), Duration::from_secs(20)).unwrap_err();
        assert!(error.contains("exited"), "{error}");
        let missing = crate::adapters::native::supervisor::WorkerSpec {
            program: tmp.path().join("missing"),
            args: vec![],
            log_path: tmp.path().join("engine.log"),
        };
        let error = probe_native(&missing, tmp.path(), Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("cannot start"), "{error}");
    }
}

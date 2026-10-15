//! Standalone Phase 0 experiment; not linked to the native engine's execution path.
//! See docs/claude-cli-bridge-evidence.md. Default invocation never runs inference.

#[path = "claude_cli_probe/interruption.rs"]
mod interruption;
#[path = "claude_cli_probe/isolation.rs"]
mod isolation;
#[path = "claude_cli_probe/lifecycle.rs"]
mod lifecycle;
#[path = "claude_cli_probe/permission.rs"]
mod permission;
#[path = "claude_cli_probe/qualification.rs"]
mod qualification;
#[cfg(test)]
#[path = "claude_cli_probe/recovery_tests.rs"]
mod recovery_tests;
use pipkin_engine::execution::claude_cli::{process, stream};

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use process::{OutputMode, Spec};

const QUALIFICATION_VERSION: &str = pipkin_engine::execution::claude_cli::policy::QUALIFIED_VERSION;
const REQUIRED_FLAGS: &[&str] = &[
    "--safe-mode",
    "--restricted",
    "--print",
    "--tools",
    "--disallowedTools",
    "--strict-mcp-config",
    "--mcp-config",
    "--permission-mode",
    "--permission-prompts",
    "--input-format",
    "--output-format",
    "--verbose",
    "--include-partial-messages",
    "--session-id",
    "--resume",
    "--model",
    "--no-chrome",
    "--disable-slash-commands",
    "--settings",
    "--plugin-dir",
];
const ROUTING_OVERRIDES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_IDENTITY_TOKEN_FILE",
    "ANTHROPIC_PROFILE",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_SIMPLE",
    "CLAUDE_CODE_SUBAGENT_MODEL",
];
const USAGE: &str = "Standalone CLI qualification spike (not a Pipkin backend).\n\
  probe [--executable /absolute/path/claude] --trust-executable\n\
  auth  [--executable /absolute/path/claude] --trust-executable\n\
  isolation [--executable /absolute/path/claude] --trust-executable\n\
  permission-deny|permission-allow|interrupt [--executable /absolute/path/claude]\n\
        --trust-executable --allow-live-usage --trust-project --session-id UUID\n\
        [--model sonnet|haiku|opus]\n\
  smoke [--executable /absolute/path/claude] --trust-executable --allow-live-usage\n\
        --trust-project --cwd /absolute/scratch/project (--session-id UUID | --resume UUID)\n\
        [--model sonnet|haiku|opus]\n\
probe runs only version/help in an empty temporary directory.\n\
auth additionally asks the CLI for auth status (redacted; may refresh login).\n\
isolation runs init-only/MCP-list controls in disposable HOME/config; no inference.\n\
permission modes send one fixed Write request in a generated scratch fixture;\n\
allow grants exactly that one scratch write, deny refuses it. These CAN consume usage.\n\
interrupt stops a turn at its pending scratch-Write approval; never grants or resumes.\n\
It CAN consume usage. Only private fixtures and the CLI's own session history are touched.\n\
smoke sends one fixed no-tools prompt through stdin; this CAN consume usage.\n\
No API fallback, retries, install, login, arbitrary prompt or extra CLI flags.\n\
Safe/restricted modes preserve managed policy: managed hooks may still execute.\n\
Ctrl-C cancels the owned process group. Never automatically repeat an uncertain smoke run.";

type Environment = BTreeMap<OsString, OsString>;

struct Options {
    mode: String,
    executable: Option<PathBuf>,
    trusted: bool,
    allow_usage: bool,
    trust_project: bool,
    cwd: Option<PathBuf>,
    session: Option<String>,
    resume: bool,
    model: String,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, &'static str> {
        let mut args = args.into_iter();
        let mode = args
            .next()
            .and_then(|s| s.into_string().ok())
            .ok_or("missing mode")?;
        if !matches!(
            mode.as_str(),
            "probe"
                | "auth"
                | "smoke"
                | "isolation"
                | "permission-deny"
                | "permission-allow"
                | "interrupt"
        ) {
            return Err("unknown qualification mode");
        }
        let mut o = Self {
            mode,
            executable: None,
            trusted: false,
            allow_usage: false,
            trust_project: false,
            cwd: None,
            session: None,
            resume: false,
            model: "sonnet".into(),
        };
        let mut seen = std::collections::HashSet::new();
        while let Some(flag) = args.next() {
            let flag = flag.into_string().map_err(|_| "invalid option")?;
            if !seen.insert(flag.clone()) {
                return Err("duplicate option");
            }
            match flag.as_str() {
                "--trust-executable" => o.trusted = true,
                "--allow-live-usage" => o.allow_usage = true,
                "--trust-project" => o.trust_project = true,
                "--executable" => {
                    o.executable = Some(args.next().ok_or("missing executable")?.into())
                }
                "--cwd" => o.cwd = Some(args.next().ok_or("missing cwd")?.into()),
                "--session-id" | "--resume" => {
                    if o.session.is_some() {
                        return Err("choose session-id or resume, not both");
                    }
                    let id = args
                        .next()
                        .and_then(|s| s.into_string().ok())
                        .ok_or("missing session id")?;
                    if !valid_uuid(&id) {
                        return Err("session id must be a canonical UUID");
                    }
                    o.session = Some(id.to_ascii_lowercase());
                    o.resume = flag == "--resume";
                }
                "--model" => {
                    o.model = args
                        .next()
                        .and_then(|s| s.into_string().ok())
                        .ok_or("missing model")?;
                    if !matches!(o.model.as_str(), "sonnet" | "haiku" | "opus") {
                        return Err("unsupported smoke-test model alias");
                    }
                }
                _ => return Err("unknown option"),
            }
        }
        if !o.trusted {
            return Err("explicit --trust-executable is required even for probes");
        }
        if o.mode == "smoke" {
            if !o.allow_usage || !o.trust_project || o.cwd.is_none() || o.session.is_none() {
                return Err(
                    "smoke requires usage permission, project trust, cwd, and an explicit session id",
                );
            }
        } else if matches!(
            o.mode.as_str(),
            "permission-deny" | "permission-allow" | "interrupt"
        ) {
            if !o.allow_usage
                || !o.trust_project
                || o.session.is_none()
                || o.resume
                || o.cwd.is_some()
            {
                return Err(
                    "permission tests require usage permission, project trust, a fresh session UUID, and no cwd/resume (scratch is generated)",
                );
            }
        } else if o.allow_usage
            || o.trust_project
            || o.cwd.is_some()
            || o.session.is_some()
            || seen.contains("--model")
        {
            return Err("live-only options are not accepted by probe/auth");
        }
        Ok(o)
    }
}

fn valid_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn resolve(executable: Option<&Path>, env: &Environment) -> Result<PathBuf, &'static str> {
    let usable = |p: &Path| {
        if !p.is_absolute() {
            return false;
        }
        let Ok(meta) = p.metadata() else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.is_file() && meta.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        meta.is_file()
    };
    if let Some(p) = executable {
        if !usable(p) {
            return Err("selected executable must be an executable absolute file path");
        }
        return p.canonicalize().map_err(|_| "cannot resolve executable");
    }
    for dir in std::env::split_paths(
        env.get(OsStr::new("PATH"))
            .ok_or("PATH is unavailable; select an executable")?,
    ) {
        let p = dir.join("claude");
        if usable(&p) {
            return p.canonicalize().map_err(|_| "cannot resolve executable");
        }
    }
    Err("Claude CLI not found in absolute PATH entries; select an executable")
}

fn routing_overrides(env: &Environment) -> Vec<&'static str> {
    ROUTING_OVERRIDES
        .iter()
        .copied()
        .filter(|name| {
            env.get(OsStr::new(name))
                .is_some_and(|value| !value.is_empty())
        })
        .collect()
}

fn base_spec(executable: &Path, cwd: &Path, env: &Environment, args: Vec<OsString>) -> Spec {
    Spec {
        executable: executable.into(),
        args,
        cwd: cwd.into(),
        env: env.clone(),
        input: Vec::new(),
        mode: OutputMode::Capture,
        timeout: Duration::from_secs(10),
        grace: Duration::from_millis(300),
        max_stdout: 128 * 1024,
        max_stderr: 128 * 1024,
    }
}

fn safety_args() -> Vec<OsString> {
    [
        "--mcp-config",
        "{\"mcpServers\":{}}",
        // End the variadic MCP option before any following auth subcommand.
        "--safe-mode",
        "--restricted",
        "--strict-mcp-config",
    ]
    .into_iter()
    .map(Into::into)
    .collect()
}

fn smoke_args(o: &Options) -> Vec<OsString> {
    let mut args = safety_args();
    args.extend(
        [
            "--print",
            "--tools",
            "",
            "--disallowedTools",
            "mcp__*",
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
            "--input-format",
            "text",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-chrome",
            "--disable-slash-commands",
            "--model",
            &o.model,
            if o.resume { "--resume" } else { "--session-id" },
            o.session.as_deref().expect("validated options"),
        ]
        .into_iter()
        .map(OsString::from),
    );
    args
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AuthMethod {
    ClaudeAccount,
    SignedOut,
    Other,
    Unknown,
}
#[derive(Debug, Serialize)]
struct AuthSummary {
    logged_in: bool,
    method: AuthMethod,
}

fn auth_summary(bytes: &[u8]) -> Result<AuthSummary, &'static str> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "invalid auth status JSON")?;
    let logged_in = v
        .get("loggedIn")
        .and_then(|v| v.as_bool())
        .ok_or("auth status missing loggedIn")?;
    let method = match v.get("authMethod").and_then(|v| v.as_str()) {
        Some("claude.ai") if logged_in => AuthMethod::ClaudeAccount,
        Some("none") if !logged_in => AuthMethod::SignedOut,
        Some("oauth_token" | "api_key" | "api_key_helper" | "third_party") => AuthMethod::Other,
        _ => AuthMethod::Unknown,
    };
    Ok(AuthSummary { logged_in, method })
}

fn help_has_flag(help: &str, flag: &str) -> bool {
    help.lines().any(|line| {
        line.trim_start().starts_with('-')
            && line
                .split_whitespace()
                .any(|token| token.trim_end_matches(',') == flag)
    })
}

fn emit(value: &impl Serialize) -> Result<(), &'static str> {
    let text = serde_json::to_string(value).map_err(|_| "could not serialize summary")?;
    println!("{text}");
    Ok(())
}

async fn execute(o: Options, cancel: CancellationToken) -> Result<(), &'static str> {
    let env: Environment = std::env::vars_os().collect();
    let executable = resolve(o.executable.as_deref(), &env)?;
    // Metadata probes never load a user's project directory. Auth identity is not changed.
    let scratch = tempfile::tempdir().map_err(|_| "could not create probe scratch directory")?;
    let cwd = match o.cwd.as_deref() {
        Some(p) if p.is_absolute() && p.is_dir() => {
            p.canonicalize().map_err(|_| "cannot resolve cwd")?
        }
        Some(_) => return Err("cwd must be an absolute existing scratch directory"),
        None => scratch.path().into(),
    };
    let version = process::run(
        base_spec(&executable, scratch.path(), &env, vec!["--version".into()]),
        cancel.clone(),
    )
    .await?;
    if !version.clean_exit() {
        emit(&version)?;
        return Err("version probe did not complete cleanly");
    }
    let version = std::str::from_utf8(&version.captured)
        .map_err(|_| "invalid version response")?
        .trim();
    let expected = format!("{QUALIFICATION_VERSION} (Claude Code)");
    if version != expected {
        emit(
            &serde_json::json!({"supported_version": false, "qualification_version": QUALIFICATION_VERSION}),
        )?;
        return Err("unqualified CLI version; no auth check or model request attempted");
    }
    let help = process::run(
        base_spec(&executable, scratch.path(), &env, vec!["--help".into()]),
        cancel.clone(),
    )
    .await?;
    if !help.clean_exit() {
        emit(&help)?;
        return Err("help probe did not complete cleanly");
    }
    let help = std::str::from_utf8(&help.captured).map_err(|_| "invalid help response")?;
    let missing: Vec<_> = REQUIRED_FLAGS
        .iter()
        .filter(|f| !help_has_flag(help, f))
        .collect();
    let conflicts = routing_overrides(&env);
    emit(&serde_json::json!({
        "mode": o.mode, "cli_version": QUALIFICATION_VERSION,
        "missing_flags": missing, "routing_override_names": conflicts,
        "billing_verified": false, "launch_policy_verified": false,
    }))?;
    if !missing.is_empty() {
        return Err("required CLI flags missing; refusing auth/inference");
    }
    if o.mode == "probe" {
        return Ok(());
    }
    if o.mode == "isolation" {
        return qualification::isolation_probe(&executable, cancel).await;
    }
    if !conflicts.is_empty() {
        return Err(
            "auth routing overrides present; resolve conflicts explicitly (values not logged)",
        );
    }
    if o.mode == "interrupt" {
        return interruption::probe(&executable, &env, &o, cancel).await;
    }
    if matches!(o.mode.as_str(), "permission-deny" | "permission-allow") {
        return qualification::permission_probe(&executable, &env, &o, cancel).await;
    }
    // Same binary, environment, policy and working directory as the inference invocation.
    let mut args = safety_args();
    args.extend(["auth", "status", "--json"].into_iter().map(Into::into));
    let auth = process::run(base_spec(&executable, &cwd, &env, args), cancel.clone()).await?;
    if auth.stop.is_some() || !matches!(auth.exit_code, Some(0 | 1)) {
        emit(&auth)?;
        return Err("auth preflight failed; no model request attempted");
    }
    let summary = auth_summary(&auth.captured)?;
    emit(&summary)?;
    if o.mode == "auth" {
        return Ok(());
    }
    if !auth.clean_exit() || summary.method != AuthMethod::ClaudeAccount {
        return Err(
            "a recognized CLI-owned Claude account login is required; no fallback attempted",
        );
    }
    let mut spec = base_spec(&executable, &cwd, &env, smoke_args(&o));
    spec.mode = OutputMode::Stream;
    spec.timeout = Duration::from_secs(60);
    spec.max_stdout = 16 * 1024 * 1024;
    spec.input = if o.resume {
        b"What exact code did I ask you to remember in the previous turn? Reply only with that code. Do not use tools.\n".to_vec()
    } else {
        b"Remember this exact code for my next turn: PIPKIN_BRIDGE_MEMORY_7Z9. For this turn reply exactly PIPKIN_BRIDGE_OK. Do not use tools.\n".to_vec()
    };
    let result = process::run(spec, cancel).await?;
    emit(&result)?;
    if !result.clean_exit()
        || !result.stream.as_ref().is_some_and(|s| {
            s.init_seen
                && s.declared_tools == Some(0)
                && s.result_seen
                && s.result_success
                && s.session_id == o.session
                && s.smoke_marker
                    == Some(if o.resume {
                        stream::SmokeMarker::Resume
                    } else {
                        stream::SmokeMarker::Fresh
                    })
        })
    {
        return Err(
            "smoke invocation not confirmed successful; do not automatically retry or resume",
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args == [OsStr::new("--help")] {
        println!("{USAGE}");
        return;
    }
    if args[0] == "fixture-interrupt" {
        if let Err(error) = interruption::helper(&args) {
            eprintln!("{error}");
            let _ = emit(&permission::denial());
        }
        return;
    }
    if matches!(
        args[0].to_str(),
        Some("fixture-mark" | "fixture-permission")
    ) {
        if let Err(error) = qualification::helper(&args) {
            eprintln!("{error}");
            if args[0] == "fixture-permission" {
                let _ = emit(&permission::denial());
                return;
            }
            std::process::exit(1);
        }
        return;
    }
    let options = match Options::parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancel.cancel();
        }
    });
    let result = execute(options, cancel).await;
    signal.abort();
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options, &'static str> {
        Options::parse(args.iter().map(OsString::from))
    }
    #[test]
    fn inference_is_explicit_and_unknown_or_conflicting_options_are_rejected() {
        assert!(parse(&["probe"]).is_err());
        assert!(parse(&["smoke", "--trust-executable"]).is_err());
        assert!(parse(&["probe", "--trust-executable", "--allow-live-usage"]).is_err());
        assert!(
            parse(&[
                "probe",
                "--trust-executable",
                "--dangerously-skip-permissions"
            ])
            .is_err()
        );
        assert!(parse(&["probe", "--trust-executable", "--trust-executable"]).is_err());
        assert!(parse(&["probe", "--trust-executable"]).is_ok());
    }
    #[test]
    fn permission_modes_require_explicit_usage_and_use_only_generated_scratch() {
        let id = "11111111-1111-4111-8111-111111111111";
        for mode in ["permission-deny", "permission-allow", "interrupt"] {
            assert!(parse(&[mode, "--trust-executable"]).is_err());
            assert!(
                parse(&[
                    mode,
                    "--trust-executable",
                    "--trust-project",
                    "--session-id",
                    id
                ])
                .is_err()
            );
            let args = [
                mode,
                "--trust-executable",
                "--allow-live-usage",
                "--trust-project",
                "--session-id",
                id,
            ];
            assert!(parse(&args).is_ok());
            let mut with_cwd = args.to_vec();
            with_cwd.extend(["--cwd", "/some/existing/project"]);
            assert!(parse(&with_cwd).is_err());
            assert!(
                parse(&[
                    mode,
                    "--trust-executable",
                    "--allow-live-usage",
                    "--trust-project",
                    "--resume",
                    id
                ])
                .is_err()
            );
        }
        assert!(parse(&["isolation", "--trust-executable"]).is_ok());
        assert!(parse(&["isolation", "--trust-executable", "--allow-live-usage"]).is_err());
    }

    #[test]
    fn launch_arguments_are_fixed_no_tools_no_bare_no_latest_session() {
        for mode in ["--session-id", "--resume"] {
            let o = parse(&[
                "smoke",
                "--trust-executable",
                "--allow-live-usage",
                "--trust-project",
                "--cwd",
                "/tmp/scratch with spaces",
                mode,
                "11111111-1111-4111-8111-111111111111",
            ])
            .unwrap();
            let args = smoke_args(&o);
            assert!(
                args.windows(2)
                    .any(|a| a == [OsStr::new("--tools"), OsStr::new("")])
            );
            assert!(
                args.windows(2)
                    .any(|a| a == [OsStr::new("--permission-mode"), OsStr::new("dontAsk")])
            );
            for forbidden in [
                "--bare",
                "--continue",
                "--dangerously-skip-permissions",
                "--fallback-model",
                "--bg",
            ] {
                assert!(!args.contains(&forbidden.into()));
            }
            assert!(
                !args
                    .iter()
                    .any(|s| s.to_string_lossy().contains("PIPKIN_BRIDGE"))
            );
        }
    }
    #[test]
    fn auth_summary_never_retains_account_or_credential_fields() {
        let s = auth_summary(br#"{"loggedIn":true,"authMethod":"claude.ai","email":"secret@example.invalid","access_token":"secret-token","configDirectory":"secret-path"}"#).unwrap();
        assert_eq!(s.method, AuthMethod::ClaudeAccount);
        let text = serde_json::to_string(&s).unwrap();
        assert!(!text.contains("secret"));
        assert!(auth_summary(br#"{"loggedIn":"true"}"#).is_err());
        assert_eq!(
            auth_summary(br#"{"loggedIn":true,"authMethod":"oauth_token"}"#)
                .unwrap()
                .method,
            AuthMethod::Other
        );
    }
    #[test]
    fn routing_checks_only_presence_without_echoing_values() {
        let env = [
            ("ANTHROPIC_API_KEY".into(), "secret-token".into()),
            ("HTTPS_PROXY".into(), "secret-proxy".into()),
        ]
        .into();
        assert_eq!(routing_overrides(&env), ["ANTHROPIC_API_KEY"]);
    }
    #[test]
    fn flags_are_exact_tokens_not_substrings_or_description_text() {
        assert!(help_has_flag("  -p, --print  Print response", "--print"));
        assert!(!help_has_flag("  --print-extra  not the flag", "--print"));
        assert!(!help_has_flag("  Example: use --print", "--print"));
    }
    #[test]
    fn uuid_validation_rejects_argument_injection_and_malformed_ids() {
        assert!(valid_uuid("11111111-1111-4111-8111-111111111111"));
        for s in [
            "--continue",
            "../session",
            "11111111-1111-4111-8111-11111111111z",
        ] {
            assert!(!valid_uuid(s));
        }
    }
    #[cfg(unix)]
    #[test]
    fn discovery_checks_paths_without_shell_execution() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        let env = [("PATH".into(), dir.path().as_os_str().to_owned())].into();
        assert!(resolve(None, &env).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(resolve(None, &env).unwrap(), path.canonicalize().unwrap());
        assert!(resolve(Some(Path::new("./claude")), &env).is_err());
        assert!(resolve(Some(&dir.path().join("missing")), &env).is_err());
    }
}

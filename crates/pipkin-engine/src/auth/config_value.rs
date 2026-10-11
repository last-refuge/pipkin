//! Configuration values for keys and headers (reference `coding-agent/core/resolve-config-value.ts`).
//!
//! - A value starting with `!` is a shell command; its trimmed stdout is the value. Commands run
//!   through the user's shell (`$SHELL -c`, falling back to `/bin/sh`) with a 10 s timeout.
//!   Key values are cached for the process lifetime; header values run each time.
//! - Otherwise the value is a template: `$NAME` / `${NAME}` are replaced by variables looked up
//!   in the credential's `env` first, then in [`Env`]. Every referenced variable must resolve,
//!   or the whole value is unresolved. `$$` is a literal `$` and `$!` a literal `!`.
//!
//! Availability checks never run commands: [`is_configured`] treats a command as configured.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::config::Env;

/// Reference `spawnSync`/`execSync` timeout.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Literal(String),
    Env(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reference {
    Command(String),
    Template(Vec<Part>),
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
}

fn push_literal(parts: &mut Vec<Part>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Part::Literal(prev)) = parts.last_mut() {
        prev.push_str(s);
    } else {
        parts.push(Part::Literal(s.to_owned()));
    }
}

fn parse_template(config: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut rest = config;
    while let Some(dollar) = rest.find('$') {
        push_literal(&mut parts, &rest[..dollar]);
        let after = &rest[dollar + 1..];
        match after.chars().next() {
            Some(c @ ('$' | '!')) => {
                push_literal(&mut parts, &c.to_string());
                rest = &after[1..];
            }
            Some('{') => match after.find('}') {
                None => {
                    push_literal(&mut parts, "$");
                    rest = after;
                }
                Some(end) => {
                    let name = &after[1..end];
                    if valid_name(name) {
                        parts.push(Part::Env(name.to_owned()));
                    } else {
                        push_literal(&mut parts, &rest[dollar..dollar + 1 + end + 1]);
                    }
                    rest = &after[end + 1..];
                }
            },
            Some(c) if is_name_start(c) => {
                let len = after
                    .char_indices()
                    .find(|(_, c)| !is_name_char(*c))
                    .map_or(after.len(), |(i, _)| i);
                parts.push(Part::Env(after[..len].to_owned()));
                rest = &after[len..];
            }
            _ => {
                push_literal(&mut parts, "$");
                rest = after;
            }
        }
    }
    push_literal(&mut parts, rest);
    parts
}

fn parse(config: &str) -> Reference {
    match config.strip_prefix('!') {
        Some(cmd) => Reference::Command(cmd.to_owned()),
        None => Reference::Template(parse_template(config)),
    }
}

pub fn is_command(config: &str) -> bool {
    config.starts_with('!')
}

/// Distinct variable names a template references (none for commands).
pub fn env_names(config: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Reference::Template(parts) = parse(config) {
        for p in parts {
            if let Part::Env(n) = p
                && !names.contains(&n)
            {
                names.push(n);
            }
        }
    }
    names
}

fn lookup(name: &str, cred_env: Option<&BTreeMap<String, String>>, env: &Env) -> Option<String> {
    cred_env
        .and_then(|e| e.get(name))
        .filter(|v| !v.is_empty())
        .cloned()
        .or_else(|| env.get(name))
}

/// Variables a template references that do not resolve.
pub fn missing_names(
    config: &str,
    cred_env: Option<&BTreeMap<String, String>>,
    env: &Env,
) -> Vec<String> {
    env_names(config)
        .into_iter()
        .filter(|n| lookup(n, cred_env, env).is_none())
        .collect()
}

/// Metadata check: a command counts as configured (it is not run); a template is configured
/// when every variable it references resolves.
pub fn is_configured(config: &str, cred_env: Option<&BTreeMap<String, String>>, env: &Env) -> bool {
    is_command(config) || missing_names(config, cred_env, env).is_empty()
}

/// Resolve a template without running anything; `None` for commands or missing variables.
pub fn resolve_template(
    config: &str,
    cred_env: Option<&BTreeMap<String, String>>,
    env: &Env,
) -> Option<String> {
    let Reference::Template(parts) = parse(config) else {
        return None;
    };
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Literal(s) => out.push_str(&s),
            Part::Env(n) => out.push_str(&lookup(&n, cred_env, env)?),
        }
    }
    Some(out)
}

/// Resolve a value. `cached` keeps a command's result for the process lifetime (keys);
/// header values pass `false`.
pub async fn resolve(
    config: &str,
    cred_env: Option<&BTreeMap<String, String>>,
    env: &Env,
    cached: bool,
) -> Option<String> {
    match parse(config) {
        Reference::Command(cmd) => {
            if cached {
                run_cached(&cmd, env).await
            } else {
                run_command(&cmd, env, COMMAND_TIMEOUT).await
            }
        }
        Reference::Template(_) => resolve_template(config, cred_env, env),
    }
}

/// Resolve or explain why not, in the reference wording
/// (`Failed to resolve <description> from environment variable: NAME`).
pub async fn resolve_or_err(
    config: &str,
    description: &str,
    cred_env: Option<&BTreeMap<String, String>>,
    env: &Env,
    cached: bool,
) -> Result<String, String> {
    if let Some(v) = resolve(config, cred_env, env, cached).await {
        return Ok(v);
    }
    if let Some(cmd) = config.strip_prefix('!') {
        return Err(format!(
            "Failed to resolve {description} from shell command: {cmd}"
        ));
    }
    let missing = missing_names(config, cred_env, env);
    Err(match missing.len() {
        0 => format!("Failed to resolve {description}"),
        1 => format!(
            "Failed to resolve {description} from environment variable: {}",
            missing[0]
        ),
        _ => format!(
            "Failed to resolve {description} from environment variables: {}",
            missing.join(", ")
        ),
    })
}

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn run_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Default::default)
}

async fn run_cached(cmd: &str, env: &Env) -> Option<String> {
    let key = format!("!{cmd}");
    if let Some(v) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    // One execution per command even when several requests miss the cache together.
    let _guard = run_lock().lock().await;
    if let Some(v) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let value = run_command(cmd, env, COMMAND_TIMEOUT).await;
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, value.clone());
    value
}

/// Run `cmd` through the user's shell. Nothing on failure, non-zero exit, timeout or empty
/// output. With an isolated [`Env`], the child sees only the configured variables.
pub(crate) async fn run_command(cmd: &str, env: &Env, timeout: Duration) -> Option<String> {
    let shell = env.get("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let mut command = tokio::process::Command::new(&shell);
    command
        .arg("-c")
        .arg(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if !env.ambient {
        command.env_clear();
        command.envs(&env.vars);
    } else {
        command.envs(&env.vars);
    }
    let child = command.spawn().ok()?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> Env {
        Env::isolated(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn templates_follow_the_reference_grammar() {
        let e = env(&[("A", "1"), ("B_2", "two"), ("EMPTY", "")]);
        let r = |s: &str| resolve_template(s, None, &e);
        assert_eq!(r("plain").as_deref(), Some("plain"));
        assert_eq!(r("$A").as_deref(), Some("1"));
        assert_eq!(r("x-${B_2}-$A.y").as_deref(), Some("x-two-1.y"));
        assert_eq!(r("$$A").as_deref(), Some("$A"));
        assert_eq!(r("$!cmd").as_deref(), Some("!cmd"));
        assert_eq!(r("a$").as_deref(), Some("a$"));
        assert_eq!(r("a$1").as_deref(), Some("a$1"));
        assert_eq!(r("${not valid}").as_deref(), Some("${not valid}"));
        assert_eq!(r("${unclosed").as_deref(), Some("${unclosed"));
        // Every variable must resolve; empty counts as missing.
        assert_eq!(r("$A$MISSING"), None);
        assert_eq!(r("$EMPTY"), None);
        assert_eq!(env_names("$A ${B_2} $A $$C"), vec!["A", "B_2"]);
        assert!(env_names("!echo $A").is_empty());
    }

    #[test]
    fn credential_env_wins_and_the_process_env_is_isolated() {
        let e = env(&[("K", "from-config")]);
        let cred: BTreeMap<String, String> = [("K".to_string(), "from-cred".to_string())].into();
        assert_eq!(
            resolve_template("$K", Some(&cred), &e).as_deref(),
            Some("from-cred")
        );
        assert_eq!(
            resolve_template("$K", None, &e).as_deref(),
            Some("from-config")
        );
        // PATH/HOME exist in the test process but an isolated Env never reads them.
        assert_eq!(resolve_template("$PATH", None, &e), None);
        assert!(!is_configured("$HOME", None, &e));
        assert!(is_configured("!anything", None, &e));
    }

    #[tokio::test]
    async fn commands_run_through_the_shell_and_cache_key_values() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("count");
        let cmd = format!("!echo x >> {} ; echo ' secret-value '", counter.display());
        let e = env(&[]);
        assert_eq!(
            resolve(&cmd, None, &e, true).await.as_deref(),
            Some("secret-value")
        );
        assert_eq!(
            resolve(&cmd, None, &e, true).await.as_deref(),
            Some("secret-value")
        );
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            1
        );
        // Uncached (header) values run every time.
        resolve(&cmd, None, &e, false).await;
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
    }

    #[tokio::test]
    async fn failing_commands_and_missing_variables_explain_themselves() {
        let e = env(&[("ONE", "1")]);
        assert_eq!(resolve("!exit 3", None, &e, false).await, None);
        assert_eq!(resolve("!true", None, &e, false).await, None);
        assert_eq!(
            resolve_or_err("!exit 3", "API key", None, &e, false).await,
            Err("Failed to resolve API key from shell command: exit 3".into())
        );
        assert_eq!(
            resolve_or_err("$ONE$TWO", "API key", None, &e, false).await,
            Err("Failed to resolve API key from environment variable: TWO".into())
        );
        assert_eq!(
            resolve_or_err("$X$Y", "key", None, &e, false).await,
            Err("Failed to resolve key from environment variables: X, Y".into())
        );
        // An isolated child sees only configured variables.
        assert_eq!(
            run_command("printf %s \"$ONE${HOME:-nohome}\"", &e, COMMAND_TIMEOUT)
                .await
                .as_deref(),
            Some("1nohome")
        );
    }

    #[tokio::test]
    async fn commands_time_out() {
        let started = std::time::Instant::now();
        assert_eq!(
            run_command("sleep 5; echo late", &env(&[]), Duration::from_millis(200)).await,
            None
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

//! Ordinary unattended coding provider. The installed CLI owns tools; stdout never
//! authorizes native execution. This policy has a new immutable identity so legacy
//! chat/approval-backed contexts cannot silently acquire different execution semantics.
use super::{
    policy,
    process::{OutputMode, Spec},
};
use std::{ffi::OsString, path::Path, time::Duration};

pub const ID: &str = "cli-unattended-coding-v1";
pub const FLAGS: &[&str] = &[
    "--print",
    "--tools",
    "--permission-mode",
    "--permission-prompts",
    "--dangerously-skip-permissions",
    "--input-format",
    "--output-format",
    "--verbose",
    "--include-partial-messages",
    "--model",
    "--session-id",
    "--resume",
    "--mcp-config",
    "--strict-mcp-config",
    "--settings",
    "--setting-sources",
    "--no-chrome",
    "--append-system-prompt",
];
pub fn metadata_args() -> Vec<OsString> {
    [
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--settings",
        "{\"disableAllHooks\":true}",
    ]
    .into_iter()
    .map(Into::into)
    .collect()
}
pub fn turn_spec(
    executable: &Path,
    cwd: &Path,
    env: &policy::Environment,
    uuid: &str,
    mode: policy::TurnMode,
    model: policy::ModelAlias,
    prompt: &str,
) -> Result<Spec, &'static str> {
    if !policy::valid_uuid(uuid)
        || prompt.trim().is_empty()
        || prompt.len() > policy::MAX_PROMPT_BYTES
        || prompt.contains('\0')
    {
        return Err("invalid CLI coding request");
    }
    let mut args = metadata_args();
    args.extend(
        [
            "--print",
            "--permission-mode",
            "bypassPermissions",
            "--permission-prompts",
            "none",
            "--dangerously-skip-permissions",
            "--input-format",
            "text",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-chrome",
            "--append-system-prompt",
            "Before changing files, read and follow applicable AGENTS.md and CLAUDE.md project instructions. Use your actual tools, not printed tool-call markup. Do not claim a file was read, changed or tested without tool evidence.",
            "--model",
            model.as_str(),
            if mode == policy::TurnMode::Resume {
                "--resume"
            } else {
                "--session-id"
            },
            uuid,
            "--tools",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push("default".into());
    let mut spec = policy::metadata_spec(executable, cwd, env, args);
    spec.input = prompt.as_bytes().to_vec();
    spec.input.push(b'\n');
    spec.mode = OutputMode::Stream;
    // Coding includes builds/tests and CLI-owned tool rounds, not just one API answer.
    spec.timeout = Duration::from_secs(30 * 60);
    spec.max_stdout = 16 * 1024 * 1024;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_provider_has_default_tools_no_permission_hook_and_stdin_only_prompts() {
        for mode in [policy::TurnMode::Fresh, policy::TurnMode::Resume] {
            let spec = turn_spec(
                Path::new("/owned/claude"),
                Path::new("/project"),
                &Default::default(),
                "11111111-1111-4111-8111-111111111111",
                mode,
                policy::ModelAlias::Opus,
                "private $(not a shell command)",
            )
            .unwrap();
            let args = spec
                .args
                .iter()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>();
            assert!(args.windows(2).any(|p| p == ["--tools", "default"]));
            assert!(
                args.windows(2)
                    .any(|p| p == ["--permission-mode", "bypassPermissions"])
            );
            assert!(
                args.windows(2)
                    .any(|p| p == ["--permission-prompts", "none"])
            );
            assert!(!args.iter().any(|a| a.contains("private")
                || matches!(
                    *a,
                    "--safe-mode" | "--restricted" | "--permission-prompt-tool"
                )));
            assert_eq!(spec.input, b"private $(not a shell command)\n");
            assert_eq!(spec.cwd, Path::new("/project"));
            assert_ne!(ID, policy::POLICY_ID);
        }
    }
}

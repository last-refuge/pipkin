//! Explicit, non-CI qualification of the current public runtime with an installed CLI.
//! No credential reads; only fixed scratch prompts/actions. A durable budget precedes admission.
use pipkin_engine::{
    config::EngineConfig,
    host::Connection,
    provider::scripted::Scripted,
    runtime::{EngineHandle, registry_with},
};
use pipkin_engine_protocol::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

// User explicitly extended the original six-run allowance by four. Keep the same ledger.
const MAX_ATTEMPTS: usize = 10;
const CONTENT: &str = "qualification-only\n";
struct Runtime {
    engine: EngineHandle,
    connection: Connection,
    next: u64,
}
impl Runtime {
    fn call(&mut self, call: Call) -> Result<Reply, String> {
        self.next += 1;
        let id = self.next;
        self.connection
            .send(ClientFrame {
                id,
                lane: call.lane(),
                call,
            })
            .map_err(|_| "request not sent".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let frame = self
                .connection
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| "RPC outcome unknown; never resend".to_string())?;
            if let ServerFrame::Reply { id: got, result } = frame
                && got == id
            {
                return result
                    .map_err(|e| format!("RPC failed ({:?}); raw error withheld", e.code));
            }
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.engine.shutdown(Duration::from_secs(10));
    }
}
fn reserve(budget: &Path) -> Result<usize, String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(budget)
        .map_err(|_| "budget unavailable")?;
    file.lock().map_err(|_| "budget lock failed")?;
    if file
        .metadata()
        .map_err(|_| "budget metadata unavailable")?
        .len()
        > 32
    {
        return Err("invalid budget length".into());
    }
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(32)
        .read_to_string(&mut text)
        .map_err(|_| "budget unreadable")?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("partial budget reservation; do not retry".into());
    }
    let mut used = 0;
    for line in text.lines() {
        used += 1;
        if line != used.to_string() {
            return Err("invalid budget sequence".into());
        }
    }
    if used >= MAX_ATTEMPTS {
        return Err("ten-run authorization exhausted".into());
    }
    writeln!(file, "{}", used + 1).map_err(|_| "budget write failed")?;
    file.sync_all().map_err(|_| "budget commit failed")?;
    fs::File::open(budget.parent().ok_or("budget parent unavailable")?)
        .and_then(|parent| parent.sync_all())
        .map_err(|_| "budget directory commit failed")?;
    Ok(used + 1)
}
fn quoted(s: &str) -> String {
    format!(
        "\"{}\"",
        s.chars().flat_map(char::escape_default).collect::<String>()
    )
}
fn expected_review(path: &Path) -> String {
    format!(
        "Write\nfile_path: {}\ncontent: {}",
        quoted(path.to_str().unwrap()),
        quoted(CONTENT)
    )
}
fn trap(project: &Path) -> bool {
    project.join("unexpected-hook").exists() || project.join("unexpected-mcp").exists()
}
fn canaries(project: &Path) -> Result<(), String> {
    fs::create_dir(project.join(".claude")).map_err(|_| "fixture creation failed")?;
    let marker = project.join("unexpected-hook");
    let command = format!("printf trap > '{}'", marker.display());
    fs::write(project.join(".claude/settings.json"),json!({"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":command}]}],"PermissionRequest":[{"hooks":[{"type":"command","command":command}]}]}}).to_string()).map_err(|_|"fixture creation failed")?;
    fs::write(project.join(".mcp.json"),json!({"mcpServers":{"qualification-canary":{"command":"/bin/sh","args":["-c",format!("printf trap > '{}'",project.join("unexpected-mcp").display())]}}}).to_string()).map_err(|_|"fixture creation failed")?;
    Ok(())
}
fn setup(
    rt: &mut Runtime,
    exe: &str,
    project: &Path,
    mode: LocalCliMode,
) -> Result<String, String> {
    let Reply::LocalCliSetup(view) = rt.call(Call::InspectLocalCli {
        mode,
        executable: exe.into(),
        target: LocalCliSetupTarget::New {
            project: project.display().to_string(),
            model: LocalCliModel::Haiku,
        },
    })?
    else {
        return Err("unexpected inspection reply".into());
    };
    let setup = view.setup;
    let Reply::LocalCliSetup(view) = rt.call(Call::CheckLocalCli {
        setup: setup.clone(),
        trust_executable: true,
        trust_project: true,
    })?
    else {
        return Err("unexpected verification reply".into());
    };
    if view.stage != LocalCliSetupStage::MetadataVerified {
        return Err("metadata verification failed; no inference released".into());
    }
    if trap(project) {
        return Err("configuration canary executed during metadata checks".into());
    }
    let session = format!("cli-{setup}");
    let Reply::Session { summary } = rt.call(Call::CreateLocalCliSession {
        session: session.clone(),
        setup,
    })?
    else {
        return Err("unexpected creation reply".into());
    };
    if summary.id != session || summary.blocked.is_some() {
        return Err("created conversation is not ready".into());
    }
    Ok(session)
}
fn prompt(
    rt: &mut Runtime,
    session: &str,
    project: &Path,
    budget: &Path,
    mode: &str,
    index: u32,
    summary: &mut Value,
) -> Result<(), String> {
    let request = format!("qualification-{index}");
    let effect = project.join(format!("bridge-write-{index}.txt"));
    let chat = mode == "chat";
    let expected = if index == 1 {
        "CLI_BRIDGE_OK"
    } else {
        "CLI_BRIDGE_RESUME_OK"
    };
    let text = if chat {
        if index == 1 {
            "Remember the code CLI_BRIDGE_RESUME_OK for my next turn. Reply exactly CLI_BRIDGE_OK. Do not use tools.".into()
        } else {
            "What exact code did I ask you to remember previously? Reply only with that code. Do not use tools.".into()
        }
    } else {
        format!(
            "Use Write exactly once to create the absolute file {} with exactly the text qualification-only followed by one newline. Do not read files or use any other tool. Do not retry denied writes. After the tool finishes, reply briefly.",
            effect.display()
        )
    };
    reserve(budget)?;
    summary["prompt_attempts"] = json!(summary["prompt_attempts"].as_u64().unwrap() + 1);
    let Reply::Admitted {
        run: Some(prompt), ..
    } = rt.call(Call::Prompt {
        session: session.into(),
        request: request.clone(),
        text,
        images: vec![],
        model: None,
    })?
    else {
        return Err("prompt admission unavailable; outcome may be unknown".into());
    };
    let deadline = Instant::now() + Duration::from_secs(100);
    let mut reviewed = None;
    let mut choice_sent = false;
    loop {
        if trap(project) {
            let _ = rt.call(Call::Stop {
                session: session.into(),
            });
            return Err("configuration canary executed; stopped".into());
        }
        let receipt = rt
            .engine
            .shared()
            .store
            .external_prompt(session, &request)
            .map_err(|_| "receipt unavailable")?
            .ok_or("receipt missing")?;
        if receipt.outcome.is_some()
            && rt
                .engine
                .shared()
                .session(session)
                .map_err(|_| "session unavailable")?
                .active_run()
                .is_none()
        {
            summary["outcomes"].as_array_mut().unwrap().push(
                json!({"state":format!("{:?}",receipt.state),"mode":format!("{:?}",receipt.mode),"evidence":receipt.evidence}),
            );
            let expected_mode = if index == 1 { "Fresh" } else { "Resume" };
            if format!("{:?}", receipt.mode) != expected_mode {
                return Err("unexpected fresh/resume admission mode".into());
            }
            if (chat || mode == "allow") && !matches!(receipt.outcome, Some(RunOutcome::Completed))
            {
                return Err("independent clean completion evidence missing".into());
            }
            if chat {
                if !matches!(receipt.result,Some(PromptResult::Done {text,..}) if text.trim()==expected)
                {
                    return Err(
                        "chat did not finish with the expected independent completion evidence"
                            .into(),
                    );
                }
            } else if reviewed.is_none() {
                return Err("run ended without the expected permission review".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = rt.call(Call::Stop {
                session: session.into(),
            });
            return Err("qualification deadline; stopped without retry".into());
        }
        if !chat && reviewed.is_none() {
            let Reply::LocalCliApprovals { run, reviews } = rt.call(Call::LocalCliApprovals {
                session: session.into(),
            })?
            else {
                return Err("unexpected review reply".into());
            };
            if !reviews.is_empty() {
                if reviews.len() != 1
                    || run.as_ref() != Some(&reviews[0].id.run)
                    || reviews[0].id.run.session != session
                    || reviews[0].id.run.prompt != prompt
                    || reviews[0].arguments != expected_review(&effect)
                    || fs::symlink_metadata(&effect).is_ok()
                {
                    let _ = rt.call(Call::Stop {
                        session: session.into(),
                    });
                    return Err("unexpected review/effect; no allow sent".into());
                }
                let id = reviews[0].id.clone();
                reviewed = Some(id.clone());
                summary["review_matched"] = json!(true);
                summary["review_count"] = json!(summary["review_count"].as_u64().unwrap_or(0) + 1);
                if mode == "stop" {
                    rt.call(Call::Stop {
                        session: session.into(),
                    })?;
                } else {
                    choice_sent = true;
                    rt.call(Call::DecideLocalCliApproval {
                        id,
                        decision: if mode == "allow" {
                            LocalCliApprovalDecision::AllowOnce
                        } else {
                            LocalCliApprovalDecision::Deny
                        },
                    })?;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Some(id) = reviewed {
        let Reply::LocalCliApproval { receipt } = rt.call(Call::LookupLocalCliApproval { id })?
        else {
            return Err("unexpected permission lookup reply".into());
        };
        summary["permission_receipt"] = json!(receipt.as_ref().map(|r| format!("{:?}", r.state)));
        let expected = match mode {
            "allow" => LocalCliApprovalState::Consumed,
            "deny" => LocalCliApprovalState::Denied,
            "stop" => LocalCliApprovalState::Revoked,
            _ => return Err("unexpected permission case".into()),
        };
        if !receipt.is_some_and(|r| r.state == expected) {
            return Err("permission receipt did not match the case; never resend".into());
        }
    }
    summary["decision_sent"] = json!(choice_sent);
    let observed = match fs::symlink_metadata(&effect) {
        Ok(m) if m.file_type().is_file() => {
            fs::read(&effect).map_err(|_| "effect unreadable")? == CONTENT.as_bytes()
        }
        Ok(_) => return Err("unexpected effect type".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err("effect status unavailable".into()),
    };
    summary["expected_effect_observed"] = json!(observed);
    if !chat && observed != (mode == "allow") {
        return Err("effect observation did not match the authorized case".into());
    }
    Ok(())
}
fn qualify(exe: &str, mode: &str, budget: &Path, summary: &mut Value) -> Result<(), String> {
    let root = tempfile::Builder::new()
        .prefix("cq")
        .tempdir()
        .map_err(|_| "private fixture unavailable")?;
    let project = root.path().join("p");
    fs::create_dir(&project).map_err(|_| "private project unavailable")?;
    canaries(&project)?;
    let provider = Scripted::echo("NATIVE INFERENCE IS FORBIDDEN");
    let mut config = EngineConfig::new(root.path().join("data"), root.path().join("config"));
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_write = true;
    let engine = EngineHandle::start_with(
        config,
        registry_with(Arc::new(provider.clone())),
        pipkin_engine::tools::builtin_tools(),
    )
    .map_err(|_| "engine start failed")?;
    let connection = engine.connect("authorized-runtime-qualification");
    let mut rt = Runtime {
        engine,
        connection,
        next: 0,
    };
    rt.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "qualification".into(),
    })?;
    let session = setup(
        &mut rt,
        exe,
        &project,
        if mode == "chat" {
            LocalCliMode::ChatOnly
        } else {
            LocalCliMode::RestrictedWrite
        },
    )?;
    summary["metadata_verified"] = json!(true);
    let result = if mode == "inspect" {
        Ok(())
    } else {
        prompt(&mut rt, &session, &project, budget, mode, 1, summary).and_then(|()| {
            // Two separately authorized fixed scenarios: only confirmed completion permits
            // the next fresh input. The second Write targets a different scratch file.
            if mode == "chat" || mode == "allow" {
                prompt(&mut rt, &session, &project, budget, mode, 2, summary)
            } else {
                Ok(())
            }
        })
    };
    summary["configuration_canaries_clear"] = json!(!trap(&project));
    summary["native_inference_calls"] = json!(provider.requests().len());
    let result = if provider.requests().is_empty() {
        result
    } else {
        Err("native inference isolation failed".into())
    };
    drop(rt); // Finish owned shutdown before inspecting/retiring the fixture.
    if result.is_err() {
        summary["fixture_retained"] = json!(true);
        eprintln!(
            "Owned qualification fixture retained for inspection: {}",
            root.keep().display()
        );
    }
    result
}
fn main() {
    if let Some(code) =
        pipkin_engine::execution::claude_cli::permission_helper::maybe_run_from_env()
    {
        std::process::exit(code);
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    // Reservation only: no engine, executable probe, prompt or permission decision.
    // Guarded desktop tests must reserve immediately before each separate submission.
    if args.len() == 5
        && args[0] == "--reserve-desktop-attempt"
        && args[1] == "--budget"
        && args[3] == "--output"
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[4])
            .expect("new reservation evidence only");
        match reserve(Path::new(&args[2])) {
            Ok(attempt) => {
                let fact = json!({"reserved_attempt":attempt,"cumulative_cap":MAX_ATTEMPTS,"execution_launched_by_reservation":false});
                writeln!(file, "{fact}").unwrap();
                file.sync_all().unwrap();
                println!("{fact}");
                return;
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }
    if args.len() != 9
        || args[0] != "--allow-live-usage"
        || args[1] != "--trust-executable"
        || args[3] != "--mode"
        || args[5] != "--budget"
        || args[7] != "--output"
        || !matches!(
            args[4].as_str(),
            "inspect" | "chat" | "allow" | "deny" | "stop"
        )
        || !Path::new(&args[2]).is_absolute()
    {
        eprintln!(
            "usage: --allow-live-usage --trust-executable /absolute/claude --mode inspect|chat|allow|deny|stop --budget FILE --output NEW_FILE"
        );
        std::process::exit(2);
    }
    let output = PathBuf::from(&args[8]);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .expect("new qualification summary only");
    let mut summary = json!({"cli_version":pipkin_engine::execution::claude_cli::policy::QUALIFIED_VERSION,"mode":args[4],"prompt_attempts":0,"outcomes":[],"billing_verified":false,"crash_containment_verified":false,"general_production_enabled":false,"runtime_host_opts_enabled_for_qualification":true});
    let result = qualify(&args[2], &args[4], Path::new(&args[6]), &mut summary);
    summary["passed"] = json!(result.is_ok());
    if let Err(reason) = &result {
        summary["reason"] = json!(reason);
    }
    let text = serde_json::to_string_pretty(&summary).unwrap();
    file.write_all(text.as_bytes()).unwrap();
    file.sync_all().unwrap();
    println!("{text}");
    if result.is_err() {
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_is_durable_and_cannot_exceed_ten_authorized_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget");
        for expected in 1..=MAX_ATTEMPTS {
            assert_eq!(reserve(&path).unwrap(), expected);
        }
        assert!(reserve(&path).is_err());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            (1..=MAX_ATTEMPTS)
                .map(|n| format!("{n}\n"))
                .collect::<String>()
        );
    }
    #[test]
    fn additional_authorization_preserves_all_six_original_reservations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget");
        fs::write(&path, "1\n2\n3\n4\n5\n6\n").unwrap();
        assert_eq!(reserve(&path).unwrap(), 7);
        assert_eq!(fs::read_to_string(path).unwrap(), "1\n2\n3\n4\n5\n6\n7\n");
    }
    #[test]
    fn partial_or_corrupt_budget_cannot_reset_allowance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget");
        for corrupt in [
            "1",
            "2\n",
            "1\n1\n",
            "1\n2",
            "\n",
            "999999999999999999999999999999999999",
        ] {
            fs::write(&path, corrupt).unwrap();
            assert!(reserve(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
        }
    }
    #[test]
    fn exact_review_is_full_escaped_and_cannot_accept_another_path_or_content() {
        let expected = expected_review(Path::new("/scratch/bridge-write.txt"));
        assert_eq!(
            expected,
            "Write\nfile_path: \"/scratch/bridge-write.txt\"\ncontent: \"qualification-only\\n\""
        );
        assert_ne!(
            expected,
            expected_review(Path::new("/elsewhere/bridge-write.txt"))
        );
    }
}

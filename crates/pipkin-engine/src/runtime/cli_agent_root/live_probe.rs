//! Explicitly ignored installed-CLI qualification. No installed process runs in ordinary tests.
//! Two root prompts maximum, no children/jobs/API inference, and only one probe-owned file.
use super::*;
use crate::{
    provider::{BoxFuture, ToolSpec, scripted::Scripted},
    runtime::{cli_agent_runtime::admission::Start, tests::Fixture as EngineFixture},
    tools::{ToolCtx, ToolError, ToolOutput},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};
const CLI: &str = "/home/jason/.local/share/mise/installs/claude/2.1.301/claude";
// Three earlier uncertain attempts remain retained. This is a newly authorized pair.
const BUDGET: &str = "fixtures/claude-cli-probe/native-orchestration-sigint-pair-budget.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    authorized: bool,
    allowance: u8,
    spent: u8,
}
fn budget(path: &Path, reserve: bool, allowance: u8) -> Result<u8, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    // Held until File drops, including the durable reservation before any launch.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("cannot lock live budget".into());
    }
    if !file.metadata().map_err(|e| e.to_string())?.is_file()
        || file.metadata().map_err(|e| e.to_string())?.len() > 4096
    {
        return Err("invalid budget file".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let mut value: Budget = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if !(1..=2).contains(&allowance)
        || !value.authorized
        || value.allowance != allowance
        || value.spent >= value.allowance
    {
        return Err(
            "explicit authorization for this bounded probe is required; do not reset the ledger"
                .into(),
        );
    }
    if reserve {
        value.spent += 1;
        let bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        file.set_len(0).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    Ok(value.spent)
}
struct ProbeWrite {
    inner: Arc<dyn Tool>,
    cwd: PathBuf,
}
impl Tool for ProbeWrite {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if ctx.cwd != self.cwd
                || args.get("path").and_then(Value::as_str) != Some("native-probe.txt")
            {
                return Err(ToolError::new(
                    "This qualification only permits native-probe.txt in its owned project",
                ));
            }
            self.inner.execute(args, ctx).await
        })
    }
}
#[test]
fn live_budget_requires_approval_reserves_durably_and_never_resets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("budget.json");
    std::fs::write(&path, r#"{"authorized":false,"allowance":2,"spent":0}"#).unwrap();
    assert!(budget(&path, false, 2).is_err());
    assert!(budget(&path, true, 2).is_err());
    // This is an owned offline ledger fixture, never the installed probe's authorization.
    std::fs::write(&path, r#"{"authorized":true,"allowance":2,"spent":0}"#).unwrap();
    assert_eq!(budget(&path, false, 2).unwrap(), 0);
    assert_eq!(budget(&path, true, 2).unwrap(), 1);
    assert_eq!(budget(&path, true, 2).unwrap(), 2);
    assert!(budget(&path, true, 2).is_err());
    assert!(budget(&path, false, 2).is_err());
    assert_eq!(
        serde_json::from_slice::<Budget>(&std::fs::read(&path).unwrap())
            .unwrap()
            .spent,
        2
    );
    std::fs::write(&path, b"incomplete reservation").unwrap();
    assert!(budget(&path, true, 2).is_err());
}
#[test]
#[ignore = "requires explicit two-prompt approval; never retry an attempted pair"]
fn installed_cli_native_write_then_resume() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let ledger = repo.join(BUDGET);
    assert_eq!(
        budget(&ledger, false, 2).expect("No installed CLI calls without renewed authorization"),
        0,
        "A probe has already been attempted; inspect its retained outcome instead of restarting it"
    );
    for prior in [
        "native-orchestration-budget.json",
        "native-orchestration-close-budget.json",
        "native-orchestration-signal-budget.json",
    ] {
        let value: Budget = serde_json::from_slice(
            &std::fs::read(repo.join("fixtures/claude-cli-probe").join(prior)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            value.spent, 1,
            "inspect earlier consumption before the new authorized pair"
        );
    }
    let root = repo.join("dist/cli-sigint-live/live");
    std::fs::create_dir_all(&root).unwrap();
    let retained = tempfile::Builder::new()
        .prefix("probe-")
        .tempdir_in(&root)
        .unwrap()
        .keep();
    let project = retained.join("project");
    std::fs::create_dir(&project).unwrap();
    let fx = EngineFixture::new();
    let provider = Scripted::echo("API fallback is forbidden");
    let mut config = fx.config();
    config.data_dir = retained.join("data");
    config.config_dir = retained.join("config");
    config.enable_local_claude_cli = true;
    config.enable_local_claude_cli_orchestration = true;
    let engine = fx.start_config(&provider, config);
    let shared = engine.handle.shared().clone();
    shared.handle.block_on(async {
        let owner=*shared.clients.lock().unwrap().keys().next().unwrap();
        let environment:Environment=std::env::vars_os().collect();
        let prepared=preflight::check_orchestration(Request {
            candidate:Candidate::inspect(Path::new(CLI),&project,&environment).unwrap(),
            trust_executable:true,trust_project:true,environment,model:ModelAlias::Sonnet,
        },CancellationToken::new()).await.expect("installed CLI metadata preflight");
        let binding=shared.cli_agents.create_approved(&shared.store,owner,&prepared,"live-probe","live-create").unwrap().0;
        assert_eq!(binding.create.identity.cli_version,"2.1.301");
        let image=HelperImage::capture_test_binary_in(&retained,&repo.join("target/debug/pipkin-engine")).unwrap();
        let write=crate::tools::builtin_tools().into_iter().find(|t|t.spec().name=="write").unwrap();
        let tools:Vec<Arc<dyn Tool>>=vec![Arc::new(ProbeWrite {inner:write,cwd:project.clone()})];
        let mut receipts=Vec::new();
        for (index,marker) in ["native-first","native-resumed"].into_iter().enumerate() {
            let ownership=shared.cli_agents.begin_turn(&shared.store,owner,"live-probe").unwrap();
            // An attempt consumes allowance before submission, even if preparation/launch fails.
            // No service in this exact catalog can launch children or further model processes.
            assert_eq!(budget(&ledger,true,2).expect("persistent live allowance exhausted"), (index+1) as u8);
            let result=shared.submit_cli_agent_root(Start { images: vec![],
                ownership,request:format!("live-{}",index+1),generation:1,operation:crate::util::uuid_v4(),
                text:format!("Use mcp__pipkin__write to write exactly {marker} to the relative path native-probe.txt. Then reply done. Do not call any other tool or do any other work."),
                system:"This is an explicitly authorized integration probe in an isolated temporary project. Use only the provided native write tool for the requested file. Do not inspect credentials, configuration or other files.".into(),
                assets_parent:retained.clone(),image:image.clone(),tools:tools.clone(),services:Services::default(),
                deadline:Instant::now()+Duration::from_secs(90),cancel:CancellationToken::new(),test_peer:None,test_preparing:None,
            }).await.unwrap();
            std::fs::write(retained.join(format!("turn-{}.json",index+1)),serde_json::to_vec_pretty(&json!({"turn":index+1,"outcome":format!("{result:?}"),"cliVersion":"2.1.301","model":"sonnet"})).unwrap()).unwrap();
            let receipt=result.expect("real CLI/SDK/MCP lifecycle must finish with native receipts");
            assert_eq!(receipt.state,cli_agent_turns::State::Completed);
            assert_eq!(receipt.mode,if index==0 {crate::execution::claude_cli::policy::TurnMode::Fresh} else {crate::execution::claude_cli::policy::TurnMode::Resume});
            assert_eq!(std::fs::read_to_string(project.join("native-probe.txt")).unwrap().trim_end(),marker);
            receipts.push(receipt.scope.run);
        }
        assert_eq!(shared.store.cli_agent_binding("live-probe").unwrap().unwrap(),binding);
        std::fs::write(retained.join("result.json"),serde_json::to_vec_pretty(&json!({"passed":true,"model":"sonnet","cliVersion":"2.1.301","runs":receipts,"sameCliUuid":true,"nativeWrite":true,"apiRequests":provider.requests().len()})).unwrap()).unwrap();
    });
    assert!(provider.requests().is_empty());
    engine.handle.shutdown(Duration::from_secs(8));
}

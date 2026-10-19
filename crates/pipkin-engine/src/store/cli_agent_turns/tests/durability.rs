use super::*;
#[tokio::test]
async fn root_cut_child() {
    let Ok(path) = std::env::var("PIPKIN_TEST_CLI_ROOT_CUT_DIR") else {
        return;
    };
    assert_eq!(
        std::env::var("PIPKIN_TEST_CLI_ROOT_CUT_MARKER").unwrap(),
        "root-journal-v1"
    );
    let cut = std::env::var("PIPKIN_TEST_CLI_ROOT_CUT").unwrap();
    let dir = std::path::PathBuf::from(path);
    let s = Store::open(&dir.join("db")).unwrap();
    let b = s
        .create_cli_agent_session(&cli_agent::tests::create("s", dir.to_str().unwrap()), 1)
        .unwrap()
        .0;
    let l = admit(&s, &b, "r");
    if cut == "admitted" {
        std::process::exit(0);
    }
    let spec = bridge(&s, &l);
    s.begin_cli_agent_launch(&l, &spec.id, 4).unwrap();
    if cut == "launch" {
        std::process::exit(0);
    }
    s.mark_cli_agent_running(&l, 5).unwrap();
    let channel = s.claim_cli_tool_bridge(&spec, 6).unwrap().unwrap();
    let args = json!({"path":"effect","content":"once"});
    let cli_bridge::Admission::New(claim) = s
        .admit_cli_native_call(&channel, &json!(1), "write", &args, 7)
        .unwrap()
    else {
        panic!()
    };
    let ctx = crate::tools::ToolCtx {
        cwd: dir.clone(),
        session: "s".into(),
        call_id: claim.call_id().into(),
        cancel: tokio_util::sync::CancellationToken::new(),
        progress: std::sync::Arc::new(|_| {}),
        env: vec![],
        shell: None,
        locks: std::sync::Arc::new(crate::tools::MutationLocks::default()),
        processes: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        host: None,
    };
    let result = crate::tools::run(&crate::tools::write::WriteTool, args, &ctx).await;
    assert!(!result.is_error);
    if cut == "effect" {
        std::process::exit(0);
    }
    s.settle_cli_native_call(&channel, &claim, &result, 8)
        .unwrap();
    if cut == "toolResult" {
        std::process::exit(0);
    }
    assert!(s.close_cli_tool_bridge(&channel, 9).unwrap());
    s.complete_cli_agent_turn(&l, &evidence(), 10).unwrap();
    std::process::exit(0);
}
#[test]
fn five_process_exit_cuts_keep_unknown_and_never_replay_native_write() {
    for cut in ["admitted", "launch", "effect", "toolResult", "completed"] {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::cli_agent_turns::tests::durability::root_cut_child",
                "--nocapture",
            ])
            .env("PIPKIN_TEST_CLI_ROOT_CUT_DIR", dir.path())
            .env("PIPKIN_TEST_CLI_ROOT_CUT_MARKER", "root-journal-v1")
            .env("PIPKIN_TEST_CLI_ROOT_CUT", cut)
            .status()
            .unwrap();
        assert!(status.success(), "{cut}");
        let s = Store::open(&dir.path().join("db")).unwrap();
        let b = s.cli_agent_binding("s").unwrap().unwrap();
        s.recover_cli_tool_bridges(20).unwrap();
        assert_eq!(
            s.recover_cli_agent_turns(21).unwrap(),
            usize::from(cut != "completed")
        );
        assert_eq!(s.recover_cli_agent_turns(22).unwrap(), 0);
        assert!(s.unsettled_runs().unwrap().is_empty());
        let r = s.cli_agent_turn("s", "r").unwrap().unwrap();
        let expected = match cut {
            "admitted" => State::Rejected,
            "completed" => State::Completed,
            _ => State::Unknown,
        };
        assert_eq!(r.state, expected);
        assert!(
            s.admit_cli_agent_turn(&input(&b, "r"), "new-owner", 23)
                .unwrap()
                .lease
                .is_none()
        );
        let next = s.admit_cli_agent_turn(&input(&b, "next"), "new-owner", 24);
        if expected == State::Unknown {
            assert!(next.is_err());
        } else {
            assert!(next.unwrap().lease.is_some());
        }
        if matches!(cut, "effect" | "toolResult" | "completed") {
            let (call, state): (String, String) = s
                .read(|c| {
                    Ok(
                        c.query_row("SELECT call_id,state FROM cli_bridge_calls", [], |r| {
                            Ok((r.get(0)?, r.get(1)?))
                        })?,
                    )
                })
                .unwrap();
            let tool = s.tool_call("s", &call).unwrap().unwrap();
            if cut == "effect" {
                assert_eq!(state, "unknown");
                assert_eq!(tool.state, ToolState::Interrupted);
                assert!(tool.result_entry.is_none());
            } else {
                assert_eq!(state, "completed");
                assert_eq!(tool.state, ToolState::Done);
                assert!(tool.result_entry.is_some());
            }
            assert_eq!(
                std::fs::read_to_string(dir.path().join("effect")).unwrap(),
                "once"
            );
        } else {
            assert!(!dir.path().join("effect").exists());
        }
        s.integrity_check().unwrap();
    }
}

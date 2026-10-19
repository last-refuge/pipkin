use super::*;
use std::sync::Arc;
// Only the parent test sets this private marker. Actual native WriteTool, no CLI/model.
#[tokio::test]
async fn native_crash_child() {
    let Ok(dir) = std::env::var("PIPKIN_TEST_NATIVE_BRIDGE_CUT_DIR") else {
        return;
    };
    assert_eq!(
        std::env::var("PIPKIN_TEST_NATIVE_BRIDGE_MARKER").unwrap(),
        "native-journal-join-v1"
    );
    let cut = std::env::var("PIPKIN_TEST_NATIVE_BRIDGE_CUT").unwrap();
    let dir = std::path::PathBuf::from(dir);
    let store = Store::open(&dir.join("db")).unwrap();
    let lease = fixture_in(&store, dir.to_str().unwrap());
    let args = json!({"path":"effect","content":"exactly once"});
    let Admission::New(claim) = store
        .admit_cli_native_call(&lease, &json!(1), "write", &args, 4)
        .unwrap()
    else {
        panic!()
    };
    if cut != "intent" {
        let ctx = crate::tools::ToolCtx {
            cwd: dir.clone(),
            session: "s".into(),
            call_id: claim.call_id().into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            progress: Arc::new(|_| {}),
            env: vec![],
            shell: None,
            locks: Arc::new(crate::tools::MutationLocks::default()),
            processes: Arc::new(tokio::sync::Semaphore::new(1)),
            host: None,
        };
        let result = crate::tools::run(&crate::tools::write::WriteTool, args, &ctx).await;
        assert!(!result.is_error);
        if cut == "completed" {
            assert_eq!(
                store
                    .settle_cli_native_call(&lease, &claim, &result, 5)
                    .unwrap()
                    .0,
                Settlement::Committed
            );
        }
    }
    // Bypass all Rust drops / SQLite close. Parent recovers committed WAL from disk.
    std::process::exit(0);
}
#[test]
fn process_exit_preserves_atomic_native_joins_and_never_replays_an_uncertain_effect() {
    for cut in ["intent", "effect", "completed"] {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::cli_bridge::native::tests::durability::native_crash_child",
                "--nocapture",
            ])
            .env("PIPKIN_TEST_NATIVE_BRIDGE_CUT_DIR", dir.path())
            .env("PIPKIN_TEST_NATIVE_BRIDGE_MARKER", "native-journal-join-v1")
            .env("PIPKIN_TEST_NATIVE_BRIDGE_CUT", cut)
            .status()
            .unwrap();
        assert!(status.success(), "{cut}");
        let store = Store::open(&dir.path().join("db")).unwrap();
        let spec = store.read(|c| Ok(stored(c, "b")?.unwrap().0)).unwrap();
        assert_eq!(store.recover_cli_tool_bridges(10).unwrap(), 1);
        assert!(store.claim_cli_tool_bridge(&spec, 11).unwrap().is_none());
        assert!(!store.prepare_cli_tool_bridge(&spec, 11).unwrap());
        let (id, state): (String, String) = store
            .read(|c| {
                Ok(
                    c.query_row("SELECT call_id,state FROM cli_bridge_calls", [], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?,
                )
            })
            .unwrap();
        let tool = store.tool_call("s", &id).unwrap().unwrap();
        assert!(!tool.replay_safe);
        if cut == "completed" {
            assert_eq!(state, "completed");
            assert_eq!(tool.state, ToolState::Done);
            assert!(tool.result_entry.is_some());
            assert_eq!(counts(&store), (1, 1, 2));
            assert!(store.tool_output("s", &id).unwrap().is_some());
        } else {
            assert_eq!(state, "unknown");
            assert_eq!(tool.state, ToolState::Interrupted);
            assert!(tool.result_entry.is_none());
            assert_eq!(counts(&store), (1, 1, 1));
            assert!(store.tool_output("s", &id).unwrap().is_none());
        }
        if cut == "intent" {
            assert!(!dir.path().join("effect").exists());
        } else {
            assert_eq!(
                std::fs::read_to_string(dir.path().join("effect")).unwrap(),
                "exactly once"
            );
        }
        assert_eq!(store.recover_cli_tool_bridges(12).unwrap(), 0);
        store.integrity_check().unwrap();
    }
}

use super::*;
struct JobFixture {
    store: Store,
    binding: cli_agent::Binding,
    run: u64,
    call: String,
}
impl JobFixture {
    fn new() -> Self {
        let store = Store::memory().unwrap();
        let binding = setup(&store);
        let root = admit(&store, &binding, "r");
        let channel = launch(&store, &root);
        let cli_bridge::Admission::New(claim) = store
            .admit_cli_native_call(
                &channel,
                &json!(1),
                "job_start",
                &json!({"command":"fixture","wake":true}),
                7,
            )
            .unwrap()
        else {
            panic!("native claim")
        };
        let call = claim.call_id().to_owned();
        let run = root.receipt().scope.run;
        store
            .tx(|c| {
                let job = Store::job_create(
                    c,
                    "s",
                    &crate::store::jobs::JobIntent {
                        name: None,
                        command: "fixture",
                        cwd: "/project",
                        origin: "model",
                        call_id: Some(&call),
                        run: Some(run),
                        task: None,
                        timeout_secs: None,
                        wake: true,
                        engine_epoch: 1,
                        nonce: "fixture",
                        log_path: "",
                    },
                    8,
                )?;
                Store::job_receipt(c, "s", job.id, 123, 9)?;
                Store::job_set_state(c, "s", job.id, "succeeded", Some(0), None, 10)?;
                Ok(())
            })
            .unwrap();
        store
            .settle_cli_native_call(
                &channel,
                &claim,
                &crate::tools::ToolResult {
                    content: vec![Block::text("Started job #1")],
                    is_error: false,
                    details: None,
                    retained: String::new(),
                    dropped_bytes: 0,
                },
                11,
            )
            .unwrap();
        store.close_cli_tool_bridge(&channel, 12).unwrap();
        store
            .complete_cli_agent_turn(&root, &evidence(), 13)
            .unwrap();
        Self {
            store,
            binding,
            run,
            call,
        }
    }
    fn wake(&self) -> Result<Wake> {
        self.store.read(|c| {
            Wake::capture(
                c,
                WakeTarget::Job(1),
                "s",
                &self.call,
                self.run,
                "Background job #1 finished.".into(),
            )
        })
    }
    fn input(&self) -> Input<'_> {
        Input {
            images: &[],
            mode: InputMode::FollowUp,
            session: "s",
            request: "cli-job-wake:1",
            generation: 1,
            operation: "wake",
            binding: self.binding.digest,
            text: "Background job #1 finished.",
        }
    }
}
#[test]
fn wake_admission_delivery_and_canonical_input_commit_once_and_rollback_together() {
    let f = JobFixture::new();
    let wake = f.wake().unwrap();
    let before = counts(&f.store);
    f.store.fail_next_write();
    assert!(
        f.store
            .admit_cli_agent_turn_with_wake(&f.input(), "owner", Some(&wake), 14)
            .is_err()
    );
    assert_eq!(counts(&f.store), before);
    assert_eq!(
        f.store
            .read(|c| Store::job(c, "s", 1))
            .unwrap()
            .unwrap()
            .delivery,
        "pending"
    );
    let first = f
        .store
        .admit_cli_agent_turn_with_wake(&f.input(), "owner", Some(&wake), 15)
        .unwrap();
    let lease = first.lease.unwrap();
    assert!(first.job_revision.is_some());
    assert!(first.parent_task_revision.is_none());
    assert_eq!(lease.receipt().mode, TurnMode::Resume);
    assert_eq!(
        f.store.cli_agent_launch_context(&lease).unwrap().prompt(),
        f.input().text
    );
    let entry = f
        .store
        .entry("s", lease.receipt().user_entry)
        .unwrap()
        .unwrap();
    assert!(matches!(
        entry.kind,
        EntryKind::Message {
            mode: InputMode::FollowUp,
            ..
        }
    ));
    let after = counts(&f.store);
    let repeated = f
        .store
        .admit_cli_agent_turn_with_wake(&f.input(), "different-owner", Some(&wake), 16)
        .unwrap();
    assert!(repeated.lease.is_none());
    assert!(repeated.job_revision.is_none());
    assert_eq!(counts(&f.store), after);
    assert_eq!(
        f.store
            .read(|c| Store::job(c, "s", 1))
            .unwrap()
            .unwrap()
            .delivery,
        "woken"
    );
    assert!(f.wake().is_err());
    let mut prompt = f.input();
    prompt.mode = InputMode::Prompt;
    assert!(f.store.lookup_cli_agent_turn(&prompt).is_err());
    assert!(
        f.store
            .admit_cli_agent_turn(&prompt, "another", 17)
            .is_err()
    );
    assert!(
        f.store
            .admit_cli_agent_turn(&f.input(), "another", 17)
            .is_err()
    );
    let mut different = f.input();
    different.text = "changed wake";
    assert!(
        f.store
            .admit_cli_agent_turn_with_wake(&different, "owner", Some(&wake), 17)
            .is_err()
    );
    assert_eq!(counts(&f.store), after);
}
#[test]
fn changed_completion_failed_handoff_unknown_parent_and_stop_cannot_wake() {
    for mutation in [
        "UPDATE jobs SET reason='changed' WHERE id=1",
        "UPDATE jobs SET name='changed' WHERE id=1",
        "UPDATE jobs SET nonce='different' WHERE id=1",
        "UPDATE jobs SET state='unknown' WHERE id=1",
        "UPDATE jobs SET delivery='suppressed' WHERE id=1",
        "UPDATE jobs SET call_id='printed-sdk-id' WHERE id=1",
        "UPDATE cli_bridge_calls SET result=json_set(result,'$.isError',1)",
        "UPDATE tool_calls SET state='interrupted'",
        "UPDATE cli_agent_turns SET fenced=1",
        "UPDATE cli_agent_turns SET state='unknown',evidence=NULL",
        "UPDATE runs SET stop_requested_at=99",
    ] {
        let f = JobFixture::new();
        let wake = f.wake().unwrap();
        let before = counts(&f.store);
        f.store
            .tx(|c| {
                // Deliberately corrupt this owned fixture past immutable-row triggers.
                // Admission must still reject it; production never removes these guards.
                c.execute_batch("DROP TRIGGER cli_bridge_call_immutable; DROP TRIGGER cli_agent_turn_immutable;")?;
                c.execute_batch(mutation)?;
                Ok(())
            })
            .unwrap();
        assert!(
            f.store
                .admit_cli_agent_turn_with_wake(&f.input(), "owner", Some(&wake), 20)
                .is_err(),
            "{mutation}"
        );
        assert_eq!(counts(&f.store), before, "{mutation}");
    }
}
#[test]
fn prompt_fingerprint_is_compatible_and_cross_mode_is_never_a_duplicate() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let original = input(&b, "r");
    let prior: [u8; 32] = Sha256::digest(
        serde_json::to_vec(&serde_json::json!([
            "pipkin-cli-agent-turn-v1",
            original.session,
            original.request,
            original.generation,
            original.operation,
            original.binding,
            original.text
        ]))
        .unwrap(),
    )
    .into();
    assert_eq!(input_digest(&original), prior);
    admit(&s, &b, "r");
    let mut changed = input(&b, "r");
    changed.mode = InputMode::FollowUp;
    assert!(s.lookup_cli_agent_turn(&changed).is_err());
}

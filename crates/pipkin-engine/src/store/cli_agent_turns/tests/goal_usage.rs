use super::*;
fn goal(s: &Store, b: &cli_agent::Binding) -> (GoalView, Lease) {
    let g = s
        .tx(|c| {
            Store::goal_insert(
                c,
                "s",
                "offline goal",
                GoalState::Active,
                None,
                "fixture",
                "replace",
                1,
            )
        })
        .unwrap();
    let text = crate::goals::turn_prompt_structured(&g.objective, &g.criteria);
    let key = crate::goals::request_key(g.id, 1);
    let input = Input {
        images: &[],
        session: "s",
        request: &key,
        generation: 1,
        operation: "goal-op",
        binding: b.digest,
        text: &text,
        mode: InputMode::FollowUp,
    };
    let lease = s
        .admit_cli_agent_turn_with_goal(
            &input,
            "owner",
            None,
            None,
            Some(&GoalTurn { view: g.clone() }),
            2,
        )
        .unwrap()
        .lease
        .unwrap();
    (g, lease)
}
#[test]
fn background_usage_receipts_distinguish_pending_rejected_unknown_partial_and_saturation() {
    for case in [
        "complete",
        "partial",
        "overflow",
        "unknown",
        "skipped-with-worker-failure",
        "rejected",
    ] {
        let s = Store::memory().unwrap();
        let b = setup(&s);
        let (g, l) = goal(&s, &b);
        let usage = s.read(|c| Store::cli_goal_usage_in(c, "s", g.id)).unwrap();
        assert_eq!(usage.active, 1);
        assert!(!usage.uncertain);
        assert!(!usage.complete);
        if case == "rejected" {
            s.abandon_cli_agent_turn(&l, 10).unwrap();
        } else {
            let bridge = launch(&s, &l);
            if matches!(case, "unknown" | "skipped-with-worker-failure") {
                s.tx(|c| Store::goal_set_state(c, "s", g.id, GoalState::Completed, None, 9))
                    .unwrap();
                if case == "skipped-with-worker-failure" {
                    s.tx(|c| {
                        let mut view = Store::goal_current(c, "s")?.unwrap();
                        view.audit.result = Some(pipkin_engine_protocol::GoalAuditResult {
                            round: 1,
                            verdict: "skipped".into(),
                            findings: vec![],
                            summary: None,
                            fingerprint: None,
                            child: None,
                        });
                        Store::goal_write_progress(c, "s", &view, 9)
                    })
                    .unwrap();
                }
                s.abandon_cli_agent_turn(&l, 10).unwrap();
            } else {
                s.close_cli_tool_bridge(&bridge, 10).unwrap();
                let mut e = evidence();
                e.terminal.usage = Some(crate::store::ExternalTerminalUsage {
                    reasoning_tokens: Some(2),
                    cache_write_1h: Some(4),
                    input_tokens: if case == "partial" { None } else { Some(2) },
                    output_tokens: Some(if case == "overflow" { u64::MAX } else { 3 }),
                    cache_read_input_tokens: Some(5),
                    cache_creation_input_tokens: Some(7),
                });
                e.terminal.metadata = crate::execution::claude_cli::usage_metadata::parse(
                    &json!({"total_cost_usd":999,"modelUsage":{"sonnet":{"inputTokens":100000}}})
                        .to_string(),
                );
                s.complete_cli_agent_turn(&l, &e, 11).unwrap();
            }
        }
        // A crash between receipt commitment and runtime publication still reconciles at recovery.
        assert_eq!(
            s.read(|c| Store::goal_current(c, "s"))
                .unwrap()
                .unwrap()
                .usage_tokens,
            0
        );
        s.recover_cli_agent_turns(20).unwrap();
        let usage = s.read(|c| Store::cli_goal_usage_in(c, "s", g.id)).unwrap();
        if matches!(case, "unknown" | "skipped-with-worker-failure") {
            assert_eq!(
                s.read(|c| Store::goal_current(c, "s"))
                    .unwrap()
                    .unwrap()
                    .state,
                GoalState::Paused
            );
        }
        let expected = match case {
            "complete" => 17,
            "partial" => 15,
            "overflow" => i64::MAX as u64,
            _ => 0,
        };
        assert_eq!(usage.known_tokens, expected);
        assert_eq!(usage.active, 0);
        assert_eq!(usage.complete, matches!(case, "complete" | "rejected"));
        assert_eq!(usage.uncertain, !matches!(case, "complete" | "rejected"));
        assert_eq!(
            s.read(|c| Store::goal_current(c, "s"))
                .unwrap()
                .unwrap()
                .usage_tokens,
            expected
        );
        assert!(
            s.tx(|c| Store::cli_goal_reconcile_run_in(c, "s", l.receipt.scope.run, 21))
                .unwrap()
                .is_empty()
        );
        s.recover_cli_agent_turns(22).unwrap();
        assert_eq!(
            s.read(|c| Store::goal_current(c, "s"))
                .unwrap()
                .unwrap()
                .usage_tokens,
            expected
        );
    }
}
#[test]
fn background_usage_prelaunch_rechecks_goal_allowance_and_reserved_request_names() {
    for limit in ["cost", "turns", "paused", "replaced"] {
        let s = Store::memory().unwrap();
        let b = setup(&s);
        assert!(
            s.admit_cli_agent_turn(&input(&b, "goal:1:1"), "owner", 1)
                .is_err()
        );
        assert!(
            s.admit_cli_agent_turn(&input(&b, "cli-job-wake:1"), "owner", 1)
                .is_err()
        );
        let (g, l) = goal(&s, &b);
        let spec = bridge(&s, &l);
        s.tx(|c| {
            let mut g = Store::goal_current(c, "s")?.unwrap();
            match limit {
                "cost" => {
                    g.limits.max_cost_usd = Some(1.0);
                    Store::goal_write_progress(c, "s", &g, 3)?;
                }
                "turns" => {
                    g.limits.max_turns = Some(0);
                    Store::goal_write_progress(c, "s", &g, 3)?;
                }
                "paused" => Store::goal_set_state(c, "s", g.id, GoalState::Paused, None, 3)?,
                _ => {
                    Store::goal_insert(
                        c,
                        "s",
                        "replacement",
                        GoalState::Active,
                        None,
                        "user",
                        "replace",
                        3,
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();
        assert!(
            s.begin_cli_agent_launch(&l, &spec.id, 4).is_err(),
            "{limit}"
        );
        assert_eq!(
            s.cli_agent_turn("s", &crate::goals::request_key(g.id, 1))
                .unwrap()
                .unwrap()
                .state,
            State::Admitted
        );
    }
}

#[test]
fn background_usage_preserves_previously_accounted_minimum_for_allowances() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let (g, l) = goal(&s, &b);
    s.abandon_cli_agent_turn(&l, 5).unwrap();
    s.tx(|c| {
        let mut view = Store::goal_current(c, "s")?.unwrap();
        view.usage_tokens = 20;
        view.limits.max_tokens = Some(20);
        Store::goal_write_progress(c, "s", &view, 6)
    })
    .unwrap();
    s.recover_cli_agent_turns(7).unwrap();
    assert_eq!(
        s.read(|c| Store::goal_current(c, "s"))
            .unwrap()
            .unwrap()
            .usage_tokens,
        20
    );
    assert!(
        s.read(|c| Store::cli_goal_allow_in(c, "s", g.id, true))
            .is_err()
    );
}

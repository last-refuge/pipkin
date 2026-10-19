use super::*;
#[test]
fn stored_inputs_are_redacted_and_mode_is_not_a_callers_choice() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let context = s.cli_agent_launch_context(&l).unwrap();
    assert_eq!(context.prompt(), input(&b, "r").text);
    assert_eq!(context.mode(), TurnMode::Fresh);
    assert_eq!(context.scope(), &l.receipt.scope);
    assert_eq!(context.binding().digest, b.digest);
    assert_eq!(format!("{context:?}"), "CliAgentLaunchContext([redacted])");
    let bridge = launch(&s, &l);
    assert!(s.cli_agent_launch_context(&l).is_err());
    s.close_cli_tool_bridge(&bridge, 7).unwrap();
    assert_eq!(
        s.complete_cli_agent_turn(&l, &evidence(), 8).unwrap(),
        Settlement::Committed
    );
    let second = admit(&s, &b, "next");
    assert_eq!(
        s.cli_agent_launch_context(&second).unwrap().mode(),
        TurnMode::Resume
    );
    assert!(s.cli_agent_launch_context(&l).is_err());
}
#[test]
fn stopped_rejected_and_foreign_owners_cannot_obtain_launch_inputs() {
    for case in 0..3 {
        let s = Store::memory().unwrap();
        let b = setup(&s);
        let mut l = admit(&s, &b, "r");
        match case {
            0 => {
                l.owner = "foreign".into();
            }
            1 => {
                s.tx(|c| {
                    c.execute("UPDATE runs SET stop_requested_at=4", [])?;
                    Ok(())
                })
                .unwrap();
            }
            _ => {
                s.abandon_cli_agent_turn(&l, 4).unwrap();
            }
        }
        assert!(s.cli_agent_launch_context(&l).is_err());
    }
}
#[test]
fn launch_cas_rechecks_changed_request_and_canonical_user_entry_after_snapshot() {
    for sql in [
        "UPDATE requests SET text='different'",
        "UPDATE requests SET fingerprint='different'",
        "UPDATE requests SET model='haiku'",
        "UPDATE requests SET images='[{}]'",
        "UPDATE requests SET mode='steer'",
        "UPDATE requests SET state='settled'",
        "UPDATE requests SET run=NULL",
        "UPDATE requests SET outcome='{}'",
        "UPDATE requests SET queue_entry=1",
        "UPDATE runs SET request='foreign'",
        "UPDATE entries SET retired=1",
        "UPDATE entries SET partial=1",
        "UPDATE entries SET body='{}'",
        "UPDATE entries SET run=NULL",
    ] {
        let s = Store::memory().unwrap();
        let b = setup(&s);
        let l = admit(&s, &b, "r");
        let spec = bridge(&s, &l);
        let context = s.cli_agent_launch_context(&l).unwrap();
        s.tx(|c| {
            c.execute(sql, [])?;
            Ok(())
        })
        .unwrap();
        assert!(s.cli_agent_launch_context(&l).is_err(), "{sql}");
        assert!(s.begin_cli_agent_launch(&l, &spec.id, 5).is_err(), "{sql}");
        assert_eq!(
            s.cli_agent_turn("s", "r").unwrap().unwrap().state,
            State::Admitted
        );
        assert_eq!(context.prompt(), "private root instruction");
    }
}
#[test]
fn internally_consistent_payload_replacement_still_conflicts_with_permanent_admission() {
    let s = Store::memory().unwrap();
    let b = setup(&s);
    let l = admit(&s, &b, "r");
    let spec = bridge(&s, &l);
    let replacement = EntryKind::Message {
        message: Message::User {
            content: vec![Block::text("replacement")],
            timestamp: 2,
            extra: Default::default(),
        },
        mode: InputMode::Prompt,
        request: Some("r".into()),
        partial: false,
    };
    s.tx(|c| {
        c.execute(
            "UPDATE requests SET text='replacement',fingerprint=?1",
            [fingerprint(InputMode::Prompt, "replacement", &[])],
        )?;
        c.execute("UPDATE entries SET body=?1", [json(&replacement)])?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        s.cli_agent_launch_context(&l),
        Err(StoreError::ExecutionConflict)
    ));
    assert!(s.begin_cli_agent_launch(&l, &spec.id, 5).is_err());
}
#[test]
fn launch_context_rejects_oversized_payloads_before_loading_or_parsing_them() {
    for sql in [
        "UPDATE requests SET text=CAST(zeroblob(?1) AS TEXT)",
        "UPDATE requests SET model=CAST(zeroblob(?1) AS TEXT)",
        "UPDATE requests SET fingerprint=CAST(zeroblob(?1) AS TEXT)",
        "UPDATE entries SET body=json_object('oversize',replace(hex(zeroblob(?1 / 2 + 1)),'0','x'))",
    ] {
        let s = Store::memory().unwrap();
        let b = setup(&s);
        let l = admit(&s, &b, "r");
        s.tx(|c| {
            c.execute(sql, [(MAX_PROMPT_BYTES * 6 + 4097) as i64])?;
            Ok(())
        })
        .unwrap();
        assert!(matches!(
            s.cli_agent_launch_context(&l),
            Err(StoreError::ExecutionConflict)
        ));
    }
}

use std::cell::RefCell;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;
use crate::store::ExternalRunKey;

const UUID: &str = "11111111-1111-4111-8111-111111111111";

fn cli() -> CliScope {
    CliScope::new(UUID, "/captured/project").unwrap()
}
fn scope() -> RunScope {
    RunScope {
        epoch: 7,
        owner: 12,
        key: ExternalRunKey {
            session: "conversation".into(),
            generation: 3,
            op: "op-1".into(),
        },
        run_id: "run-1".into(),
        cli: cli(),
    }
}
fn envelope(content: &str) -> Value {
    json!({
        "session_id": UUID,
        "cwd": "/captured/project",
        "hook_event_name": "PermissionRequest",
        "tool_name": "Write",
        "tool_input": {"file_path": "/captured/project/file.txt", "content": content}
    })
}
fn parse(value: Value) -> Result<PermissionRequest, RequestError> {
    PermissionRequest::parse(&serde_json::to_vec(&value).unwrap(), &cli())
}
fn request() -> PermissionRequest {
    parse(envelope("private arguments")).unwrap()
}
fn ok<T: ?Sized>(_: &T) -> Result<(), ()> {
    Ok(())
}
fn gate(limits: GateLimits) -> PermissionGate {
    PermissionGate::new(scope(), limits, 10, ok).unwrap()
}
fn admit(gate: &mut PermissionGate, n: u8) -> Review {
    gate.admit(&scope(), [n; 32], request(), 10, 100).unwrap()
}
fn no_persist(_: &DecisionRecord) -> Result<(), ()> {
    panic!("unexpected persistence")
}
fn no_delivery(_: &[u8]) -> Result<(), ()> {
    panic!("unexpected delivery")
}

#[test]
fn ticket_and_decision_debug_do_not_expose_nonces_or_fingerprints() {
    let mut gate = gate(GateLimits::default());
    let review = admit(&mut gate, 173);
    assert_eq!(format!("{:?}", review.ticket), "Ticket([redacted])");
    let record = DecisionRecord {
        ticket: review.ticket,
        decision: Decision::AllowOnce,
        deadline: 100,
    };
    assert_eq!(format!("{record:?}"), "DecisionRecord([redacted])");
}

#[test]
fn malformed_nonobject_trailing_and_oversize_are_rejected() {
    for bytes in [
        b"".as_slice(),
        b"{",
        b"[]",
        b"null",
        b"true",
        b"\xff",
        b"{} trailing",
    ] {
        assert_eq!(
            PermissionRequest::parse(bytes, &cli()).unwrap_err(),
            RequestError::InvalidJson
        );
    }
    let mut bytes = serde_json::to_vec(&envelope("ok")).unwrap();
    bytes.resize(MAX_INPUT_BYTES, b' ');
    assert!(PermissionRequest::parse(&bytes, &cli()).is_ok());
    bytes.push(b' ');
    assert_eq!(
        PermissionRequest::parse(&bytes, &cli()).unwrap_err(),
        RequestError::TooLarge
    );
}

#[test]
fn duplicate_keys_at_both_levels_including_escaped_names_fail_closed() {
    let base = serde_json::to_string(&envelope("ok")).unwrap();
    for field in [
        "session_id",
        "cwd",
        "hook_event_name",
        "tool_name",
        "tool_input",
    ] {
        let duplicate = format!("{{\"{field}\":{},{}", envelope("ok")[field], &base[1..]);
        assert_eq!(
            PermissionRequest::parse(duplicate.as_bytes(), &cli()).unwrap_err(),
            RequestError::InvalidJson
        );
    }
    for replacement in [
        "\"content\":\"ok\",\"content\":\"ok\"",
        "\"content\":\"ok\",\"cont\\u0065nt\":\"different\"",
        "\"content\":\"ok\",\"file_path\":\"other\"",
    ] {
        let bytes = base.replace("\"content\":\"ok\"", replacement);
        assert_eq!(
            PermissionRequest::parse(bytes.as_bytes(), &cli()).unwrap_err(),
            RequestError::InvalidJson
        );
    }
}

#[test]
fn exact_closed_write_and_envelope_only() {
    for field in [
        "session_id",
        "cwd",
        "hook_event_name",
        "tool_name",
        "tool_input",
    ] {
        let mut value = envelope("ok");
        value.as_object_mut().unwrap().remove(field);
        assert!(parse(value).is_err());
    }
    for field in [
        "tool_use_id",
        "agent_id",
        "agent_type",
        "mcp_server",
        "permission_suggestions",
        "unexpected",
    ] {
        let mut value = envelope("ok");
        value[field] = json!("unqualified");
        assert_eq!(parse(value).unwrap_err(), RequestError::InvalidJson);
    }
    for field in ["content", "file_path"] {
        for wrong in [Value::Null, json!(12), json!(false), json!([]), json!({})] {
            let mut value = envelope("ok");
            value["tool_input"][field] = wrong;
            assert!(parse(value).is_err());
        }
        let mut value = envelope("ok");
        value["tool_input"].as_object_mut().unwrap().remove(field);
        assert!(parse(value).is_err());
    }
    for wrong in [
        json!(["path", "content"]),
        json!({"file_path":"p","content":"c","extra":true}),
    ] {
        let mut value = envelope("ok");
        value["tool_input"] = wrong;
        assert!(parse(value).is_err());
    }
    for (field, wrong) in [
        ("tool_name", "Edit"),
        ("tool_name", "mcp__Write"),
        ("hook_event_name", "PreToolUse"),
    ] {
        let mut value = envelope("ok");
        value[field] = json!(wrong);
        assert_eq!(parse(value).unwrap_err(), RequestError::Unsupported);
    }
    let sequence = json!([UUID, "/captured/project", "PermissionRequest", "Write", {"file_path":"p","content":"c"}]);
    assert!(parse(sequence).is_err());
}

#[test]
fn documented_common_metadata_is_inert_bounded_and_full_digest_bound() {
    let base = envelope("ok");
    let base_display = parse(base.clone()).unwrap().argument_display().to_owned();
    let mut with = base.clone();
    with["transcript_path"] = json!("/never/open/credentials-or-transcript");
    with["scratchpad_dir"] = json!("/never/open/scratchpad");
    with["prompt_id"] = json!("provider-prompt-not-a-host-tool-id");
    with["permission_mode"] = json!("default");
    with["effort"] = json!({"level":"high"});
    with["permission_suggestions"] = json!([
        {"type":"setMode","mode":"bypassPermissions","destination":"session"},
        {"type":"addRules","rules":[{"toolName":"Bash"}],"behavior":"allow","destination":"localSettings"},
        {"futureSuggestion":{"array":[null,true,1,-1,1.5,"inert"]}}
    ]);
    let request = parse(with.clone()).unwrap();
    assert_eq!(request.argument_display(), base_display);
    assert_ne!(request.fingerprint(), parse(base).unwrap().fingerprint());
    assert!(!request.argument_display().contains("bypassPermissions"));
    for field in [
        "transcript_path",
        "scratchpad_dir",
        "prompt_id",
        "permission_mode",
        "effort",
        "permission_suggestions",
    ] {
        let mut invalid = with.clone();
        invalid[field] = Value::Null;
        assert!(parse(invalid).is_err(), "{field}");
    }
    for mode in [
        "acceptEdits",
        "plan",
        "auto",
        "dontAsk",
        "bypassPermissions",
        "manual",
        "unknown",
    ] {
        let mut invalid = with.clone();
        invalid["permission_mode"] = json!(mode);
        assert_eq!(parse(invalid).unwrap_err(), RequestError::Unsupported);
    }
    for effort in [
        json!(["high"]),
        json!({"level":"future"}),
        json!({"level":"high","extra":true}),
    ] {
        let mut invalid = with.clone();
        invalid["effort"] = effort;
        assert!(parse(invalid).is_err());
    }
    let raw = serde_json::to_string(&envelope("ok")).unwrap();
    for metadata in [
        r#""permission_suggestions":[{"nested":{"a":1,"a":2}}]"#,
        r#""permission_suggestions":[{"a":1,"\u0061":2}]"#,
        r#""effort":{"level":"high","level":"low"}"#,
        r#""permission_mode":"default","permission_mode":"default""#,
    ] {
        let input = format!("{},{} }}", &raw[..raw.len() - 1], metadata);
        assert_eq!(
            PermissionRequest::parse(input.as_bytes(), &cli()).unwrap_err(),
            RequestError::InvalidJson
        );
    }
}

#[test]
fn uuid_and_cwd_are_validated_and_matched_without_io_or_normalization() {
    for uuid in [
        "",
        "not-a-uuid",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
        "11111111111141118111111111111111",
    ] {
        assert_eq!(
            CliScope::new(uuid, "/a").unwrap_err(),
            RequestError::InvalidScope
        );
    }
    for cwd in [
        "", "relative", "//a", "/a/", "/a/./b", "/a/../b", "/a//b", "/a\0b",
    ] {
        assert_eq!(
            CliScope::new(UUID, cwd).unwrap_err(),
            RequestError::InvalidScope
        );
    }
    assert!(CliScope::new(UUID, "/").is_ok());
    assert!(CliScope::new(UUID, "/nonexistent/no-filesystem-probe").is_ok());
    for (field, wrong) in [
        ("session_id", "22222222-2222-4222-8222-222222222222"),
        ("session_id", "bad"),
        ("cwd", "/captured/project/"),
        ("cwd", "/captured/./project"),
        ("cwd", "/other"),
    ] {
        let mut value = envelope("ok");
        value[field] = json!(wrong);
        assert_eq!(parse(value).unwrap_err(), RequestError::WrongScope);
    }
}

#[test]
fn full_sha256_and_complete_inert_unicode_control_display() {
    let content = "\0\n\r\t\u{1b}[31m\u{7f}\u{85}\u{202e}\u{2066}\u{200b}é😀\"\\<b>**$(cmd)";
    let bytes = serde_json::to_vec(&envelope(content)).unwrap();
    let parsed = PermissionRequest::parse(&bytes, &cli()).unwrap();
    assert_eq!(
        parsed.fingerprint(),
        <[u8; 32]>::from(Sha256::digest(&bytes))
    );
    assert_eq!(
        parsed.fingerprint(),
        PermissionRequest::parse(&bytes, &cli())
            .unwrap()
            .fingerprint()
    );
    assert_eq!(
        parsed.argument_display(),
        "Write\nfile_path: \"/captured/project/file.txt\"\ncontent: \"\\u{0}\\n\\r\\t\\u{1b}[31m\\u{7f}\\u{85}\\u{202e}\\u{2066}\\u{200b}\\u{e9}\\u{1f600}\\\"\\\\<b>**$(cmd)\""
    );
    assert!(parsed.argument_display().is_ascii());
    assert_eq!(parsed.argument_display().lines().count(), 3);
    let mut spaced = bytes.clone();
    spaced.push(b' ');
    assert_ne!(
        parsed.fingerprint(),
        PermissionRequest::parse(&spaced, &cli())
            .unwrap()
            .fingerprint()
    );
    let long = "a".repeat(20_000);
    assert_ne!(
        parse(envelope(&(long.clone() + "x")))
            .unwrap()
            .fingerprint(),
        parse(envelope(&(long + "y"))).unwrap().fingerprint()
    );
    for debug in [
        format!("{parsed:?}"),
        format!("{:?}", cli()),
        format!("{:?}", scope()),
    ] {
        for secret in [UUID, "/captured/project", content] {
            assert!(!debug.contains(secret));
        }
    }
}

#[test]
fn display_bound_is_exact_and_never_truncates_an_approvable_request() {
    let overhead = parse(envelope("")).unwrap().argument_display().len();
    let text = "x".repeat(MAX_DISPLAY_BYTES - overhead);
    let parsed = parse(envelope(&text)).unwrap();
    assert_eq!(parsed.argument_display().len(), MAX_DISPLAY_BYTES);
    assert!(parsed.argument_display().ends_with(&(text.clone() + "\"")));
    assert_eq!(
        parse(envelope(&(text + "x"))).unwrap_err(),
        RequestError::DisplayTooLarge
    );
    // Small wire input can expand beyond the review limit: no truncated fallback.
    assert_eq!(
        parse(envelope(&"\u{202e}".repeat(5000))).unwrap_err(),
        RequestError::DisplayTooLarge
    );
}

#[test]
fn allow_consumes_before_delivery_and_never_reissues() {
    let mut gate = gate(GateLimits::default());
    let review = admit(&mut gate, 1);
    assert_eq!(review.argument_display(), request().argument_display());
    let persisted = RefCell::new(None);
    gate.decide(&review.ticket, Decision::AllowOnce, 11, |record| {
        *persisted.borrow_mut() = Some(record.clone());
        Ok::<_, ()>(())
    }, |bytes| {
        let saved = persisted.borrow();
        let saved = saved.as_ref().unwrap();
        assert_eq!(saved.decision, Decision::AllowOnce);
        assert_eq!(saved.ticket, review.ticket);
        assert_eq!(saved.deadline, 100);
        let wire: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(wire, json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}));
        let text = std::str::from_utf8(bytes).unwrap();
        for forbidden in [UUID, "private arguments", "tool_use_id", "updatedInput", "updatedPermissions", "Completed"] {
            assert!(!text.contains(forbidden));
        }
        Ok::<_, ()>(())
    }).unwrap();
    for decision in [Decision::AllowOnce, Decision::Deny] {
        assert_eq!(
            gate.decide(&review.ticket, decision, 12, no_persist, no_delivery),
            Err(GateError::NotPending)
        );
    }
    assert!(matches!(
        gate.admit(&scope(), [1; 32], request(), 12, 100),
        Err(GateError::Duplicate)
    ));
}

#[test]
fn wrong_scope_nonce_and_changed_args_do_not_consume() {
    let mut gate = gate(GateLimits::default());
    let review = admit(&mut gate, 1);
    for i in 0..7 {
        let mut ticket = review.ticket.clone();
        match i {
            0 => ticket.scope.epoch += 1,
            1 => ticket.scope.owner += 1,
            2 => ticket.scope.key.generation += 1,
            3 => ticket.scope.key.session = "other".into(),
            4 => ticket.scope.key.op = "other".into(),
            5 => ticket.scope.run_id = "other".into(),
            _ => ticket.scope.cli = CliScope::new(UUID, "/other").unwrap(),
        }
        assert_eq!(
            gate.decide(&ticket, Decision::AllowOnce, 10, no_persist, no_delivery),
            Err(GateError::WrongScope)
        );
        assert!(matches!(
            gate.admit(&ticket.scope, [2; 32], request(), 10, 100),
            Err(GateError::WrongScope)
        ));
    }
    let other_cli = CliScope::new(UUID, "/other").unwrap();
    let mut other_input = envelope("ok");
    other_input["cwd"] = json!("/other");
    let other_request =
        PermissionRequest::parse(&serde_json::to_vec(&other_input).unwrap(), &other_cli).unwrap();
    assert!(matches!(
        gate.admit(&scope(), [2; 32], other_request, 10, 100),
        Err(GateError::WrongScope)
    ));
    let mut ticket = review.ticket.clone();
    ticket.nonce = [2; 32];
    assert_eq!(
        gate.decide(&ticket, Decision::AllowOnce, 10, no_persist, no_delivery),
        Err(GateError::NotPending)
    );
    ticket = review.ticket.clone();
    ticket.fingerprint = parse(envelope("changed arguments")).unwrap().fingerprint();
    assert_eq!(
        gate.decide(&ticket, Decision::AllowOnce, 10, no_persist, no_delivery),
        Err(GateError::FingerprintChanged)
    );
    gate.decide(&review.ticket, Decision::AllowOnce, 11, ok, ok)
        .unwrap();
}

#[test]
fn expiry_is_exclusive_and_clock_regression_permanently_fences() {
    let mut g = gate(GateLimits::default());
    let review = admit(&mut g, 1);
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            100,
            no_persist,
            no_delivery
        ),
        Err(GateError::Expired)
    );
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            99,
            no_persist,
            no_delivery
        ),
        Err(GateError::ClockRegressed)
    );
    assert!(g.is_fenced());
    assert!(matches!(
        g.admit(&scope(), [2; 32], request(), 101, 200),
        Err(GateError::Fenced)
    ));
    let mut g = gate(GateLimits::default());
    let review = g
        .admit(&scope(), [1; 32], request(), u64::MAX - 1, u64::MAX)
        .unwrap();
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            u64::MAX,
            no_persist,
            no_delivery
        ),
        Err(GateError::Expired)
    );
}

#[test]
fn revoke_fences_existing_and_future_allows_without_claiming_completion() {
    let mut g = gate(GateLimits::default());
    let review = admit(&mut g, 1);
    g.fence();
    g.fence();
    assert!(g.is_fenced());
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            10,
            no_persist,
            no_delivery
        ),
        Err(GateError::Fenced)
    );
    assert!(matches!(
        g.admit(&scope(), [2; 32], request(), 10, 100),
        Err(GateError::Fenced)
    ));
}

#[test]
fn persistence_failure_never_delivers_and_fences_all_pending() {
    for decision in [Decision::AllowOnce, Decision::Deny] {
        let mut g = gate(GateLimits::default());
        let review = admit(&mut g, 1);
        let other = admit(&mut g, 2);
        assert_eq!(
            g.decide(
                &review.ticket,
                decision,
                10,
                |_| Err("private storage error"),
                no_delivery
            ),
            Err(GateError::Persistence)
        );
        for ticket in [&review.ticket, &other.ticket] {
            assert_eq!(
                g.decide(ticket, Decision::AllowOnce, 10, no_persist, no_delivery),
                Err(GateError::Fenced)
            );
        }
    }
}

#[test]
fn delivery_failure_is_consumed_and_restart_cannot_reissue() {
    let reserved = RefCell::new(false);
    let reserve = |_: &RunScope| {
        if reserved.replace(true) {
            Err(())
        } else {
            Ok(())
        }
    };
    let mut g = PermissionGate::new(scope(), GateLimits::default(), 10, reserve).unwrap();
    let review = admit(&mut g, 1);
    let consumed = RefCell::new(false);
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            10,
            |_| {
                *consumed.borrow_mut() = true;
                Ok::<_, ()>(())
            },
            |_| {
                assert!(*consumed.borrow());
                Err("partial delivery")
            }
        ),
        Err(GateError::Delivery)
    );
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            10,
            no_persist,
            no_delivery
        ),
        Err(GateError::Fenced)
    );
    drop(g);
    assert!(matches!(
        PermissionGate::new(scope(), GateLimits::default(), 11, reserve),
        Err(GateError::Persistence)
    ));
}

#[test]
fn denial_is_durable_terminal_and_has_only_hook_denial_fields() {
    let mut g = gate(GateLimits::default());
    let review = admit(&mut g, 1);
    let persisted = RefCell::new(false);
    g.decide(&review.ticket, Decision::Deny, 10, |record| {
        assert_eq!(record.decision, Decision::Deny);
        *persisted.borrow_mut() = true;
        Ok::<_, ()>(())
    }, |bytes| {
        assert!(*persisted.borrow());
        assert_eq!(serde_json::from_slice::<Value>(bytes).unwrap(), json!({"hookSpecificOutput": {
            "hookEventName":"PermissionRequest", "decision": {"behavior":"deny", "message":"Permission denied.", "interrupt":true}
        }}));
        Ok::<_, ()>(())
    }).unwrap();
    assert_eq!(
        g.decide(
            &review.ticket,
            Decision::AllowOnce,
            10,
            no_persist,
            no_delivery
        ),
        Err(GateError::NotPending)
    );
}

#[test]
fn bounded_pending_total_and_no_terminal_eviction() {
    let mut g = gate(GateLimits {
        pending: 1,
        total: 2,
    });
    let review = admit(&mut g, 1);
    assert!(matches!(
        g.admit(&scope(), [2; 32], request(), 10, 100),
        Err(GateError::Capacity)
    ));
    g.decide(&review.ticket, Decision::Deny, 10, ok, ok)
        .unwrap();
    let second = admit(&mut g, 2);
    assert!(matches!(
        g.admit(&scope(), [1; 32], request(), 10, 100),
        Err(GateError::Duplicate)
    ));
    assert_eq!(
        g.decide(
            &second.ticket,
            Decision::AllowOnce,
            100,
            no_persist,
            no_delivery
        ),
        Err(GateError::Expired)
    );
    assert!(matches!(
        g.admit(&scope(), [3; 32], request(), 100, 200),
        Err(GateError::Capacity)
    ));
    for limits in [
        GateLimits {
            pending: 0,
            total: 1,
        },
        GateLimits {
            pending: 33,
            total: 128,
        },
        GateLimits {
            pending: 2,
            total: 1,
        },
        GateLimits {
            pending: 1,
            total: 129,
        },
    ] {
        assert!(matches!(
            PermissionGate::new(scope(), limits, 10, |_| -> Result<(), ()> {
                panic!("invalid reservation")
            }),
            Err(GateError::Invalid)
        ));
    }
}

#[test]
fn pending_expiry_frees_pending_capacity_but_not_nonce_or_total() {
    let mut g = gate(GateLimits {
        pending: 1,
        total: 2,
    });
    admit(&mut g, 1);
    assert!(matches!(
        g.admit(&scope(), [1; 32], request(), 100, 200),
        Err(GateError::Duplicate)
    ));
    assert!(g.admit(&scope(), [2; 32], request(), 100, 200).is_ok());
    assert!(matches!(
        g.admit(&scope(), [3; 32], request(), 200, 300),
        Err(GateError::Capacity)
    ));
}

#[test]
fn invalid_admission_never_allocates_and_run_reservation_is_required() {
    let mut g = gate(GateLimits {
        pending: 1,
        total: 1,
    });
    assert!(matches!(
        g.admit(&scope(), [0; 32], request(), 10, 100),
        Err(GateError::Invalid)
    ));
    assert!(matches!(
        g.admit(&scope(), [1; 32], request(), 10, 10),
        Err(GateError::Expired)
    ));
    assert!(matches!(
        g.admit(&scope(), [1; 32], request(), 10, 9),
        Err(GateError::Expired)
    ));
    admit(&mut g, 1);
    for bad_id in [
        String::new(),
        "x".repeat(129),
        "untrusted\\nidentifier".into(),
    ] {
        let mut s = scope();
        s.run_id = bad_id;
        assert!(matches!(
            PermissionGate::new(s, GateLimits::default(), 10, |_| -> Result<(), ()> {
                panic!("invalid scope reached reservation")
            }),
            Err(GateError::Invalid)
        ));
    }
    let reserved = RefCell::new(false);
    let reserve = |_: &RunScope| {
        if reserved.replace(true) {
            Err(())
        } else {
            Ok(())
        }
    };
    drop(PermissionGate::new(scope(), GateLimits::default(), 10, reserve).unwrap());
    // A restart before any decision still cannot reopen the run, including with
    // a new owner/epoch. The durable host callback owns that uniqueness check.
    let mut restarted = scope();
    restarted.epoch += 1;
    restarted.owner += 1;
    assert!(matches!(
        PermissionGate::new(restarted, GateLimits::default(), 10, reserve),
        Err(GateError::Persistence)
    ));
}

#[test]
fn panicking_callbacks_leave_gate_fenced_and_cannot_reissue() {
    for panic_in_persist in [true, false] {
        let mut g = gate(GateLimits::default());
        let review = admit(&mut g, 1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = g.decide(
                &review.ticket,
                Decision::AllowOnce,
                10,
                |_| -> Result<(), ()> {
                    assert!(!panic_in_persist, "persistence interrupted");
                    Ok(())
                },
                |_| -> Result<(), ()> { panic!("delivery interrupted") },
            );
        }));
        assert!(result.is_err());
        assert!(g.is_fenced());
        assert_eq!(
            g.decide(
                &review.ticket,
                Decision::AllowOnce,
                10,
                no_persist,
                no_delivery
            ),
            Err(GateError::Fenced)
        );
    }
}

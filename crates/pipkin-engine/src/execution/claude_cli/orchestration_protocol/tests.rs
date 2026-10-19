use super::*;

fn scope() -> Scope {
    Scope {
        session: "root".into(),
        generation: 2,
        operation: "op".into(),
        run: 7,
    }
}
fn tool() -> ToolSpec {
    ToolSpec {
        name: "plan_update".into(),
        description: "Update the native plan".into(),
        parameters: json!({"type":"object","properties":{"ops":{"type":"array"}},"required":["ops"]}),
        constrained_sampling: None,
    }
}
fn send(s: &mut Session, value: Value) -> Incoming {
    s.receive(&serde_json::to_vec(&value).unwrap())
}
fn initialize(s: &mut Session) {
    let reply = send(
        s,
        json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{
        "protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
    );
    let Incoming::Response(reply) = reply else {
        panic!("initialize")
    };
    assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(
        reply["result"]["capabilities"],
        json!({"tools":{"listChanged":false}})
    );
    assert_eq!(
        send(
            s,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        ),
        Incoming::Ignore
    );
}
#[test]
fn quiescence_requires_initialization_and_all_calls_settled() {
    let mut s = Session::new(scope(), &[tool()]).unwrap();
    assert!(!s.is_quiescent());
    initialize(&mut s);
    assert!(s.is_quiescent());
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"plan_update","arguments":{"ops":[]}}});
    let Incoming::Call { key, .. } = send(&mut s, request.clone()) else {
        panic!()
    };
    assert!(!s.is_quiescent());
    s.complete(&key, json!({"content":[]})).unwrap();
    assert!(s.is_quiescent());
    let mut conflicting = request;
    conflicting["params"]["arguments"] = json!({"ops":[{}]});
    assert!(matches!(send(&mut s, conflicting), Incoming::Fence(_)));
    assert!(!s.is_quiescent());
}
#[test]
fn catalog_hash_is_canonical_scope_independent_and_schema_sensitive() {
    let t = tool();
    let original = Session::new(scope(), std::slice::from_ref(&t))
        .unwrap()
        .catalog_hash();
    let mut other = scope();
    other.generation += 1;
    assert_eq!(
        Session::new(other, std::slice::from_ref(&t))
            .unwrap()
            .catalog_hash(),
        original
    );
    let mut changed = t.clone();
    changed.parameters["additionalProperties"] = json!(false);
    assert_ne!(
        Session::new(scope(), &[changed]).unwrap().catalog_hash(),
        original
    );
    let mut changed = t.clone();
    changed.description.push('x');
    assert_ne!(
        Session::new(scope(), &[changed]).unwrap().catalog_hash(),
        original
    );
    assert_eq!(
        Session::new(scope(), &[t]).unwrap().catalog_hash(),
        original
    );
}

fn ready() -> Session {
    let mut s = Session::new(scope(), &[tool()]).unwrap();
    initialize(&mut s);
    s
}
fn call(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"plan_update","arguments":{"ops":[]}}})
}
fn accepted(s: &mut Session, id: Value) -> CallKey {
    match send(s, call(id)) {
        Incoming::Call {
            key,
            name,
            arguments,
        } => {
            assert_eq!(name, "plan_update");
            assert_eq!(arguments, json!({"ops":[]}));
            key
        }
        other => panic!("expected one intent, got {other:?}"),
    }
}
fn result() -> Value {
    json!({"content":[{"type":"text","text":"committed plan receipt"}],"isError":false})
}
fn assert_error(action: Incoming, code: i64) {
    let Incoming::Response(value) = action else {
        panic!("expected protocol error, got {action:?}")
    };
    assert_eq!(value["error"]["code"], code);
}

#[test]
fn real_native_builtin_schemas_are_exported_without_new_cli_or_api_authority() {
    let tools: Vec<_> = crate::tools::builtin_tools()
        .into_iter()
        .map(|t| t.spec())
        .collect();
    let mut s = Session::new(scope(), &tools).unwrap();
    initialize(&mut s);
    let Incoming::Response(value) = send(
        &mut s,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    ) else {
        panic!("catalog")
    };
    let listed = value["result"]["tools"].as_array().unwrap();
    assert_eq!(listed.len(), tools.len());
    for tool in &tools {
        let item = listed.iter().find(|t| t["name"] == tool.name).unwrap();
        assert_eq!(item["inputSchema"], tool.parameters);
    }
    assert_eq!(s.pending(), 0);
}

#[test]
fn calls_require_initialized_notification_and_exact_catalog_name() {
    let mut s = Session::new(scope(), &[tool()]).unwrap();
    assert_error(send(&mut s, call(json!(1))), -32002);
    initialize(&mut s);
    let mut unknown = call(json!(2));
    unknown["params"]["name"] = json!("mcp__other__plan_update");
    assert_error(send(&mut s, unknown), -32602);
    let mut malformed = call(json!(3));
    malformed["params"]["arguments"] = json!("print <invoke>");
    assert_error(send(&mut s, malformed), -32602);
    assert_eq!(s.pending(), 0);
    assert_eq!(accepted(&mut s, json!(4)).scope(), &scope());
}

#[test]
fn duplicate_call_is_pending_then_durable_receipt_lookup_never_another_intent() {
    let mut s = ready();
    let key = accepted(&mut s, json!(11));
    assert_eq!(
        send(&mut s, call(json!(11))),
        Incoming::Pending(key.clone())
    );
    assert_eq!(s.pending(), 1);
    let reply = s.complete(&key, result()).unwrap();
    assert_eq!(reply["id"], 11);
    assert_eq!(reply["result"], result());
    assert_eq!(s.pending(), 0);
    assert_eq!(send(&mut s, call(json!(11))), Incoming::Replay(key.clone()));
    assert_eq!(s.replay(&key, result()).unwrap(), reply);
    assert_eq!(
        s.complete(&key, result()),
        Err(CompletionError::AlreadyCompleted)
    );
}

#[test]
fn canonical_argument_order_and_typed_ids_remain_distinct() {
    let mut s = ready();
    let key = accepted(&mut s, json!(1));
    assert_eq!(s.receive(br#"{"method":"tools/call","params":{"arguments":{"ops":[]},"name":"plan_update"},"id":1,"jsonrpc":"2.0"}"#), Incoming::Pending(key));
    let string = accepted(&mut s, json!("1"));
    let negative = accepted(&mut s, json!(-1));
    assert_ne!(string, negative);
    assert_eq!(s.pending(), 3);
}

#[test]
fn conflicting_duplicate_fences_instead_of_reexecuting_or_overwriting() {
    for completed in [false, true] {
        let mut s = ready();
        let key = accepted(&mut s, json!(1));
        if completed {
            s.complete(&key, result()).unwrap();
        }
        let mut changed = call(json!(1));
        changed["params"]["arguments"]["ops"] = json!([{"op":"create","title":"different"}]);
        assert_eq!(send(&mut s, changed), Incoming::Fence(Fault::ConflictingId));
        assert!(s.is_closed());
        assert_eq!(s.complete(&key, result()), Err(CompletionError::Closed));
        assert_eq!(send(&mut s, call(json!(2))), Incoming::Fence(Fault::Closed));
    }
}

#[test]
fn metadata_request_ids_cannot_be_reused_to_gain_tool_dispatch() {
    let mut s = ready();
    let ping = json!({"jsonrpc":"2.0","id":12,"method":"ping"});
    let response = send(&mut s, ping.clone());
    assert_eq!(send(&mut s, ping), response);
    assert_eq!(
        send(&mut s, call(json!(12))),
        Incoming::Fence(Fault::ConflictingId)
    );
}

#[test]
fn completion_from_another_bridge_even_with_identical_scope_is_rejected() {
    let mut a = ready();
    let mut b = ready();
    let ka = accepted(&mut a, json!(1));
    let kb = accepted(&mut b, json!(1));
    assert_eq!(b.complete(&ka, result()), Err(CompletionError::ForeignCall));
    assert_eq!(b.pending(), 1);
    b.complete(&kb, result()).unwrap();
    a.close();
    assert_eq!(a.complete(&ka, result()), Err(CompletionError::Closed));
}

#[test]
fn cancellation_is_correlated_once_and_does_not_claim_effects_were_undone() {
    let mut s = ready();
    let key = accepted(&mut s, json!(1));
    let cancel = json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1,"reason":"caller cancelled"}});
    assert_eq!(send(&mut s, cancel.clone()), Incoming::Cancel(key.clone()));
    assert_eq!(send(&mut s, cancel.clone()), Incoming::Ignore);
    assert_eq!(s.pending(), 1);
    assert_eq!(send(&mut s, call(json!(1))), Incoming::Pending(key.clone()));
    s.complete(&key, result()).unwrap();
    assert_eq!(send(&mut s, cancel), Incoming::Ignore);
}

#[test]
fn malformed_markup_cli_observations_batches_and_notifications_cannot_dispatch() {
    let mut s = ready();
    for bytes in [
        b"<invoke name=\"plan_update\">".as_slice(),
        br#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"plan_update"}]}}"#,
        b"[]",
        b"null",
    ] {
        assert!(matches!(s.receive(bytes), Incoming::Response(_)));
    }
    for id in [Value::Null, json!(1.5), json!({}), json!("x".repeat(129))] {
        assert_error(send(&mut s, call(id)), -32600);
    }
    let mut notification = call(json!(1));
    notification.as_object_mut().unwrap().remove("id");
    assert_eq!(send(&mut s, notification), Incoming::Ignore);
    assert_eq!(s.pending(), 0);
}

#[test]
fn caller_supplied_scope_cannot_replace_the_engine_scope() {
    let mut s = ready();
    let mut request = call(json!(1));
    request["scope"] = json!({"session":"foreign","run":100,"generation":99});
    request["params"]["session"] = json!("foreign");
    let Incoming::Call { key, .. } = send(&mut s, request) else {
        panic!("intent")
    };
    assert_eq!(key.scope(), &scope());
}

#[test]
fn pressure_is_bounded_and_completed_requests_remain_nonexecutable() {
    let mut s = ready();
    let mut keys = Vec::new();
    for n in 0..MAX_PENDING {
        keys.push(accepted(&mut s, json!(n)));
    }
    assert_error(send(&mut s, call(json!(MAX_PENDING))), -32000);
    s.complete(&keys[0], result()).unwrap();
    accepted(&mut s, json!(MAX_PENDING + 1));
    assert_eq!(s.pending(), MAX_PENDING);
    // Backpressure refusal is stable for the same request ID, not implicit retry permission.
    assert_error(send(&mut s, call(json!(MAX_PENDING))), -32000);
    assert_eq!(
        send(&mut s, call(json!(0))),
        Incoming::Replay(keys[0].clone())
    );
}

#[test]
fn total_request_limit_and_frame_limit_close_without_eviction_or_replay() {
    let mut s = ready();
    for n in 0..(MAX_REQUESTS - 1) {
        assert!(matches!(
            send(&mut s, json!({"jsonrpc":"2.0","id":n,"method":"ping"})),
            Incoming::Response(_)
        ));
    }
    assert_eq!(
        send(&mut s, call(json!("new"))),
        Incoming::Fence(Fault::RequestLimit)
    );
    let mut s = ready();
    assert_eq!(
        s.receive(&vec![b' '; MAX_FRAME_BYTES + 1]),
        Incoming::Fence(Fault::FrameLimit)
    );
}

#[test]
fn lost_response_disconnect_and_oversized_results_never_reopen_admission() {
    let mut s = ready();
    let key = accepted(&mut s, json!(1));
    s.complete(&key, result()).unwrap();
    s.close(); // Driver calls this on reply-write failure, not just clean EOF.
    assert_eq!(send(&mut s, call(json!(1))), Incoming::Fence(Fault::Closed));
    assert_eq!(s.replay(&key, result()), Err(CompletionError::Closed));
    let mut s = ready();
    let key = accepted(&mut s, json!(1));
    assert_eq!(
        s.complete(&key, json!({"content":"x".repeat(MAX_RESULT_BYTES)})),
        Err(CompletionError::ResultLimit)
    );
    assert!(s.is_closed());
}

#[test]
fn diagnostics_do_not_dump_call_arguments_or_response_bodies() {
    let mut s = ready();
    let mut request = call(json!(1));
    request["params"]["arguments"]["private"] = json!("synthetic-private-marker");
    let action = send(&mut s, request);
    assert!(!format!("{action:?}").contains("synthetic-private-marker"));
    let response = Incoming::Response(json!({"content":"synthetic-private-marker"}));
    assert!(!format!("{response:?}").contains("synthetic-private-marker"));
}

#[test]
fn inconsistent_receipt_replay_fences_without_reexecution() {
    let mut s = ready();
    let key = accepted(&mut s, json!("original"));
    assert_eq!(key.request_id(), json!("original"));
    s.complete(&key, result()).unwrap();
    assert_eq!(
        s.replay(&key, json!({"content":[],"isError":true})),
        Err(CompletionError::ConflictingResult)
    );
    assert!(s.is_closed());
    assert_eq!(
        send(&mut s, call(json!("next"))),
        Incoming::Fence(Fault::Closed)
    );
}

#[test]
fn malformed_initialization_never_enables_tool_intents() {
    let mut s = Session::new(scope(), &[tool()]).unwrap();
    assert_error(
        send(
            &mut s,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{}}}),
        ),
        -32602,
    );
    assert_eq!(
        send(
            &mut s,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        ),
        Incoming::Ignore
    );
    assert_error(send(&mut s, call(json!(2))), -32002);
    assert_eq!(s.pending(), 0);
}

#[test]
fn invalid_catalogs_and_scope_are_rejected_before_initialization() {
    let mut invalid = scope();
    invalid.run = 0;
    assert!(matches!(
        Session::new(invalid, &[tool()]),
        Err(SetupError::Scope)
    ));
    assert!(matches!(
        Session::new(scope(), &[tool(), tool()]),
        Err(SetupError::Catalog)
    ));
    let mut t = tool();
    t.parameters = json!({"type":"string"});
    assert!(matches!(
        Session::new(scope(), &[t]),
        Err(SetupError::Catalog)
    ));
    let mut t = tool();
    t.constrained_sampling = Some(json!({"type":"grammar"}));
    assert!(matches!(
        Session::new(scope(), &[t]),
        Err(SetupError::Catalog)
    ));
    let mut t = tool();
    t.description = "x".repeat(MAX_CATALOG_BYTES + 1);
    assert!(matches!(
        Session::new(scope(), &[t]),
        Err(SetupError::Catalog)
    ));
}

#[test]
fn parity_maximum_receipt_body_leaves_room_for_the_rpc_envelope() {
    let mut s = ready();
    let key = accepted(&mut s, json!("x".repeat(128)));
    let overhead = serde_json::to_vec(&json!({"content":""})).unwrap().len();
    let result = json!({"content":"x".repeat(MAX_RESULT_BODY_BYTES-overhead)});
    assert_eq!(
        serde_json::to_vec(&result).unwrap().len(),
        MAX_RESULT_BODY_BYTES
    );
    let reply = s.complete(&key, result).unwrap();
    assert!(serde_json::to_vec(&reply).unwrap().len() <= MAX_RESULT_BYTES);
    assert!(s.is_quiescent());
}

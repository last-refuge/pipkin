use super::*;

fn fixture(agent: &Agent) -> Value {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/native-fork-import");
    let dest = agent.dir("forks");
    std::fs::create_dir_all(&dest).unwrap();
    for name in ["session.sqlite", "meta.json"] {
        std::fs::copy(source.join(name), dest.join(name)).unwrap();
    }
    serde_json::from_slice(&std::fs::read(source.join("reference.json")).unwrap()).unwrap()
}

#[test]
fn fork_import_matches_executed_pi_sqlite_context_and_keeps_as_of_documents() {
    let agent = Agent::new();
    let reference = fixture(&agent);
    assert_eq!(
        sha256_file(&agent.dir("forks").join("session.sqlite")).unwrap(),
        reference["sqliteSha256"]
    );
    let before = snapshot(&agent.dir("forks"));
    let store = Store::memory().unwrap();
    let outcome = import(&agent, &store, "forks").unwrap();
    assert_eq!(outcome.children.len(), 6);
    assert_eq!(outcome.manifest["counts"]["forks"], 6);
    let counted: u64 = outcome.manifest["counts"]["entries"]
        .as_object()
        .unwrap()
        .values()
        .map(|n| n.as_u64().unwrap())
        .sum();
    assert_eq!(
        counted,
        reference["entries"].as_array().unwrap().len() as u64
    );
    assert!(
        outcome.manifest["counts"]["materializedEntries"]
            .as_u64()
            .unwrap()
            > counted
    );
    for expected in reference["expected"].as_array().unwrap() {
        let id = expected["id"].as_i64().unwrap();
        let native = if id == 1 {
            "forks".into()
        } else {
            child_session_id("forks", id)
        };
        let session = store.session(&native).unwrap().unwrap();
        assert!(
            session.parent.is_none() && session.root.is_none(),
            "forks are independent roots"
        );
        let history = store.page(&native, None, 1000).unwrap().0;
        assert_eq!(
            json!(history.iter().map(|e| e.id).collect::<Vec<_>>()),
            expected["visibleIds"],
            "{native}"
        );
        let expected: Vec<Value> = expected["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| serde_json::to_value(context::normalize_message(m).unwrap().0).unwrap())
            .collect();
        assert_eq!(native_context(&store, &native), expected, "{native}");
        let provenance: Value =
            serde_json::from_str(&store.kv_get(&native, "pi.conversation").unwrap().unwrap())
                .unwrap();
        assert_eq!(provenance["id"], id);
        assert_eq!(
            crate::extensions::profiles::resolve(&store, &native).unwrap(),
            (vec![], true)
        );
    }
    let root = store.session("forks").unwrap().unwrap();
    let branch = store.session("forks:c10").unwrap().unwrap();
    assert_eq!(root.model.as_deref(), Some("openai/after"));
    assert_eq!(branch.model.as_deref(), Some("openai/before"));
    assert_eq!(branch.thinking.as_deref(), Some("low"));
    assert_eq!(snapshot(&agent.dir("forks")), before);
    let rows = dump(&store);
    assert!(import(&agent, &store, "forks").unwrap().already_imported);
    assert_eq!(dump(&store), rows);
}

#[test]
fn fork_import_is_atomic_on_failure_and_retry() {
    let agent = Agent::new();
    fixture(&agent);
    let store = Store::memory().unwrap();
    store.fail_next_write();
    assert!(matches!(
        import(&agent, &store, "forks"),
        Err(ImportError::Store(StoreError::Injected))
    ));
    assert!(dump(&store).is_empty());
    assert!(store.imported(ENGINE, "forks").unwrap().is_none());
    assert_eq!(import(&agent, &store, "forks").unwrap().children.len(), 6);
}

#[test]
fn malformed_fork_ancestry_is_rejected_without_partial_import() {
    let cases = [
        (json!({"conversationId": 99,"at":2}), "unknown conversation"),
        (json!({"conversationId": 3,"at":2}), "cyclic"),
        (json!({"conversationId": 1,"at":99}), "missing entry"),
        (json!({"conversationId": 1}), "malformed"),
        (json!({"conversationId": 1,"at":-1}), "invalid"),
    ];
    for (parent, reason) in cases {
        let agent = Agent::new();
        let mut l = agent.session("bad-fork");
        l.conversation(json!({"id":1}));
        l.entry(user(2, 1, json!("original"), 2));
        l.conversation(json!({"id":3,"parent":parent}));
        drop(l);
        assert!(
            blocked(&agent, "bad-fork")
                .iter()
                .any(|r| r.contains(reason)),
            "{reason}"
        );
    }
    let agent = Agent::new();
    let mut l = agent.session("bad-fork");
    l.conversation(json!({"id":1}));
    l.entry(user(2, 1, json!("original"), 2));
    l.conversation(json!({"id":3,"parent":{"conversationId":1,"at":2}}));
    l.entry(user(4, 1, json!("later parent"), 4));
    l.conversation(json!({"id":5,"parent":{"conversationId":3,"at":4}}));
    drop(l);
    assert!(
        blocked(&agent, "bad-fork")
            .iter()
            .any(|r| r.contains("not visible"))
    );
}

#[test]
fn fork_ancestry_does_not_change_task_ownership_or_duplicate_usage() {
    let agent = Agent::new();
    let mut l = rich_session(&agent);
    // Existing subagent inherits root history but is still owned by its original tool task.
    l.conn
        .execute(
            "UPDATE conversations SET record=json_set(record,'$.parent',json(?1)) WHERE id=29",
            [json!({"conversationId":1,"at":25}).to_string()],
        )
        .unwrap();
    // An ownerless fork of that subagent is a new root, not part of its parent's usage tree.
    l.conversation(json!({"id":70,"parent":{"conversationId":29,"at":32}}));
    l.doc(
        71,
        "pi.usage",
        70,
        json!({"models":{"anthropic/claude-test":usage(2,2,0.03)},"tools":{}}),
        &[],
    );
    // A tool owned by the independent fork creates its own child, whose history can
    // independently fork from a different conversation in the same source store.
    l.task(terminal_task(
        79,
        70,
        "pi.generation",
        json!({}),
        json!({"status":"completed","result":null}),
        None,
    ));
    l.entry(assistant(80,70,json!([{"type":"toolCall","id":"fork-child","name":"subagent","arguments":{"task":"fork work"}}]),"toolUse",80,79));
    l.task(terminal_task(
        81,
        70,
        "pi.tool",
        json!({"assistant":80,"callId":"fork-child"}),
        json!({"status":"completed","result":null}),
        None,
    ));
    l.conversation(json!({"id":82,"parent":{"conversationId":29,"at":32},"owner":{"conversationId":70,"taskId":81}}));
    l.entry(user(83, 82, json!("fork work"), 83));
    l.doc(
        84,
        "pi.usage",
        82,
        json!({"models":{"anthropic/claude-test":usage(5,5,0.01)},"tools":{}}),
        &[],
    );
    drop(l);
    let store = Store::memory().unwrap();
    import(&agent, &store, ROOT).unwrap();
    let child = store.session(&child_session_id(ROOT, 29)).unwrap().unwrap();
    assert_eq!(child.parent.as_deref(), Some(ROOT));
    assert_eq!(child.root.as_deref(), Some(ROOT));
    let fork_id = child_session_id(ROOT, 70);
    let fork = store.session(&fork_id).unwrap().unwrap();
    assert!(fork.parent.is_none() && fork.root.is_none());
    assert_eq!(store.tasks(ROOT, 100).unwrap().0.len(), 1);
    let tasks = store.tasks(&fork_id, 100).unwrap().0;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].root, fork_id);
    let fork_child = store.session(&child_session_id(ROOT, 82)).unwrap().unwrap();
    assert_eq!(fork_child.parent.as_deref(), Some(fork_id.as_str()));
    assert_eq!(fork_child.root.as_deref(), Some(fork_id.as_str()));
    assert_eq!(store.usage(&fork_id).unwrap().models[0].1.input, 7);
    assert_eq!(store.usage(ROOT).unwrap().models[0].1.input, 34);
}

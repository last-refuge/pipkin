use super::*;

fn fixture(agent: &Agent, kind: &str, blocked: bool) -> Value {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/native-orphan-import")
        .join(kind);
    let dest = agent.dir(kind);
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::copy(source.join("meta.json"), dest.join("meta.json")).unwrap();
    std::fs::copy(
        source.join(if blocked {
            "blocked.sqlite"
        } else {
            "session.sqlite"
        }),
        dest.join("session.sqlite"),
    )
    .unwrap();
    let reference: Value =
        serde_json::from_slice(&std::fs::read(source.join("reference.json")).unwrap()).unwrap();
    assert_eq!(
        sha256_file(&dest.join("session.sqlite")).unwrap(),
        reference[if blocked {
            "blockedSha256"
        } else {
            "sqliteSha256"
        }]
    );
    reference
}

#[test]
fn newer_builtin_pi_tasks_refuse_import_until_explicitly_settled() {
    for kind in ["generation", "compaction", "tool"] {
        let agent = Agent::new();
        let reference = fixture(&agent, kind, true);
        let before = snapshot(&agent.dir(kind));
        let reasons = blocked(&agent, kind);
        assert!(
            reasons.iter().any(|r| r.starts_with(IN_FLIGHT)),
            "{kind}: {reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&format!("task {}", reference["taskId"])))
        );
        assert_eq!(snapshot(&agent.dir(kind)), before);
    }
}

#[test]
fn orphaned_builtin_import_preserves_executed_pi_context_outcomes_and_partials() {
    for kind in ["generation", "compaction", "tool", "faulted"] {
        let agent = Agent::new();
        let reference = fixture(&agent, kind, false);
        let before = snapshot(&agent.dir(kind));
        let store = Store::memory().unwrap();
        import(&agent, &store, kind).unwrap();
        let expected: Vec<Value> = reference["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| serde_json::to_value(context::normalize_message(m).unwrap().0).unwrap())
            .collect();
        assert_eq!(native_context(&store, kind), expected, "{kind}");
        let tasks: Value =
            serde_json::from_str(&store.kv_get(kind, "pi.tasks").unwrap().unwrap()).unwrap();
        assert_eq!(tasks, json!([reference["settled"]]));
        let live: Value =
            serde_json::from_str(&store.kv_get(kind, "pi.doc.pi.live").unwrap().unwrap()).unwrap();
        assert_eq!(live["value"], reference["live"]);
        let history = store.page(kind, None, 100).unwrap().0;
        assert_eq!(
            history.len(),
            reference["entries"].as_array().unwrap().len()
        );
        if kind == "generation" || kind == "faulted" {
            let request = store
                .request(kind, &format!("recover-{kind}"))
                .unwrap()
                .unwrap();
            assert_eq!(request.state, RequestState::Settled);
            let detail = if kind == "generation" {
                "task_too_old".into()
            } else {
                format!(
                    "faulted: {}",
                    reference["submission"]["detail"].as_str().unwrap()
                )
            };
            assert_eq!(request.outcome, Some(RunOutcome::Failed { detail }));
            assert!(
                serde_json::to_string(&history)
                    .unwrap()
                    .contains("retained partial")
            );
            let submissions: Value =
                serde_json::from_str(&store.kv_get(kind, "pi.submissions").unwrap().unwrap())
                    .unwrap();
            assert_eq!(submissions, json!([reference["submission"]]));
        }
        assert_eq!(snapshot(&agent.dir(kind)), before);
        let rows = dump(&store);
        assert!(import(&agent, &store, kind).unwrap().already_imported);
        assert_eq!(dump(&store), rows);
    }
}

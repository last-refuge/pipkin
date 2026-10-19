use super::*;
pub(crate) fn create(session: &str, cwd: &str) -> Create {
    Create {
        session: session.into(),
        request_key: format!("create-{session}"),
        model: "sonnet".into(),
        thinking: None,
        parent: None,
        identity: CliBindingSpec {
            cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
            project_path: cwd.into(),
            executable_path: "/fixture/claude".into(),
            cli_version: "2.1.301".into(),
            config_ref: "config".into(),
            account_scope: "account".into(),
            launch_policy: cli_bridge::ORCHESTRATION_POLICY.into(),
        },
    }
}
fn legacy(mut spec: CliBindingSpec) -> CliBindingSpec {
    spec.launch_policy = "cli-unattended-coding-v1".into();
    spec
}
fn counts(s: &Store) -> (i64, i64, i64) {
    s.read(|c|Ok(c.query_row("SELECT (SELECT COUNT(*) FROM sessions),(SELECT COUNT(*) FROM cli_agent_bindings),(SELECT COUNT(*) FROM cli_uuid_owners)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)).unwrap()
}
#[test]
fn exact_creation_receipt_is_not_launch_authority_and_native_api_stays_closed() {
    let s = Store::memory().unwrap();
    let spec = create("agent", "/project");
    let (binding, fresh) = s.create_cli_agent_session(&spec, 1).unwrap();
    assert!(fresh);
    assert_eq!(
        s.create_cli_agent_session(&spec, 2).unwrap(),
        (binding.clone(), false)
    );
    assert_eq!(s.cli_agent_binding("agent").unwrap(), Some(binding));
    assert_eq!(counts(&s), (1, 1, 1));
    assert_eq!(
        s.session("agent").unwrap().unwrap().execution_backend,
        ExecutionBackend::ClaudeCliAgent
    );
    assert!(
        s.read(|c| Store::assert_native_execution(c, "agent"))
            .is_err()
    );
    assert!(s.cli_binding("agent").unwrap().is_none());
    assert!(s.cli_creation("agent", "create-agent").is_err());
}
#[test]
fn failed_commit_rolls_back_identity_uuid_and_outbox() {
    let s = Store::memory().unwrap();
    let spec = create("agent", "/project");
    s.fail_next_write();
    assert!(s.create_cli_agent_session(&spec, 1).is_err());
    assert_eq!(counts(&s), (0, 0, 0));
    assert_eq!(
        s.read(
            |c| Ok(c.query_row("SELECT COUNT(*) FROM event_outbox", [], |r| r
                .get::<_, i64>(0))?)
        )
        .unwrap(),
        0
    );
    assert!(s.create_cli_agent_session(&spec, 2).unwrap().1);
}
#[test]
fn all_identity_changes_conflict_instead_of_rebinding() {
    for field in 0..10 {
        let s = Store::memory().unwrap();
        let spec = create("agent", "/project");
        let original = s.create_cli_agent_session(&spec, 1).unwrap().0;
        let mut changed = spec;
        match field {
            0 => changed.model = "opus".into(),
            1 => changed.thinking = Some("high".into()),
            2 => changed.request_key.push('x'),
            3 => changed.identity.cli_session_id = "22222222-2222-4222-8222-222222222222".into(),
            4 => changed.identity.project_path = "/other".into(),
            5 => changed.identity.executable_path = "/other/cli".into(),
            6 => changed.identity.cli_version = "2.1.302".into(),
            7 => changed.identity.config_ref.push('x'),
            8 => changed.identity.account_scope.push('x'),
            _ => changed.parent = Some("foreign".into()),
        }
        assert!(s.create_cli_agent_session(&changed, 2).is_err());
        assert_eq!(s.cli_agent_binding("agent").unwrap(), Some(original));
    }
}
#[test]
fn invalid_alias_policy_version_paths_and_uuid_fail_before_creation() {
    for field in 0..6 {
        let s = Store::memory().unwrap();
        let mut spec = create("agent", "/project");
        match field {
            0 => spec.model = "openai/gpt".into(),
            1 => spec.identity.launch_policy = "cli-unattended-coding-v1".into(),
            2 => spec.identity.cli_version = "2.1.298".into(),
            3 => spec.identity.project_path = "/project/../other".into(),
            4 => spec.identity.cli_session_id = "not-uuid".into(),
            _ => spec.identity.cli_version = "2.1.301-beta".into(),
        }
        assert!(s.create_cli_agent_session(&spec, 1).is_err());
        assert_eq!(counts(&s), (0, 0, 0));
    }
}
#[test]
fn uuid_namespace_is_global_in_both_directions_even_after_removal() {
    for first_agent in [true, false] {
        let s = Store::memory().unwrap();
        let spec = create("first", "/project");
        let old = legacy(spec.identity.clone());
        if first_agent {
            s.create_cli_agent_session(&spec, 1).unwrap();
        } else {
            s.create_cli_session_with_model("first", "old-create", &old, "sonnet", 1)
                .unwrap();
        }
        s.begin_removal("first").unwrap();
        s.finish_removal("first").unwrap();
        let mut next = create("second", "/project");
        next.identity.cli_session_id = old.cli_session_id.clone();
        assert!(s.create_cli_agent_session(&next, 2).is_err());
        assert!(
            s.create_cli_session_with_model("second", "new-create", &old, "sonnet", 2)
                .is_err()
        );
        assert_eq!(counts(&s), (0, 0, 1));
        assert!(
            s.tx(|tx| {
                tx.execute("DELETE FROM cli_uuid_owners", [])?;
                Ok(())
            })
            .is_err()
        );
    }
}
#[test]
fn sql_cannot_change_bound_model_project_parent_or_binding() {
    let s = Store::memory().unwrap();
    s.create_cli_agent_session(&create("agent", "/project"), 1)
        .unwrap();
    for sql in [
        "UPDATE sessions SET model='opus' WHERE id='agent'",
        "UPDATE sessions SET cwd='/other' WHERE id='agent'",
        "UPDATE sessions SET parent='foreign' WHERE id='agent'",
        "UPDATE cli_agent_bindings SET executable_path='/other'",
        "UPDATE cli_uuid_owners SET backend='localClaudeCli'",
    ] {
        assert!(
            s.tx(|tx| {
                tx.execute_batch(sql)?;
                Ok(())
            })
            .is_err(),
            "{sql}"
        );
    }
}
#[test]
fn children_inherit_cli_identity_and_may_only_select_cli_profiles() {
    let s = Store::memory().unwrap();
    let root = create("root", "/project");
    s.create_cli_agent_session(&root, 1).unwrap();
    let mut child = create("child", "/project");
    child.parent = Some("root".into());
    child.model = "haiku".into();
    child.identity.cli_session_id = "22222222-2222-4222-8222-222222222222".into();
    let b = s.create_cli_agent_session(&child, 2).unwrap().0;
    assert_eq!(b.root.as_deref(), Some("root"));
    let mut grandchild = child.clone();
    grandchild.session = "grandchild".into();
    grandchild.request_key = "grandchild-key".into();
    grandchild.parent = Some("child".into());
    grandchild.identity.cli_session_id = "33333333-3333-4333-8333-333333333333".into();
    assert_eq!(
        s.create_cli_agent_session(&grandchild, 3)
            .unwrap()
            .0
            .root
            .as_deref(),
        Some("root")
    );
    s.begin_removal("root").unwrap();
    s.finish_removal("root").unwrap();
    assert_eq!(counts(&s), (0, 0, 3));
    assert!(s.create_cli_agent_session(&child, 4).is_err());
    let mut reuse = create("different-session", "/project");
    reuse.request_key = child.request_key.clone();
    reuse.identity.cli_session_id = "44444444-4444-4444-8444-444444444444".into();
    assert!(s.create_cli_agent_session(&reuse, 5).is_err());
    assert!(
        s.create_session(
            "native-other",
            "/project",
            Some(&child.request_key),
            &Origin::default(),
            None,
            None,
            None,
            5
        )
        .is_err()
    );
}
#[test]
fn foreign_parent_account_project_disabled_or_removing_parent_cannot_create_child() {
    for mode in 0..7 {
        let s = Store::memory().unwrap();
        s.create_cli_agent_session(&create("root", "/project"), 1)
            .unwrap();
        let mut child = create("child", "/project");
        child.parent = Some("root".into());
        child.identity.cli_session_id = "22222222-2222-4222-8222-222222222222".into();
        match mode {
            0 => child.identity.account_scope = "other".into(),
            1 => child.identity.project_path = "/other".into(),
            2 => child.parent = Some("missing".into()),
            3 => {
                s.tx(|tx| {
                    tx.execute(
                        "UPDATE sessions SET subagents_enabled=0 WHERE id='root'",
                        [],
                    )?;
                    Ok(())
                })
                .unwrap();
            }
            4 => {
                s.begin_removal("root").unwrap();
            }
            5 => {
                s.create_session(
                    "native",
                    "/project",
                    Some("native-key"),
                    &Origin::default(),
                    None,
                    None,
                    None,
                    1,
                )
                .unwrap();
                child.parent = Some("native".into());
            }
            _ => {
                s.tx(|tx| {
                    tx.execute("UPDATE sessions SET blocked='held' WHERE id='root'", [])?;
                    Ok(())
                })
                .unwrap();
            }
        }
        assert!(s.create_cli_agent_session(&child, 2).is_err());
        assert_eq!(counts(&s), (if mode == 5 { 2 } else { 1 }, 1, 1));
    }
}
#[test]
fn concurrent_coding_and_orchestration_creation_reserves_exactly_one_owner() {
    let s = std::sync::Arc::new(Store::memory().unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
    let tasks = (0..16)
        .map(|i| {
            let s = s.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                let spec = create(&format!("s{i}"), "/project");
                b.wait();
                if i % 2 == 0 {
                    s.create_cli_agent_session(&spec, 1).is_ok()
                } else {
                    s.create_cli_session_with_model(
                        &spec.session,
                        &spec.request_key,
                        &legacy(spec.identity),
                        "sonnet",
                        1,
                    )
                    .is_ok()
                }
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|won| *won)
            .count(),
        1
    );
    let (sessions, _, owners) = counts(&s);
    assert_eq!((sessions, owners), (1, 1));
    s.integrity_check().unwrap();
}
#[test]
fn binding_debug_is_redacted() {
    let s = Store::memory().unwrap();
    let spec = create("private-session", "/private-project");
    let b = s.create_cli_agent_session(&spec, 1).unwrap().0;
    assert!(!format!("{b:?} {spec:?}").contains("private"));
}

mod migration;

use super::*;
mod persistence;
use crate::{
    config::Settings,
    context::prompt::PromptInput,
    execution::claude_cli::orchestration_child::{self, Error},
    provider::ToolSpec,
    tools::SubagentLaunch,
};
use serde_json::json;
struct Fixture {
    s: Store,
    b: cli_agent::Binding,
    root: cli_agent_turns::Lease,
    bridge: cli_bridge::Lease,
    settings: Settings,
}
impl Fixture {
    fn new() -> Self {
        let s = Store::memory().unwrap();
        let b = s
            .create_cli_agent_session(&cli_agent::tests::create("parent", "/project"), 1)
            .unwrap()
            .0;
        let root = s
            .admit_cli_agent_turn(
                &cli_agent_turns::Input {
                    images: &[],
                    mode: pipkin_engine_protocol::InputMode::Prompt,
                    session: "parent",
                    request: "request",
                    generation: 1,
                    operation: "op",
                    binding: b.digest,
                    text: "delegate",
                },
                "owner",
                2,
            )
            .unwrap()
            .lease
            .unwrap();
        let spec = cli_bridge::BridgeSpec {
            id: "bridge".into(),
            scope: root.receipt().scope.clone(),
            owner: "owner".into(),
            binding_hash: [3; 32],
            capability_hash: [4; 32],
        };
        s.prepare_cli_tool_bridge(&spec, 3).unwrap();
        s.begin_cli_agent_launch(&root, "bridge", 4).unwrap();
        s.mark_cli_agent_running(&root, 5).unwrap();
        let bridge = s.claim_cli_tool_bridge(&spec, 6).unwrap().unwrap();
        Self {
            s,
            b,
            root,
            bridge,
            settings: Settings::default(),
        }
    }
    fn policy(
        &self,
        call: &str,
        mode: crate::profiles::ContextMode,
    ) -> std::result::Result<Frozen, Error> {
        let launch = SubagentLaunch {
            child: None,
            task: "child task".into(),
            profile: None,
            context: Some(mode),
            wait: true,
            wake: false,
            model: Some("claude-cli/haiku".into()),
        };
        let tools = ["read", "write", "subagent", "ask_user_question"].map(|name| ToolSpec {
            name: name.into(),
            description: name.into(),
            parameters: json!({"type":"object"}),
            constrained_sampling: None,
        });
        orchestration_child::capture(
            self.bridge.scope(),
            call,
            &self.b,
            &self.settings,
            &launch,
            &tools,
            &PromptInput {
                cwd: "/project".into(),
                ..Default::default()
            },
            &[],
            1,
            7,
        )
    }
    fn call(&self, id: u64, name: &str) -> String {
        let cli_bridge::Admission::New(claim) = self
            .s
            .admit_cli_native_call(
                &self.bridge,
                &json!(id),
                name,
                &json!({"task":"child task"}),
                7,
            )
            .unwrap()
        else {
            panic!("new native call")
        };
        claim.call_id().into()
    }
    fn create(&self, p: &Frozen, child: &str, uuid: &str) -> Result<Created> {
        self.s
            .tx(|tx| Store::create_cli_native_child_in(tx, &self.bridge, p, child, uuid, 8))
    }
    fn counts(&self) -> (i64, i64, i64) {
        self.s
            .read(|c| {
                Ok((
                    c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(*) FROM cli_uuid_owners", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(*) FROM subagent_tasks", [], |r| r.get(0))?,
                ))
            })
            .unwrap()
    }
}
#[test]
fn startup_classifies_unlaunched_child_without_adopting_unjoined_metadata() {
    for intact in [true, false] {
        let f = Fixture::new();
        let call = f.call(1, "subagent");
        let policy = f.policy(&call, fresh()).unwrap();
        f.create(&policy, "child", UUID).unwrap();
        if !intact {
            // Corrupt only this owned fixture's native journal: a matching policy or
            // printed call ID alone must not be adopted by cleanup/recovery.
            f.s.tx(|c| {
                c.execute(
                    "DELETE FROM tool_calls WHERE session='parent' AND call_id=?1",
                    [&call],
                )?;
                Ok(())
            })
            .unwrap();
        }
        let before = f.counts();
        assert_eq!(
            f.s.recover_cli_native_children(9).unwrap(),
            usize::from(intact)
        );
        assert_eq!(f.s.recover_cli_native_children(10).unwrap(), 0);
        assert_eq!(f.counts(), before);
        let task = f.s.tasks("parent", 10).unwrap().0.remove(0);
        assert_eq!(task.status, if intact { "failed" } else { "pending" });
        assert!(
            f.s.cli_agent_turn("child", &format!("child:{call}"))
                .unwrap()
                .is_none()
        );
        if intact {
            assert!(task.result.unwrap().contains("do not replay"));
        }
    }
}
const UUID: &str = "22222222-2222-4222-8222-222222222222";
fn fresh() -> crate::profiles::ContextMode {
    crate::profiles::ContextMode::Fresh
}
#[test]
fn atomic_native_child_metadata_freezes_policy_but_admits_no_turn() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    let c = f.create(&p, "child", UUID).unwrap();
    assert!(c.created);
    assert!(c.revision.is_some());
    assert_eq!(c.binding.create.model, "haiku");
    assert_eq!(c.binding.create.parent.as_deref(), Some("parent"));
    assert_eq!(c.binding.root.as_deref(), Some("parent"));
    let mut identity = f.b.create.identity.clone();
    identity.cli_session_id = UUID.into();
    assert_eq!(c.binding.create.identity, identity);
    assert_eq!(c.binding.create.request_key, p.create_key());
    assert_eq!(
        c.task.launch.as_deref(),
        Some(p.record().to_json().as_str())
    );
    assert!(!c.task.launch.as_ref().unwrap().contains("cliNative"));
    let stored =
        f.s.read(|tx| Store::cli_native_child_policy_in(tx, "child"))
            .unwrap()
            .unwrap();
    assert_eq!(stored.0, p.digest());
    assert_eq!(stored.1, p.snapshot());
    assert_eq!(c.task.task, "child task");
    assert_eq!(c.task.status, "pending");
    assert_eq!(c.task.delivery, "inline");
    assert_eq!(f.counts(), (2, 2, 1));
    assert!(!f.s.session("child").unwrap().unwrap().subagents_enabled);
    assert!(f.s.open_run("child").unwrap().is_none());
    let turns: i64 =
        f.s.read(|tx| {
            Ok(tx.query_row(
                "SELECT COUNT(*) FROM cli_agent_turns WHERE session='child'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(turns, 0);
    assert!(
        f.s.read(|tx| Store::assert_native_execution(tx, "child"))
            .is_err()
    );
}
#[test]
fn exact_duplicate_is_metadata_only_and_does_not_bump_or_consume_caps() {
    let mut f = Fixture::new();
    f.settings
        .other
        .insert("subagents".into(), json!({"maxConcurrent":1,"maxTotal":1}));
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    let first = f.create(&p, "child", UUID).unwrap();
    let revision = f.s.session("parent").unwrap().unwrap().revision;
    let second = f
        .create(
            &p,
            "unused-proposal",
            "33333333-3333-4333-8333-333333333333",
        )
        .unwrap();
    assert!(!second.created);
    assert_eq!(second.revision, None);
    assert_eq!(first.binding, second.binding);
    assert_eq!(f.counts(), (2, 2, 1));
    assert_eq!(f.s.session("parent").unwrap().unwrap().revision, revision);
    let call = f.call(2, "subagent");
    let next = f.policy(&call, fresh()).unwrap();
    assert_ne!(p.create_key(), next.create_key());
    assert!(
        f.create(&next, "second", "33333333-3333-4333-8333-333333333333")
            .is_err()
    );
    assert_eq!(f.counts(), (2, 2, 1));
}
#[test]
fn late_task_insert_failure_rolls_back_binding_uuid_reservation_and_revision() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    let before = f.s.session("parent").unwrap().unwrap().revision;
    f.s.tx(|tx|{tx.execute_batch("CREATE TRIGGER reject_child BEFORE INSERT ON subagent_tasks BEGIN SELECT RAISE(ABORT,'injected task failure'); END;")?;Ok(())}).unwrap();
    assert!(f.create(&p, "child", UUID).is_err());
    assert_eq!(f.counts(), (1, 1, 0));
    assert_eq!(f.s.session("parent").unwrap().unwrap().revision, before);
    f.s.tx(|tx| {
        tx.execute_batch("DROP TRIGGER reject_child")?;
        Ok(())
    })
    .unwrap();
    assert!(f.create(&p, "child", UUID).unwrap().created);
}
#[test]
fn protocol_only_intents_and_wrong_native_names_cannot_create_children() {
    let f = Fixture::new();
    let cli_bridge::Admission::New(claim) =
        f.s.admit_cli_bridge_call(
            &f.bridge,
            &json!(1),
            "subagent",
            &json!({"task":"child task"}),
            7,
        )
        .unwrap()
    else {
        panic!("new protocol call")
    };
    let p = f.policy(claim.call_id(), fresh()).unwrap();
    assert!(f.create(&p, "child", UUID).is_err());
    let call = f.call(2, "read");
    let p = f.policy(&call, fresh()).unwrap();
    assert!(f.create(&p, "child", UUID).is_err());
    assert_eq!(f.counts(), (1, 1, 0));
}
#[test]
fn changed_policy_or_expired_parent_never_becomes_another_child() {
    let mut f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    f.create(&p, "child", UUID).unwrap();
    f.settings.other.insert(
        "subagents".into(),
        json!({"profiles":{"worker":{"tools":["read"]}}}),
    );
    let changed = f.policy(&call, fresh()).unwrap();
    assert_ne!(p.create_key(), changed.create_key());
    assert!(f.create(&changed, "another", UUID).is_err());
    f.s.abandon_cli_agent_turn(&f.root, 10).unwrap();
    assert!(f.create(&p, "another", UUID).is_err());
    assert_eq!(f.counts(), (2, 2, 1));
    assert_eq!(
        f.s.cli_agent_turn("parent", "request")
            .unwrap()
            .unwrap()
            .state,
        cli_agent_turns::State::Unknown
    );
    assert_eq!(
        f.s.tasks("parent", 10).unwrap().0[0].launch.as_deref(),
        Some(p.record().to_json().as_str())
    );
}
#[test]
fn inherit_keeps_task_separate_and_disabled_delegation_has_no_effect() {
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f
        .policy(&call, crate::profiles::ContextMode::Inherit)
        .unwrap();
    let created = f.create(&p, "child", UUID).unwrap();
    assert_eq!(created.task.task, "child task");
    assert!(p.context_tool().is_some());
    let f = Fixture::new();
    let call = f.call(1, "subagent");
    let p = f.policy(&call, fresh()).unwrap();
    f.s.tx(|tx| {
        tx.execute(
            "UPDATE sessions SET subagents_enabled=0 WHERE id='parent'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(f.create(&p, "child", UUID).is_err());
    assert_eq!(f.counts(), (1, 1, 0));
}

#[test]
fn features_child_effort_creation_and_repeated_receipt_keep_frozen_profile() {
    let mut f = Fixture::new();
    f.settings.other.insert(
        "subagents".into(),
        json!({"profiles":{"worker":{"tools":["read"],"thinking":"medium"}}}),
    );
    let call = f.call(1, "subagent");
    let policy = f
        .policy(&call, crate::profiles::ContextMode::Fresh)
        .unwrap();
    let uuid = "22222222-2222-4222-8222-222222222222";
    let first = f.create(&policy, "child", uuid).unwrap();
    assert_eq!(first.binding.create.thinking.as_deref(), Some("medium"));
    let repeated = f.create(&policy, "child", uuid).unwrap();
    assert!(!repeated.created);
    assert_eq!(first.binding, repeated.binding);
    f.settings.other.insert(
        "subagents".into(),
        json!({"profiles":{"worker":{"tools":["read"],"thinking":"low"}}}),
    );
    let changed = f
        .policy(&call, crate::profiles::ContextMode::Fresh)
        .unwrap();
    assert!(f.create(&changed, "child", uuid).is_err());
}

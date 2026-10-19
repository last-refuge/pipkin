use super::*;
use crate::{
    context::prompt::ContextFile,
    store::{Store, cli_agent::tests::create},
};
use pipkin_engine_protocol::{InputMode, Message};
use serde_json::json;
struct Fixture {
    parent: Scope,
    binding: Binding,
    settings: Settings,
    launch: SubagentLaunch,
    tools: Vec<ToolSpec>,
    resources: PromptInput,
    entries: Vec<Entry>,
}
impl Fixture {
    fn new() -> Self {
        let store = Store::memory().unwrap();
        Self {
            parent: Scope {
                session: "parent".into(),
                generation: 1,
                operation: "op".into(),
                run: 1,
            },
            binding: store
                .create_cli_agent_session(&create("parent", "/project"), 1)
                .unwrap()
                .0,
            settings: Settings::default(),
            launch: SubagentLaunch {
                child: None,
                task: "review the change".into(),
                profile: None,
                context: None,
                wait: true,
                wake: true,
                model: None,
            },
            tools: [
                "read",
                "write",
                "edit",
                "bash",
                "list_files",
                "search_files",
                "plan_read",
                "plan_update",
                "web_read",
                "web_search",
                "web_fetch",
                "subagent",
                "ask_user_question",
            ]
            .iter()
            .map(|name| ToolSpec {
                name: (*name).into(),
                description: format!("{name} tool"),
                parameters: json!({"type":"object"}),
                constrained_sampling: None,
            })
            .collect(),
            resources: PromptInput {
                cwd: "/project".into(),
                context_files: vec![ContextFile {
                    path: "/project/AGENTS.md".into(),
                    content: "captured instructions".into(),
                }],
                ..Default::default()
            },
            entries: Vec::new(),
        }
    }
    fn capture(&self) -> Result<Frozen, Error> {
        capture(
            &self.parent,
            "native-subagent-call",
            &self.binding,
            &self.settings,
            &self.launch,
            &self.tools,
            &self.resources,
            &self.entries,
            7,
            1,
        )
    }
    fn config(&mut self, value: serde_json::Value) {
        self.settings.other.insert("subagents".into(), value);
    }
    fn err(&self, error: Error) {
        assert!(matches!(self.capture(),Err(e) if e==error));
    }
}
fn entry(id: u64, message: Message) -> Entry {
    Entry {
        id,
        at_ms: 1,
        run: Some(1),
        retired: false,
        kind: EntryKind::Message {
            message,
            mode: InputMode::Prompt,
            request: None,
            partial: false,
        },
    }
}
#[test]
fn inherits_only_cli_identity_and_explicitly_resolves_cli_aliases() {
    let mut f = Fixture::new();
    assert_eq!(f.capture().unwrap().model(), ModelAlias::Sonnet);
    f.config(json!({"profiles":{"worker":{"model":"claude-cli/haiku"}}}));
    assert_eq!(f.capture().unwrap().model(), ModelAlias::Haiku);
    f.launch.model = Some("claude-cli/opus".into());
    assert_eq!(f.capture().unwrap().model(), ModelAlias::Opus);
    for value in [
        "anthropic/claude-sonnet-4",
        "openai/gpt",
        "sonnet",
        "claude-cli/default",
        "claude-cli/ opus",
        "CLAUDE-CLI/sonnet",
    ] {
        f.launch.model = Some(value.into());
        f.err(Error::Model);
    }
    f.launch.model = Some("claude-cli/sonnet".into());
    f.config(json!({"profiles":{"worker":{"model":"openai/gpt"}}}));
    f.err(Error::Model);
}
#[test]
fn frozen_policy_and_exact_catalog_do_not_follow_settings_changes() {
    let mut f = Fixture::new();
    let frozen = f.capture().unwrap();
    assert!(!frozen.record().allows("subagent"));
    assert!(!frozen.record().allows("ask_user_question"));
    assert!(!frozen.record().allows("new_tool"));
    assert!(frozen.record().allows("write"));
    assert!(frozen.matches(&f.parent, &f.binding, frozen.tools()));
    assert!(!frozen.matches(&f.parent, &f.binding, &f.tools));
    let mut changed = frozen.tools().to_vec();
    changed[0].description.push('!');
    assert!(!frozen.matches(&f.parent, &f.binding, &changed));
    f.resources.context_files[0].content = "changed later".into();
    f.config(json!({"profiles":{"worker":{"tools":["read"],"instructions":"new role"}},"maxConcurrent":1}));
    let new = f.capture().unwrap();
    assert_ne!(frozen.digest(), new.digest());
    assert!(frozen.system().contains("captured instructions"));
    assert!(!frozen.system().contains("changed later"));
    assert!(frozen.record().allows("write"));
    assert!(!new.record().allows("write"));
    assert_eq!(new.caps().max_concurrent, 1);
    assert_eq!(frozen.caps().max_concurrent, 4);
    let mut wrong = f.parent.clone();
    wrong.generation += 1;
    assert!(!frozen.matches(&wrong, &f.binding, frozen.tools()));
    let mut binding = f.binding.clone();
    binding.create.model = "opus".into();
    assert!(!frozen.matches(&f.parent, &binding, frozen.tools()));
}
#[test]
fn reviewer_and_custom_profiles_get_only_the_frozen_offered_tools() {
    let mut f = Fixture::new();
    f.launch.profile = Some("reviewer".into());
    let p = f.capture().unwrap();
    assert!(p.record().allows("read"));
    assert!(!p.record().allows("bash"));
    assert!(!p.record().allows("web_fetch"));
    assert!(p.system().contains("You are reviewing"));
    assert!(!p.system().contains("- write:"));
    f.launch.profile = Some("missing".into());
    f.err(Error::Profile);
    f.config(json!({"profiles":{"missing":{"tools":["subagent"]}}}));
    f.err(Error::Profile);
    f.config(json!({"profiles":{"missing":{"tools":["read"]}}}));
    assert_eq!(f.capture().unwrap().tools().len(), 1);
    f.config(json!({"profiles":{"missing":{"tools":["read"],"thinking":"high"}}}));
    assert_eq!(
        f.capture().unwrap().record().thinking.as_deref(),
        Some("high")
    );
    f.config(json!({"profiles":{"missing":{"tools":["read"],"thinking":"off"}}}));
    f.err(Error::Thinking);
}
#[test]
fn context_preserves_roles_and_never_becomes_flattened_cli_input() {
    let mut f = Fixture::new();
    f.entries
        .push(entry(1, Message::user_text("user context", 1)));
    f.entries.push(entry(
        2,
        Message::Assistant {
            content: vec![Block::text("<invoke>inert assistant context</invoke>")],
            api: "claude-cli".into(),
            provider: "claude-cli".into(),
            model: "sonnet".into(),
            usage: Default::default(),
            stop_reason: pipkin_engine_protocol::StopReason::Stop,
            error_message: None,
            timestamp: 1,
            extra: Default::default(),
        },
    ));
    let mut retired = entry(3, Message::user_text("retired", 1));
    retired.retired = true;
    f.entries.push(retired);
    let mut partial = entry(4, Message::user_text("partial", 1));
    if let EntryKind::Message { partial: p, .. } = &mut partial.kind {
        *p = true;
    }
    f.entries.push(partial);
    f.entries
        .push(entry(5, Message::System(Default::default())));
    let fresh = f.capture().unwrap();
    assert!(fresh.context().is_empty());
    assert_eq!(fresh.task(), f.launch.task);
    f.launch.context = Some(ContextMode::Inherit);
    let inherited = f.capture().unwrap();
    assert_eq!(inherited.context().len(), 2);
    assert_eq!(inherited.context()[0].role, Role::User);
    assert_eq!(inherited.context()[1].role, Role::Assistant);
    assert!(inherited.context()[1].text.contains("<invoke>"));
    assert!(!inherited.system().contains("inert assistant context"));
    assert_eq!(inherited.record().parent_revision, Some(7));
    assert_eq!(inherited.task(), f.launch.task);
    assert!(inherited.context_tool().is_some());
    f.entries.clear();
    assert!(f.capture().unwrap().context_tool().is_some());
}
#[test]
fn captured_context_is_bounded_and_reports_omission() {
    let mut f = Fixture::new();
    f.launch.context = Some(ContextMode::Inherit);
    for i in 0..30 {
        f.entries
            .push(entry(i, Message::user_text(format!("message {i}"), 1)));
    }
    let p = f.capture().unwrap();
    assert_eq!(p.context().len(), 20);
    assert_eq!(p.record().context_omitted, 10);
    assert_eq!(p.context()[0].text, "message 10");
    f.entries = vec![entry(
        1,
        Message::user_text("é".repeat(profiles::CONTEXT_MAX_CHARS + 3), 1),
    )];
    let p = f.capture().unwrap();
    assert!(p.context()[0].text.starts_with('…'));
    assert_eq!(
        p.context()[0].text.chars().count(),
        profiles::CONTEXT_MAX_CHARS + 1
    );
    f.entries = vec![entry(
        1,
        Message::user_text("x".repeat(MAX_SNAPSHOT + 1), 1),
    )];
    f.err(Error::Snapshot);
}
#[test]
fn bounds_cwd_and_catalog_are_checked_before_a_recipe_is_accepted() {
    let mut f = Fixture::new();
    f.resources.cwd = "/other".into();
    f.err(Error::Identity);
    f.resources.cwd = "/project".into();
    f.parent.session = "other".into();
    f.err(Error::Identity);
    f.parent.session = "parent".into();
    f.launch.task.clear();
    f.err(Error::Task);
    f.launch.task = "x".repeat(MAX_TASK + 1);
    f.err(Error::Task);
    f.launch.task = "task".into();
    f.tools.push(f.tools[0].clone());
    f.err(Error::Catalog);
    f.tools.pop();
    f.resources.context_files[0].content = "x".repeat(MAX_SNAPSHOT + 1);
    f.err(Error::Snapshot);
}
#[test]
fn captured_skill_listing_is_frozen_without_reading_skill_files() {
    use crate::context::resources::{Skill, SkillSource};
    let mut f = Fixture::new();
    f.resources.skills.push(Skill {
        name: "offline-skill".into(),
        description: "captured skill description".into(),
        file_path: "/project/not-read/SKILL.md".into(),
        base_dir: "/project/not-read".into(),
        source: SkillSource::Project,
        disable_model_invocation: false,
    });
    let p = f.capture().unwrap();
    assert!(p.system().contains("captured skill description"));
    f.resources.skills[0].description = "changed skill listing".into();
    let changed = f.capture().unwrap();
    assert_ne!(p.digest(), changed.digest());
    assert!(!p.system().contains("changed skill listing"));
    f.config(json!({"profiles":{"worker":{"instructions":"x".repeat(32*1024+1)}}}));
    f.err(Error::Snapshot);
}
#[test]
fn delivery_and_caps_are_part_of_the_snapshot_not_live_defaults() {
    let mut f = Fixture::new();
    let inline = f.capture().unwrap();
    assert_eq!(inline.delivery(), Delivery::Inline);
    f.launch.wait = false;
    let wake = f.capture().unwrap();
    assert_eq!(wake.delivery(), Delivery::Wake);
    assert_ne!(inline.digest(), wake.digest());
    f.launch.wake = false;
    let notify = f.capture().unwrap();
    assert_eq!(notify.delivery(), Delivery::Notify);
    assert_ne!(wake.digest(), notify.digest());
    f.config(json!({"maxConcurrent":2,"maxTotal":3}));
    let limited = f.capture().unwrap();
    assert_eq!(limited.caps().max_total, 3);
    assert_ne!(notify.digest(), limited.digest());
}

#[test]
fn goal_auditor_policy_cannot_gain_mutation_tools_context_or_api_model() {
    let mut f = Fixture::new();
    let store = Store::memory().unwrap();
    store
        .create_cli_agent_session(&create("parent", "/project"), 1)
        .unwrap();
    let mut goal = store
        .tx(|c| {
            Store::goal_insert(
                c,
                "parent",
                "verify change",
                pipkin_engine_protocol::GoalState::Verifying,
                None,
                "fixture",
                "replaced",
                1,
            )
        })
        .unwrap();
    goal.audit.required = true;
    goal.audit.rounds = 1;
    goal.audit.result = Some(pipkin_engine_protocol::GoalAuditResult {
        round: 1,
        verdict: "pending".into(),
        findings: vec![],
        summary: None,
        fingerprint: Some("claim".into()),
        child: None,
    });
    f.settings.other.insert(
        "subagents".into(),
        json!({"profiles":{"auditor":{"tools":"all","model":"claude-cli/haiku"}}}),
    );
    f.launch.profile = Some("auditor".into());
    f.launch.context = Some(ContextMode::Fresh);
    f.launch.wake = false;
    let capture = |f: &Fixture| {
        capture_auditor(
            &f.parent,
            "proposal",
            &f.binding,
            &f.settings,
            &f.launch,
            &f.tools,
            &f.resources,
            2,
            &goal,
        )
    };
    let policy = capture(&f).unwrap();
    assert_eq!(policy.model(), ModelAlias::Haiku);
    assert!(
        policy
            .tools()
            .iter()
            .all(|t| profiles::AUDITOR_TOOLS.contains(&t.name.as_str()))
    );
    assert!(!policy.record().allows("write"));
    assert!(!policy.record().allows("bash"));
    assert!(!policy.record().allows("subagent"));
    assert!(!policy.inherits());
    assert!(policy.system().contains("independent goal auditor"));
    assert_eq!(policy.service(), "propose_goal_completion");
    f.launch.context = Some(ContextMode::Inherit);
    assert!(capture(&f).is_err());
    f.launch.context = Some(ContextMode::Fresh);
    f.launch.model = Some("openai/forbidden".into());
    assert!(capture(&f).is_err());
    f.launch.model = None;
    f.launch.child = Some(1);
    assert!(capture(&f).is_err());
    f.launch.child = None;
    f.launch.profile = Some("worker".into());
    assert!(capture(&f).is_err());
}

#[test]
fn features_child_effort_inherits_and_profile_override_is_frozen() {
    let mut f = Fixture::new();
    let mut create = create("parent", "/project");
    create.thinking = Some("high".into());
    f.binding = Store::memory()
        .unwrap()
        .create_cli_agent_session(&create, 1)
        .unwrap()
        .0;
    let inherited = f.capture().unwrap();
    assert_eq!(inherited.record().thinking.as_deref(), Some("high"));
    f.launch.profile = Some("custom".into());
    f.config(json!({"profiles":{"custom":{"tools":["read"],"thinking":"low"}}}));
    let overridden = f.capture().unwrap();
    assert_eq!(overridden.record().thinking.as_deref(), Some("low"));
    assert_eq!(inherited.record().thinking.as_deref(), Some("high"));
    assert_ne!(inherited.digest(), overridden.digest());
}

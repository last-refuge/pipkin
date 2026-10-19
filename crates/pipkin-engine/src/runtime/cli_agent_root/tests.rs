//! Owned shell/metadata and authenticated SDK/MCP simulations. Never installed CLI inference.
#[path = "admission_tests.rs"]
mod admission_tests;
#[path = "background_tests.rs"]
mod background_tests;
#[path = "background_usage_tests.rs"]
mod background_usage_tests;
#[path = "child_continuation_tests.rs"]
mod child_continuation_tests;
#[path = "child_tests.rs"]
mod child_tests;
#[path = "desktop_controls_tests.rs"]
mod desktop_controls_tests;
#[path = "desktop_tests.rs"]
mod desktop_tests;
#[path = "goal_lifecycle_tests.rs"]
mod goal_lifecycle_tests;
#[path = "goal_tests.rs"]
mod goal_tests;
#[path = "host_tests.rs"]
mod host_tests;
#[path = "input_tests.rs"]
mod input_tests;
#[path = "job_tests.rs"]
mod job_tests;
#[path = "live_probe.rs"]
mod live_probe;
#[path = "owner_tests.rs"]
mod owner_tests;
#[path = "question_tests.rs"]
mod question_tests;
#[path = "runtime_tests.rs"]
mod runtime_tests;
#[path = "service_tests.rs"]
mod service_tests;
use super::*;
use crate::execution::claude_cli::{
    orchestration_control::Control,
    orchestration_policy::{FLAGS, HelperImage},
    orchestration_protocol::{PROTOCOL_VERSION, Session},
    policy::{Environment, ModelAlias},
    preflight::{self, Candidate, Request},
};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, sync::OnceLock};
const UUID: &str = "11111111-1111-4111-8111-111111111111";
fn image() -> Arc<HelperImage> {
    static IMAGE: OnceLock<Arc<HelperImage>> = OnceLock::new();
    IMAGE
        .get_or_init(|| HelperImage::capture_in(&std::env::temp_dir()).unwrap())
        .clone()
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<Store>,
    launch: Launch,
    peer: cli_bridge_channel::Launch,
    events: mpsc::Receiver<Update>,
    registry: crate::runtime::cli_agent_owner::Registry,
    connection: CancellationToken,
}
impl Fixture {
    async fn new(variant: &str) -> Self {
        Self::new_in(variant, None).await
    }
    async fn new_in(variant: &str, engine: Option<(Arc<crate::runtime::Shared>, u64)>) -> Self {
        let thinking = (variant == "goal-thinking").then_some("high");
        let background_goals = variant == "goal-background";
        let variant = if background_goals || thinking.is_some() {
            "goal-runner"
        } else {
            variant
        };
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fake-cli");
        let names = if matches!(variant, "goals" | "goal-runner") {
            vec![
                "mcp__pipkin__write",
                "mcp__pipkin__get_goal",
                "mcp__pipkin__update_goal",
                "mcp__pipkin__propose_goal_completion",
            ]
        } else if variant == "background" {
            vec![
                "mcp__pipkin__write",
                "mcp__pipkin__subagent",
                "mcp__pipkin__job_start",
                "mcp__pipkin__job_status",
                "mcp__pipkin__job_log",
                "mcp__pipkin__job_wait",
                "mcp__pipkin__job_cancel",
            ]
        } else if matches!(variant, "jobs" | "job-wake" | "job-short") {
            vec![
                "mcp__pipkin__write",
                "mcp__pipkin__job_start",
                "mcp__pipkin__job_status",
                "mcp__pipkin__job_log",
                "mcp__pipkin__job_wait",
                "mcp__pipkin__job_cancel",
            ]
        } else if variant == "services" {
            vec![
                "mcp__pipkin__write",
                "mcp__pipkin__mcp_search",
                "mcp__pipkin__mcp_describe",
                "mcp__pipkin__mcp_call",
                "mcp__pipkin__mcp_resources",
                "mcp__pipkin__mcp_read",
                "mcp__pipkin__mcp_prompts",
                "mcp__pipkin__mcp_prompt",
                "mcp__pipkin__web_search",
                "mcp__pipkin__web_fetch",
                "mcp__pipkin__web_read",
                "mcp__pipkin__subagent_status",
                "mcp__pipkin__subagent_result",
                "mcp__pipkin__subagent_wait",
            ]
        } else if variant == "children" {
            vec!["mcp__pipkin__write", "mcp__pipkin__subagent"]
        } else if variant == "plan" {
            vec![
                "mcp__pipkin__write",
                "mcp__pipkin__plan_update",
                "mcp__pipkin__plan_read",
            ]
        } else if variant == "question" {
            vec!["mcp__pipkin__write", "mcp__pipkin__ask_user_question"]
        } else {
            vec!["mcp__pipkin__write"]
        };
        let init = json!({"type":"system","subtype":"init","session_id":UUID,"tools":names});
        let text = json!({"type":"assistant","session_id":UUID,"message":{"id":"model-message","role":"assistant","content":[{"type":"tool_use","id":"sdk-tool-id","name":"mcp__pipkin__write","input":{"path":"not-execution-authority"}},{"type":"text","text":"done"}]}});
        let result = json!({"type":"user","session_id":UUID,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"sdk-tool-id","is_error":false,"content":"native result observed"}]}});
        let mut terminal = json!({"type":"result","session_id":UUID,"subtype":"success","is_error":false,"result":"done","usage":{"output_tokens":7}});
        if variant == "warning" {
            terminal["result"] = Value::Null;
        }
        let tail = match variant {
            "stop" => "while :; do sleep 0.02; done\n".to_string(),
            "trailing" => {
                "printf '%s\\n' '{\"type\":\"result\",\"session_id\":\"wrong\"}'\n".to_string()
            }
            "exit" => "exit 7\n".to_string(),
            _ => String::new(),
        };
        let terminal = if variant == "stop" {
            String::new()
        } else {
            let warning = if variant == "warning" {
                format!(
                    "printf '%s\\n' '{}'\n",
                    json!({"type":"assistant","session_id":UUID,"message":{"id":"lost","role":"assistant","content":[{"type":"not-supported","text":"PRIVATE"}]}})
                )
            } else {
                String::new()
            };
            format!(
                "{warning}printf '%s\\n' '{terminal}'\nwhile [ ! -f closed ]; do sleep 0.02; done\n"
            )
        };
        let stream = |event: Value| {
            format!(
                "printf '%s\\n' '{}'\n",
                json!({"type":"stream_event","session_id":UUID,"event":event,"parent_tool_use_id":null})
            )
        };
        let (before, after) = if variant == "stream" {
            (vec![
                json!({"type":"message_start","message":{"id":"model-message","role":"assistant","content":[]}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"sdk-tool-id","name":"mcp__pipkin__write","input":{"path":"not-execution-authority"}}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"do"}}),
            ].into_iter().map(&stream).collect::<String>(),vec![
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"ne"}}),
                json!({"type":"content_block_stop","index":1}),json!({"type":"message_stop"}),
            ].into_iter().map(&stream).collect::<String>())
        } else {
            (String::new(), String::new())
        };
        let script = format!(
            "#!/bin/sh\nmode=\"$1\"\nfor arg in \"$@\"; do [ \"$arg\" != '--print' ] || mode='--print'; done\nif [ \"$mode\" != '--print' ]; then if [ -f hold-metadata ]; then printf '%s' \"$$\" > metadata-pid; fi; while [ -f hold-metadata ]; do sleep 0.02; done; fi\ncase \"$mode\" in\n--version) printf '2.1.301 (Claude Code)\\n';;\n--help) printf '%s\\n' '{}';;\n--mcp-config) printf '%s\\n' '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"fixture@example.invalid\",\"orgId\":\"fixture\"}}';;\n--print)\nuuid=$(cat fixture-uuid)\nprintf 'spawn\\n' >> launches\nIFS= read -r prompt\n[ \"$prompt\" = 'offline root fixture' ] || exit 28\nprintf '%s\\n' '{init}'\n{before}while [ ! -f acknowledged ]; do sleep 0.02; done\n{after}printf '%s\\n' '{text}' '{result}'\n{terminal}{tail};;\n*) exit 99;;\nesac\n",
            FLAGS
                .iter()
                .chain(crate::execution::claude_cli::coding_policy::FLAGS)
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
        );
        let script = if matches!(
            variant,
            "children" | "job-wake" | "job-short" | "goal-runner" | "background"
        ) {
            let mut child_names = vec!["mcp__pipkin__write"];
            if variant == "background" {
                child_names.extend([
                    "mcp__pipkin__job_start",
                    "mcp__pipkin__job_status",
                    "mcp__pipkin__job_log",
                    "mcp__pipkin__job_wait",
                    "mcp__pipkin__job_cancel",
                ]);
            }
            let child_init =
                json!({"type":"system","subtype":"init","session_id":UUID,"tools":child_names});
            child_names.push("mcp__pipkin__parent_context");
            let inherited_init =
                json!({"type":"system","subtype":"init","session_id":UUID,"tools":child_names});
            script.replace("uuid=$(cat fixture-uuid)",r#"prev=''; uuid=''; launch_mode=''; system=''; effort='default'; input_format=''; for arg in "$@"; do if [ "$prev" = '--effort' ]; then effort="$arg"; fi; if [ "$prev" = '--input-format' ]; then input_format="$arg"; fi; if [ "$prev" = '--system-prompt-file' ]; then system="$arg"; fi; if [ "$prev" = '--session-id' ] || [ "$prev" = '--resume' ]; then uuid="$arg"; launch_mode="$prev"; fi; prev="$arg"; done
[ -n "$uuid" ] || exit 29
turn=1; if [ -f "turn-$uuid" ]; then turn=$(($(cat "turn-$uuid") + 1)); fi
printf '%s' "$turn" > "turn-$uuid"
marker="$uuid"; if [ "$turn" -gt 1 ]; then marker="$uuid-$turn"; fi
printf '%s %s %s\n' "$launch_mode" "$uuid" "$turn" >> launch-identities
printf '%s %s\n' "$effort" "$input_format" >> launch-options
cp "$system" "system-$marker""#)
                .replace("[ \"$prompt\" = 'offline root fixture' ] || exit 28",r#"printf '%s' "$prompt" > "prompt-$marker"
bare=0; disabled=0; for arg in "$@"; do [ "$arg" != '--bare' ] || bare=1; [ "$arg" != '--disable-slash-commands' ] || disabled=1; done
case "$prompt" in '/compact'|'/compact '*) [ "$bare" = 1 ] && [ "$disabled" = 0 ] || exit 30;; *) [ "$disabled" = 1 ] || exit 31;; esac
case "$prompt" in
'{"'*) [ "$input_format" = 'stream-json' ] || exit 32;;
'/compact'|'/compact '*|'queued '*|'offline root fixture'|'offline child fixture'|'offline continuation fixture'|'Pursue this ongoing goal: '*|'Audit round '*) ;;
'Background subagent #'*|'Background job #'*) [ "$launch_mode" = '--resume' ] || exit 28;;
*) exit 28;;
esac"#)
                .replace(&format!("printf '%s\\n' '{init}'"),&format!("if [ \"$uuid\" = \"$(cat fixture-uuid)\" ]; then printf '%s\\n' '{init}'; elif /usr/bin/grep -q 'Call parent_context' \"$system\"; then printf '%s\\n' '{inherited_init}'; else printf '%s\\n' '{child_init}'; fi"))
                .replace("! -f acknowledged","! -f \"acknowledged-$marker\"")
                .replace("! -f closed","! -f \"closed-$marker\"")
        } else {
            script
        };
        let script = if variant == "goal-runner" {
            let audit_init = json!({"type":"system","subtype":"init","session_id":UUID,"tools":["mcp__pipkin__read"]});
            let root_init = json!({"type":"system","subtype":"init","session_id":UUID,"tools":["mcp__pipkin__write","mcp__pipkin__read","mcp__pipkin__get_goal","mcp__pipkin__update_goal","mcp__pipkin__propose_goal_completion"]});
            script.replace("IFS= read -r prompt", "prompt=$(cat)")
                .replace(&init.to_string(),&root_init.to_string())
                .replace(&json!({"type":"system","subtype":"init","session_id":UUID,"tools":["mcp__pipkin__write"]}).to_string(),&audit_init.to_string())
                .replace(&format!("printf '%s\\n' '{text}' '{result}'"),"cat \"answer-$marker\"")
                .replace(&format!("printf '%s\\n' '{}'",json!({"type":"result","session_id":UUID,"subtype":"success","is_error":false,"result":"done","usage":{"output_tokens":7}})),"cat \"terminal-$marker\"")
        } else {
            script
        };
        let script = if background_goals {
            let original = json!({"type":"system","subtype":"init","session_id":UUID,"tools":["mcp__pipkin__write","mcp__pipkin__read","mcp__pipkin__get_goal","mcp__pipkin__update_goal","mcp__pipkin__propose_goal_completion"]});
            let mut root = original.clone();
            root["tools"].as_array_mut().unwrap().extend(
                [
                    "subagent",
                    "job_start",
                    "job_status",
                    "job_log",
                    "job_wait",
                    "job_cancel",
                ]
                .map(|n| json!(format!("mcp__pipkin__{n}"))),
            );
            let mut child = root.clone();
            child["tools"]
                .as_array_mut()
                .unwrap()
                .retain(|t| t != "mcp__pipkin__subagent");
            let audit = json!({"type":"system","subtype":"init","session_id":UUID,"tools":["mcp__pipkin__read"]});
            script.replace(&original.to_string(),&root.to_string()).replace(&format!("printf '%s\\n' '{audit}'"),&format!("if /usr/bin/grep -q 'independent goal auditor' \"$system\"; then printf '%s\\n' '{audit}'; else printf '%s\\n' '{child}'; fi"))
        } else {
            script
        };
        crate::execution::claude_cli::test_support::write_script(
            &exe,
            script.replace(UUID, "'\"$uuid\"'"),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env: Environment = [
            ("HOME".into(), dir.path().as_os_str().into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
        .into_iter()
        .collect();
        let prepared = preflight::check_orchestration(
            Request {
                candidate: Candidate::inspect(&exe, dir.path(), &env).unwrap(),
                trust_executable: true,
                trust_project: true,
                environment: env,
                model: ModelAlias::Sonnet,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let connection = CancellationToken::new();
        let (store, registry, owner) = if let Some((shared, owner)) = engine {
            (shared.store.clone(), shared.cli_agents.clone(), owner)
        } else {
            let registry = crate::runtime::cli_agent_owner::Registry::new(true);
            registry.connect_with_lifetime(1, connection.clone());
            (Arc::new(Store::memory().unwrap()), registry, 1)
        };
        let binding = registry
            .create_approved_with_thinking(&store, owner, &prepared, "s", "create", thinking)
            .unwrap()
            .0;
        std::fs::write(
            dir.path().join("fixture-uuid"),
            &binding.create.identity.cli_session_id,
        )
        .unwrap();
        let ownership = registry.begin_turn(&store, owner, "s").unwrap();
        let root = ownership
            .admit(&store, "r", 1, "op", "offline root fixture")
            .unwrap()
            .lease
            .unwrap();
        let mut tools = crate::tools::builtin_tools()
            .into_iter()
            .filter(|t| t.spec().name == "write")
            .collect::<Vec<_>>();
        if variant == "plan" {
            tools.extend([
                Arc::new(crate::tools::plan::PlanUpdateTool) as Arc<dyn crate::tools::Tool>,
                Arc::new(crate::tools::plan::PlanReadTool),
            ]);
        }
        if variant == "question" {
            tools.push(Arc::new(crate::tools::ask::AskUserQuestionTool));
        }
        if matches!(variant, "children" | "background") {
            tools.push(Arc::new(crate::runtime::subagents::SubagentTool));
        }
        if variant == "goal-runner" {
            tools.extend(
                crate::tools::builtin_tools()
                    .into_iter()
                    .filter(|t| t.spec().name == "read"),
            );
        }
        if matches!(variant, "goals" | "goal-runner") {
            tools.extend([
                Arc::new(crate::tools::goal::GetGoalTool) as Arc<dyn Tool>,
                Arc::new(crate::tools::goal::UpdateGoalTool),
                Arc::new(crate::tools::goal::ProposeGoalCompletionTool),
            ]);
        }
        if matches!(variant, "jobs" | "job-wake" | "job-short" | "background") {
            tools.extend([
                Arc::new(crate::tools::jobs::JobStartTool) as Arc<dyn Tool>,
                Arc::new(crate::tools::jobs::JobStatusTool),
                Arc::new(crate::tools::jobs::JobLogTool),
                Arc::new(crate::tools::jobs::JobWaitTool),
                Arc::new(crate::tools::jobs::JobCancelTool),
            ]);
        }
        if variant == "services" {
            tools.extend([
                Arc::new(crate::tools::mcp::McpSearchTool) as Arc<dyn Tool>,
                Arc::new(crate::tools::mcp::McpDescribeTool),
                Arc::new(crate::tools::mcp::McpCallTool),
                Arc::new(crate::tools::mcp::McpResourcesTool),
                Arc::new(crate::tools::mcp::McpReadTool),
                Arc::new(crate::tools::mcp::McpPromptsTool),
                Arc::new(crate::tools::mcp::McpPromptTool),
                Arc::new(crate::tools::web::WebSearchTool),
                Arc::new(crate::tools::web::WebFetchTool),
                Arc::new(crate::tools::web::WebReadTool),
                Arc::new(crate::tools::subagents::SubagentStatusTool),
                Arc::new(crate::tools::subagents::SubagentResultTool),
                Arc::new(crate::tools::subagents::SubagentWaitTool),
            ]);
        }
        if background_goals {
            tools.extend([
                Arc::new(crate::runtime::subagents::SubagentTool) as Arc<dyn Tool>,
                Arc::new(crate::tools::jobs::JobStartTool),
                Arc::new(crate::tools::jobs::JobStatusTool),
                Arc::new(crate::tools::jobs::JobLogTool),
                Arc::new(crate::tools::jobs::JobWaitTool),
                Arc::new(crate::tools::jobs::JobCancelTool),
            ]);
        }
        let catalog = Session::new(
            root.receipt().scope.clone(),
            &tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
        )
        .unwrap()
        .catalog_hash();
        let helper = image();
        let deadline = Instant::now()
            + Duration::from_secs(if variant == "deadline" {
                2
            } else if variant == "job-short" {
                3
            } else {
                20
            });
        let server = Server::bind_native(
            store.clone(),
            crate::store::cli_bridge::BridgeSpec {
                id: "b".into(),
                scope: root.receipt().scope.clone(),
                owner: ownership.owner_key().into(),
                binding_hash: binding.digest,
                capability_hash: [0; 32],
            },
            catalog,
            deadline,
        )
        .await
        .unwrap();
        let peer = server.launch();
        let assets = Assets::new(
            dir.path(),
            helper,
            server.native_launch().unwrap(),
            "offline system fixture",
        )
        .unwrap();
        let (publication, events) = mpsc::channel(16);
        Self {
            dir,
            store,
            peer,
            events,
            launch: Launch {
                ownership,
                root,
                server,
                assets,
                tools,
                services: Services::default(),
                cancel: CancellationToken::new(),
                publication,
            },
            registry,
            connection,
        }
    }
}
async fn client(peer: cli_bridge_channel::Launch, path: std::path::PathBuf, close: bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut reader, mut writer) = peer.connect(deadline).await.unwrap().split();
    for request in [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"write","arguments":{"path":"effect","content":"once"}}}),
    ] {
        if request["method"] == "tools/call" && path.join("wait-first-delta").exists() {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !path.join("delta-observed").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        writer
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        if request.get("id").is_some() {
            let reply: Value =
                serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
            assert!(reply.get("error").is_none());
        }
    }
    if path.join("early-close").exists() {
        writer
            .write(
                &serde_json::to_vec(&Control::InputClosed.value()).unwrap(),
                deadline,
            )
            .await
            .unwrap();
        let reply: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
        assert!(Control::Closed.matches(&reply));
        std::fs::write(path.join("closed"), "closed").unwrap();
        // The fake CLI cannot publish its terminal until its MCP peer has closed.
        std::fs::write(path.join("acknowledged"), "ack").unwrap();
        return;
    }
    std::fs::write(path.join("acknowledged"), "ack").unwrap();
    if close {
        let request: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
        assert!(Control::Quiesce.matches(&request));
        writer
            .write(
                &serde_json::to_vec(&Control::Quiesced.value()).unwrap(),
                deadline,
            )
            .await
            .unwrap();
        let reply: Value = serde_json::from_slice(&reader.read(deadline).await.unwrap()).unwrap();
        assert!(Control::Closed.matches(&reply));
        std::fs::write(path.join("closed"), "closed").unwrap();
    } else {
        assert!(reader.read(deadline).await.is_err());
    }
}
fn drain(mut receiver: mpsc::Receiver<Update>) -> tokio::task::JoinHandle<Vec<Update>> {
    tokio::spawn(async move {
        let mut updates = Vec::new();
        while let Some(u) = receiver.recv().await {
            assert_eq!(u.scope.session, "s");
            updates.push(u);
        }
        updates
    })
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_root_process_native_write_projection_and_clean_settlement() {
    let f = Fixture::new("success").await;
    let events = drain(f.events);
    let peer = tokio::spawn(client(f.peer, f.dir.path().into(), true));
    let receipt = run(f.store.clone(), f.launch).await.unwrap();
    peer.await.unwrap();
    assert_eq!(receipt.state, cli_agent_turns::State::Completed);
    let updates = events.await.unwrap();
    assert!(updates.windows(2).all(|w| w[0].revision < w[1].revision));
    assert!(updates.iter().any(|u| !u.changes.is_empty()));
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
        "once"
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("launches")).unwrap(),
        "spawn\n"
    );
    let entries = f.store.page("s", None, 20).unwrap().0;
    assert_eq!(entries.iter().filter(|e|matches!(&e.kind,pipkin_engine_protocol::EntryKind::Message {message:pipkin_engine_protocol::Message::Assistant {api,..},partial:false,..} if api=="claude-cli-native")).count(),1);
    assert_eq!(
        f.store
            .read(|c| Ok(c.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE state='done'",
                [],
                |r| r.get::<_, u64>(0)
            )?))
            .unwrap(),
        1
    );
    // No automatic resume: the original request returns lookup only, without a new lease.
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Completed
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_closure_before_terminal_preserves_success_and_exit_failure_distinction() {
    for variant in ["success", "exit"] {
        let f = Fixture::new(variant).await;
        std::fs::write(f.dir.path().join("early-close"), "").unwrap();
        let events = drain(f.events);
        let peer = tokio::spawn(client(f.peer, f.dir.path().into(), true));
        let result = run(f.store.clone(), f.launch).await;
        peer.await.unwrap();
        events.await.unwrap();
        assert_eq!(result.is_ok(), variant == "success");
        let expected = if variant == "success" {
            cli_agent_turns::State::Completed
        } else {
            cli_agent_turns::State::Unknown
        };
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            expected
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("launches")).unwrap(),
            "spawn\n"
        );
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_exit_or_trailing_identity_failure_cannot_complete_known_native_effects() {
    for variant in ["exit", "trailing"] {
        let f = Fixture::new(variant).await;
        let events = drain(f.events);
        let peer = tokio::spawn(client(f.peer, f.dir.path().into(), true));
        assert!(run(f.store.clone(), f.launch).await.is_err());
        peer.await.unwrap();
        events.await.unwrap();
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Unknown
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("effect")).unwrap(),
            "once"
        );
        let entries = f.store.page("s", None, 20).unwrap().0;
        assert!(entries.iter().filter(|e|matches!(&e.kind,pipkin_engine_protocol::EntryKind::Message {message:pipkin_engine_protocol::Message::Assistant {api,..},..} if api=="claude-cli-native")).all(|e|e.retired));
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_revokes_process_and_channel_without_replaying_a_known_write() {
    let f = Fixture::new("stop").await;
    let events = drain(f.events);
    let peer = tokio::spawn(client(f.peer, f.dir.path().into(), false));
    let cancel = f.launch.cancel.clone();
    let root = tokio::spawn(run(f.store.clone(), f.launch));
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        while !f.dir.path().join("acknowledged").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    cancel.cancel();
    assert!(root.await.unwrap().is_err());
    assert!(
        ready.is_ok(),
        "native Write acknowledgment was not observed"
    );
    peer.await.unwrap();
    events.await.unwrap();
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Unknown
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("launches")).unwrap(),
        "spawn\n"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_before_launch_is_rejected_without_spawning() {
    let f = Fixture::new("success").await;
    f.launch.cancel.cancel();
    assert_eq!(run(f.store.clone(), f.launch).await, Err(Error::Cancelled));
    assert!(!f.dir.path().join("launches").exists());
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Rejected
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_loss_is_recorded_without_inventing_execution_failure_or_known_usage() {
    let f = Fixture::new("warning").await;
    let events = drain(f.events);
    let peer = tokio::spawn(client(f.peer, f.dir.path().into(), true));
    assert_eq!(
        run(f.store.clone(), f.launch).await.unwrap().state,
        cli_agent_turns::State::Completed
    );
    peer.await.unwrap();
    events.await.unwrap();
    let evidence: Value = f
        .store
        .read(|c| {
            let raw: String = c.query_row(
                "SELECT evidence FROM cli_agent_turns WHERE request_key='r'",
                [],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&raw).unwrap())
        })
        .unwrap();
    assert_eq!(evidence["supervisor"]["presentation_incomplete"], true);
    assert_eq!(evidence["terminal"]["usage"]["output_tokens"], 7);
    assert!(evidence["terminal"]["usage"]["input_tokens"].is_null());
    assert!(
        f.store
            .page("s", None, 20)
            .unwrap()
            .0
            .iter()
            .any(|e| matches!(
                e.kind,
                pipkin_engine_protocol::EntryKind::Notice {
                    level: pipkin_engine_protocol::NoticeLevel::Warning,
                    ..
                }
            ))
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_loss_stops_the_process_and_cannot_complete() {
    let f = Fixture::new("success").await;
    drop(f.events);
    assert!(run(f.store.clone(), f.launch).await.is_err());
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Unknown
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("launches")).unwrap(),
        "spawn\n"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_executable_or_failed_launch_commit_never_spawns() {
    for changed in [true, false] {
        let f = Fixture::new("success").await;
        if changed {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(f.dir.path().join("fake-cli"))
                .unwrap()
                .write_all(b"# changed\n")
                .unwrap();
        } else {
            f.store.fail_next_write();
        }
        assert!(run(f.store.clone(), f.launch).await.is_err());
        assert!(!f.dir.path().join("launches").exists());
        assert_eq!(
            f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
            cli_agent_turns::State::Rejected
        );
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_helper_cannot_turn_a_root_process_into_success() {
    let f = Fixture::new("deadline").await;
    let events = drain(f.events);
    assert!(run(f.store.clone(), f.launch).await.is_err());
    events.await.unwrap();
    assert_eq!(
        f.store.cli_agent_turn("s", "r").unwrap().unwrap().state,
        cli_agent_turns::State::Unknown
    );
}

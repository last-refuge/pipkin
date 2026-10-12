//! The MCP servers section's state: commands reach the backend only from a connected real
//! engine, local view changes always apply, inputs are checked before they are sent, and
//! engine snapshots replace what is shown.

use pipkin_core::onboarding::SignInAnswer;
use pipkin_core::*;

fn real_state() -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now: 0,
        },
        Prefs::default(),
    );
    state.mode = Mode::Real;
    state.connection = Connection::Ready;
    state
}

fn server(name: &str, scope: McpScope) -> McpServer {
    McpServer {
        name: name.into(),
        scope,
        kind: McpKind::Stdio,
        enabled: true,
        state: McpServerState::Stopped,
        error: None,
        tools: Some(2),
        catalog: vec![
            McpTool {
                name: "read".into(),
                description: "Read".into(),
                enabled: true,
            },
            McpTool {
                name: "rm".into(),
                description: "Remove".into(),
                enabled: false,
            },
        ],
        disabled_tools: vec!["rm".into()],
        command: "npx srv".into(),
        env: vec!["T=$TOK".into()],
        keep_alive: false,
        idle_seconds: 300,
        in_flight: 0,
        auth: McpAuth::default(),
    }
}

fn backend(out: &Outcome) -> Vec<BackendRequest> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            Effect::Backend(r) => Some(r.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn management_commands_need_a_connected_real_engine_and_local_changes_do_not() {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now: 0,
        },
        Prefs::default(),
    );
    state.mode = Mode::Demo;
    state.connection = Connection::Ready;
    assert!(!state.mcp.available);
    assert!(state.dispatch(Command::McpRefresh).effects.is_empty());
    assert!(
        state
            .dispatch(Command::McpTestServer("a".into()))
            .effects
            .is_empty()
    );
    state.mode = Mode::Real;
    state.connection = Connection::Connecting;
    assert!(state.dispatch(Command::McpRefresh).effects.is_empty());
    state.connection = Connection::Ready;
    assert_eq!(
        backend(&state.dispatch(Command::McpRefresh)),
        [BackendRequest::McpRefresh { conversation: None }]
    );
    // Local view changes apply regardless.
    state.mode = Mode::Demo;
    state.apply_mcp(McpSnapshot {
        servers: vec![server("a", McpScope::User)],
        warnings: vec![],
        project_available: false,
    });
    assert!(state.mcp.available);
    let out = state.dispatch(Command::McpToggleTools("a".into()));
    assert!(out.effects.is_empty() && out.notes == [Note::Other]);
    assert_eq!(state.mcp.expanded.as_deref(), Some("a"));
    state.dispatch(Command::McpToggleTools("a".into()));
    assert_eq!(state.mcp.expanded, None);
}

#[test]
fn inputs_are_checked_before_they_are_sent_and_scopes_follow_the_server() {
    let mut state = real_state();
    state.apply_mcp(McpSnapshot {
        servers: vec![server("a", McpScope::Project)],
        warnings: vec!["w1".into()],
        project_available: true,
    });
    assert_eq!(state.mcp.warnings, ["w1"]);
    let bad = McpServerInput {
        scope: McpScope::User,
        name: "bad name".into(),
        kind: McpKind::Stdio,
        target: "x".into(),
        values: vec![],
    };
    let out = state.dispatch(Command::McpAddServer(bad));
    assert!(out.effects.is_empty());
    assert_eq!(
        state.mcp.notice.as_deref(),
        Some("Names use letters, digits, - _ and . only.")
    );
    let no_url = McpServerInput {
        scope: McpScope::User,
        name: "r".into(),
        kind: McpKind::Http,
        target: "example.com".into(),
        values: vec![],
    };
    state.dispatch(Command::McpAddServer(no_url));
    assert_eq!(
        state.mcp.notice.as_deref(),
        Some("The URL must start with http:// or https://.")
    );
    let good = McpServerInput {
        scope: McpScope::User,
        name: "fs".into(),
        kind: McpKind::Stdio,
        target: "npx -y srv".into(),
        values: McpServerInput::parse_values("T=$TOK\nL=lit"),
    };
    let out = state.dispatch(Command::McpAddServer(good.clone()));
    assert_eq!(
        backend(&out),
        [BackendRequest::McpUpsert {
            conversation: None,
            input: good
        }]
    );
    assert_eq!(state.mcp.busy.as_deref(), Some("Saving fs…"));
    assert_eq!(state.mcp.notice, None);
    // Project scope needs a project; the input's debug output never prints values.
    let project = McpServerInput {
        scope: McpScope::Project,
        name: "p".into(),
        kind: McpKind::Stdio,
        target: "x".into(),
        values: vec![("K".into(), "secret-literal".into())],
    };
    assert!(!format!("{project:?}").contains("secret-literal"));
    state.mcp.project_available = false;
    assert!(
        state
            .dispatch(Command::McpAddServer(project))
            .effects
            .is_empty()
    );
    assert!(
        state
            .mcp
            .notice
            .as_deref()
            .unwrap()
            .contains("Open a conversation")
    );
    // Removal and tool switches use the server's own scope.
    assert_eq!(
        backend(&state.dispatch(Command::McpRemoveServer("a".into()))),
        [BackendRequest::McpRemove {
            conversation: None,
            scope: McpScope::Project,
            name: "a".into()
        }]
    );
    assert!(
        state
            .dispatch(Command::McpRemoveServer("nope".into()))
            .effects
            .is_empty()
    );
    assert_eq!(
        backend(&state.dispatch(Command::McpSetToolEnabled {
            server: "a".into(),
            tool: "rm".into(),
            enabled: true
        })),
        [BackendRequest::McpSetDisabledTools {
            conversation: None,
            scope: McpScope::Project,
            name: "a".into(),
            disabled: vec![]
        }]
    );
    assert_eq!(
        backend(&state.dispatch(Command::McpSetToolEnabled {
            server: "a".into(),
            tool: "read".into(),
            enabled: false
        })),
        [BackendRequest::McpSetDisabledTools {
            conversation: None,
            scope: McpScope::Project,
            name: "a".into(),
            disabled: vec!["rm".into(), "read".into()]
        }]
    );
    assert_eq!(
        backend(&state.dispatch(Command::McpTestServer("a".into()))),
        [BackendRequest::McpTest {
            conversation: None,
            name: "a".into()
        }]
    );
    assert_eq!(state.mcp.busy.as_deref(), Some("Testing a…"));
    assert_eq!(
        backend(&state.dispatch(Command::McpSignIn("a".into()))),
        [BackendRequest::McpAuthStart {
            conversation: None,
            name: "a".into()
        }]
    );
    assert_eq!(
        backend(&state.dispatch(Command::McpAnswerSignIn {
            server: "a".into(),
            prompt: "p1".into(),
            response: SignInAnswer::new("code".into())
        })),
        [BackendRequest::McpAuthAnswer {
            name: "a".into(),
            prompt: "p1".into(),
            response: SignInAnswer::new("code".into())
        }]
    );
    assert_eq!(
        backend(&state.dispatch(Command::McpSignOut("a".into()))),
        [BackendRequest::McpAuthRevoke {
            conversation: None,
            name: "a".into()
        }]
    );
}

#[test]
fn import_previews_select_valid_entries_and_apply_sends_the_chosen_names() {
    let mut state = real_state();
    state.apply_mcp(McpSnapshot::default());
    assert!(
        state
            .dispatch(Command::McpPreviewImport("   ".into()))
            .effects
            .is_empty()
    );
    assert_eq!(
        state.mcp.notice.as_deref(),
        Some("Paste a configuration to import.")
    );
    let out = state.dispatch(Command::McpPreviewImport("{}".into()));
    assert_eq!(
        backend(&out),
        [BackendRequest::McpImportPreview {
            conversation: None,
            text: "{}".into()
        }]
    );
    assert_eq!(state.mcp.notice, None);
    state.mcp_import_previewed(McpImportPreview {
        text: "{}".into(),
        entries: vec![
            McpImportEntry {
                name: "a".into(),
                kind: "stdio".into(),
                summary: "npx a".into(),
                warnings: vec![],
                exists: false,
                valid: true,
                selected: true,
            },
            McpImportEntry {
                name: "b".into(),
                kind: "http".into(),
                summary: "https://b/mcp".into(),
                warnings: vec!["legacy SSE".into()],
                exists: true,
                valid: true,
                selected: true,
            },
            McpImportEntry {
                name: "c".into(),
                kind: "unknown".into(),
                summary: String::new(),
                warnings: vec!["neither command nor url".into()],
                exists: false,
                valid: false,
                selected: false,
            },
        ],
        warnings: vec![],
    });
    assert_eq!(state.mcp.busy, None);
    // Invalid entries cannot be selected; others toggle.
    state.dispatch(Command::McpToggleImportEntry("c".into()));
    assert!(!state.mcp.import.as_ref().unwrap().entries[2].selected);
    state.dispatch(Command::McpToggleImportEntry("b".into()));
    assert_eq!(state.mcp.import.as_ref().unwrap().chosen(), ["a"]);
    assert!(
        state
            .dispatch(Command::McpApplyImport(McpScope::Project))
            .effects
            .is_empty()
    );
    assert!(
        state
            .mcp
            .notice
            .as_deref()
            .unwrap()
            .contains("Open a conversation")
    );
    assert_eq!(
        backend(&state.dispatch(Command::McpApplyImport(McpScope::User))),
        [BackendRequest::McpImportApply {
            conversation: None,
            scope: McpScope::User,
            text: "{}".into(),
            names: vec!["a".into()]
        }]
    );
    state.dispatch(Command::McpToggleImportEntry("a".into()));
    assert!(
        state
            .dispatch(Command::McpApplyImport(McpScope::User))
            .effects
            .is_empty()
    );
    assert_eq!(
        state.mcp.notice.as_deref(),
        Some("Choose at least one server to import.")
    );
    state.dispatch(Command::McpDismissImport);
    assert!(state.mcp.import.is_none());
    // Results and failures land on the page, not as app notices.
    state.mcp_test_result(McpTestResult {
        name: "a".into(),
        ok: true,
        message: "Connected".into(),
        tools: vec!["x".into()],
    });
    assert!(state.mcp.test.is_some() && state.mcp.busy.is_none());
    state.mcp_failed("refused".into());
    assert_eq!(state.mcp.notice.as_deref(), Some("refused"));
    assert_eq!(state.notice, None);
    state.dispatch(Command::McpDismissNotice);
    assert!(state.mcp.notice.is_none() && state.mcp.test.is_none());
    // A snapshot replaces the servers and drops an expansion of a server that is gone.
    state.mcp.expanded = Some("gone".into());
    state.apply_mcp(McpSnapshot {
        servers: vec![server("a", McpScope::User)],
        warnings: vec![],
        project_available: true,
    });
    assert_eq!(state.mcp.expanded, None);
    assert_eq!(state.mcp.servers.len(), 1);
    assert!(
        !format!(
            "{:?}",
            McpAuth {
                url: Some("https://secret".into()),
                ..Default::default()
            }
        )
        .contains("secret")
    );
}

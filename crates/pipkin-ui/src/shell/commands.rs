//! One command registry shared by the palette, shortcuts and buttons. Availability comes from
//! `AppState::availability()` so every surface agrees.

use gpui::Action;
use pipkin_core::{
    AppState, Command, ImportFlow, ImportKind, ItemKind, Mode, RELEASE_CANCEL_TITLE,
    RELEASE_CONFIRM_TITLE, RELEASE_TITLE, ReleaseFlow, TextSize, Theme as ThemeChoice, ToolStatus,
};

use super::actions::*;
use crate::model::DemoControls;

pub enum Run {
    Action(Box<dyn Action>),
    Dispatch(Command),
    CliApprovals,
    InspectSubagent(Option<u64>),
    CopyDiagnostics,
    DemoScenario(&'static str),
    ToggleSaveFailure,
}

pub struct Cmd {
    pub title: String,
    pub group: &'static str,
    pub shortcut: Option<&'static str>,
    pub enabled: bool,
    pub checked: bool,
    pub run: Run,
}

fn cmd(
    title: &str,
    group: &'static str,
    shortcut: Option<&'static str>,
    enabled: bool,
    run: Run,
) -> Cmd {
    Cmd {
        title: title.into(),
        group,
        shortcut,
        enabled,
        checked: false,
        run,
    }
}

pub fn build(state: &AppState, demo: Option<&DemoControls>) -> Vec<Cmd> {
    let a = state.availability();
    let has_conv = state.current().is_some();
    let mut v = vec![
        cmd(
            "Orchestration: open dashboard",
            "Navigation",
            Some("Ctrl+Shift+A"),
            true,
            Run::Dispatch(Command::Dashboard(pipkin_core::dashboard::Command::Open)),
        ),
        cmd(
            "Orchestration: return to conversation",
            "Navigation",
            None,
            state.dashboard.open,
            Run::Dispatch(Command::Dashboard(pipkin_core::dashboard::Command::Close)),
        ),
        cmd(
            "New conversation",
            "Conversation",
            Some("Ctrl+N"),
            a.new_conversation,
            Run::Action(Box::new(NewConversation)),
        ),
        cmd(
            super::cli_approvals::LABEL,
            "Conversation",
            None,
            state
                .current()
                .is_some_and(|c| state.cli_approval_panel_available(c.id)),
            Run::CliApprovals,
        ),
        cmd(
            "Open project folder\u{2026}",
            "Conversation",
            Some("Ctrl+Shift+O"),
            state.can_create,
            Run::Action(Box::new(OpenProject)),
        ),
        cmd(
            "Session usage",
            "Conversation",
            None,
            has_conv,
            Run::Action(Box::new(OpenSession)),
        ),
        cmd(
            "Rename conversation",
            "Conversation",
            Some("F2"),
            has_conv && state.mode == Mode::Demo,
            Run::Action(Box::new(RenameConversation)),
        ),
        cmd(
            "Next conversation",
            "Conversation",
            Some("Alt+↓"),
            has_conv,
            Run::Action(Box::new(NextConversation)),
        ),
        cmd(
            "Previous conversation",
            "Conversation",
            Some("Alt+↑"),
            has_conv,
            Run::Action(Box::new(PrevConversation)),
        ),
        cmd(
            "Attach files…",
            "Composer",
            Some("Ctrl+O"),
            state.execution_capabilities().attachments,
            Run::Action(Box::new(AttachFiles)),
        ),
        cmd(
            "Choose model…",
            "Composer",
            Some("Ctrl+M"),
            state.provider_picker_available(),
            Run::Action(Box::new(OpenModelMenu)),
        ),
        cmd(
            "Send follow-up (queue)",
            "Run",
            Some("Ctrl+Enter"),
            a.queue,
            Run::Action(Box::new(QueueFollowUp)),
        ),
        cmd(
            "Steer current run",
            "Run",
            Some("Ctrl+Shift+Enter"),
            a.steer,
            Run::Action(Box::new(SteerRun)),
        ),
        cmd(
            "Stop run",
            "Run",
            Some("Ctrl+."),
            a.cancel,
            Run::Action(Box::new(CancelRun)),
        ),
        cmd(
            "Retry stop request",
            "Run",
            None,
            a.retry_stop,
            Run::Dispatch(Command::RetryStop),
        ),
        cmd(
            "Refresh workspace changes",
            "Changes",
            None,
            a.refresh_changes,
            Run::Dispatch(Command::RefreshChanges),
        ),
        cmd(
            "Refresh models",
            "Composer",
            None,
            a.refresh_models,
            Run::Dispatch(Command::RefreshModels),
        ),
        cmd(
            "Retry saving draft",
            "Composer",
            None,
            a.retry_save,
            Run::Dispatch(Command::RetrySave),
        ),
        cmd(
            "Dismiss saved-data notice",
            "Application",
            None,
            state.storage_issue.is_some(),
            Run::Dispatch(Command::DismissStorageIssue),
        ),
        cmd(
            "Check submission status",
            "Run",
            None,
            a.check_status,
            Run::Dispatch(Command::CheckStatus),
        ),
        cmd(
            "Retry last prompt",
            "Run",
            None,
            a.retry,
            Run::Dispatch(Command::Retry),
        ),
        cmd(
            "Load earlier messages",
            "Transcript",
            None,
            a.load_older,
            Run::Dispatch(Command::LoadOlder),
        ),
        cmd(
            "Jump to latest",
            "Transcript",
            Some("Ctrl+↓"),
            has_conv,
            Run::Action(Box::new(JumpToLatest)),
        ),
        cmd(
            "Focus transcript",
            "View",
            Some("Ctrl+J"),
            has_conv,
            Run::Action(Box::new(FocusTranscript)),
        ),
        cmd(
            "Focus composer",
            "View",
            Some("Ctrl+L"),
            has_conv,
            Run::Action(Box::new(FocusComposer)),
        ),
        cmd(
            "Toggle navigation",
            "View",
            Some("Ctrl+B"),
            true,
            Run::Action(Box::new(ToggleNav)),
        ),
        cmd(
            "Toggle changes inspector",
            "View",
            Some("Ctrl+I"),
            true,
            Run::Action(Box::new(ToggleInspector)),
        ),
        cmd(
            "Show subagent tasks",
            "View",
            None,
            has_conv,
            Run::Action(Box::new(ShowSubagents)),
        ),
        cmd(
            "Show plan",
            "View",
            None,
            state.current().is_some_and(|c| c.plan.available),
            Run::Action(Box::new(ShowPlan)),
        ),
        cmd(
            "Pause goal",
            "Goal",
            None,
            state
                .current()
                .and_then(|c| c.goal.as_ref())
                .is_some_and(|g| g.paused.is_none()),
            Run::Dispatch(Command::PauseGoal),
        ),
        cmd(
            "Resume goal",
            "Goal",
            None,
            state
                .current()
                .and_then(|c| c.goal.as_ref())
                .is_some_and(|g| match g.state {
                    Some(s) => s.resumable(),
                    None => g.paused.is_some(),
                }),
            Run::Dispatch(Command::ResumeGoal),
        ),
        cmd(
            "Clear goal",
            "Goal",
            None,
            state.current().is_some_and(|c| c.goal.is_some()),
            Run::Dispatch(Command::ClearGoal),
        ),
        cmd(
            "Copy diagnostics",
            "Support (metadata only; no logs, paths or conversation text)",
            None,
            true,
            Run::CopyDiagnostics,
        ),
        cmd(
            "Settings…",
            "View",
            Some("Ctrl+,"),
            true,
            Run::Action(Box::new(OpenPreferences)),
        ),
    ];
    // Importing a Pi profile: the built-in engine only, and never without a confirmation.
    if state.import_available {
        let can = state.can_start_import();
        v.push(cmd(
            "Import legacy conversations\u{2026}",
            "Import",
            None,
            can,
            Run::Dispatch(Command::ImportFromPi(ImportKind::Conversations)),
        ));
        v.push(cmd(
            "Import legacy provider settings and credentials\u{2026}",
            "Import",
            None,
            can,
            Run::Dispatch(Command::ImportFromPi(ImportKind::Credentials)),
        ));
        if let ImportFlow::Offered(offer) = &state.import {
            v.push(cmd(
                &offer.confirm_title(),
                "Import",
                None,
                state.connection.is_ready(),
                Run::Dispatch(Command::ConfirmImport),
            ));
            v.push(cmd(
                "Cancel the legacy import",
                "Import",
                None,
                true,
                Run::Dispatch(Command::CancelImport),
            ));
        }
    }
    // Handing a conversation the built-in engine owns back to Pi. Only Pi mode restores that
    // ownership, so this exists only there, and never without a confirmation.
    if state.current().is_some_and(|c| state.owned_elsewhere(c.id)) {
        v.push(cmd(
            RELEASE_TITLE,
            "Conversation",
            None,
            state.can_release(),
            Run::Dispatch(Command::ReleaseToPi),
        ));
    }
    if let ReleaseFlow::Confirming(_) = state.release {
        v.push(cmd(
            RELEASE_CONFIRM_TITLE,
            "Conversation",
            None,
            true,
            Run::Dispatch(Command::ConfirmRelease),
        ));
        v.push(cmd(
            RELEASE_CANCEL_TITLE,
            "Conversation",
            None,
            true,
            Run::Dispatch(Command::CancelRelease),
        ));
    }
    // Queued input can be withdrawn without a pointer: one palette entry per queued item.
    if let Some(conversation) = state.current() {
        for queued in conversation.queue.iter().take(8) {
            let text: String = queued.text.chars().take(48).collect();
            let more = if queued.text.chars().count() > 48 {
                "\u{2026}"
            } else {
                ""
            };
            v.push(cmd(
                &format!("Remove queued: {text}{more}"),
                "Run",
                None,
                !state.owned_elsewhere(conversation.id),
                Run::Dispatch(Command::RemoveQueued(queued.id)),
            ));
        }
    }
    // Search results can be opened without a pointer, and the way back is one command.
    for (i, hit) in state.history_search.hits.iter().take(8).enumerate() {
        let title = state
            .conversation(hit.conversation)
            .map_or("Conversation", |c| c.title.as_str());
        let snippet: String = hit
            .snippet
            .replace(['\u{2}', '\u{3}'], "")
            .chars()
            .take(60)
            .collect();
        v.push(cmd(
            &format!("Open search result: {title} \u{2014} {snippet}"),
            "Search",
            None,
            true,
            Run::Dispatch(Command::OpenSearchHit(i)),
        ));
    }
    if state.search_return.is_some() {
        v.push(cmd(
            "Go back to where the search started",
            "Search",
            None,
            true,
            Run::Dispatch(Command::ReturnFromSearch),
        ));
    }
    // Questions from extensions, and what they post.
    if let Some(conversation) = state.current() {
        if let Some(q) = conversation.ui_requests.first() {
            v.push(cmd(
                &format!("Answer the extension's question: {}", q.title),
                "Extensions",
                None,
                true,
                Run::Action(Box::new(AnswerQuestion)),
            ));
            v.push(cmd(
                &format!("Decline the extension's question: {}", q.title),
                "Extensions",
                None,
                true,
                Run::Dispatch(Command::CancelUiRequest(q.id.clone())),
            ));
        }
        if !conversation.ui_notices.is_empty() {
            v.push(cmd(
                "Dismiss notices from extensions",
                "Extensions",
                None,
                true,
                Run::Dispatch(Command::DismissUiNotices),
            ));
        }
        // The complete output of the latest finished tool call.
        if let Some(item) = conversation
            .items
            .iter()
            .rev()
            .find(|i| matches!(&i.kind, ItemKind::Tool(t) if t.status != ToolStatus::Running))
        {
            v.push(cmd(
                "Copy the full output of the latest tool call",
                "Transcript",
                None,
                true,
                Run::Dispatch(Command::CopyToolOutput(item.id)),
            ));
            v.push(cmd(
                "Save the full output of the latest tool call\u{2026}",
                "Transcript",
                None,
                true,
                Run::Dispatch(Command::SaveToolOutput(item.id)),
            ));
        }
        if let Some(change) = conversation
            .selected_change
            .and_then(|i| conversation.changes.get(i).map(|c| (i, c)))
        {
            v.push(cmd(
                &format!("Open in editor: {}", change.1.path),
                "Workspace",
                None,
                a.open_in_editor,
                Run::Dispatch(Command::OpenInEditor(change.0)),
            ));
        }
    }
    v.push(cmd(
        "Open a terminal in the project folder",
        "Workspace",
        None,
        a.open_terminal,
        Run::Dispatch(Command::OpenTerminal),
    ));
    // So can the attachments on the draft.
    if let Some(conversation) = state.current() {
        for (i, attachment) in conversation.draft.attachments.iter().enumerate() {
            v.push(cmd(
                &format!("Remove attachment: {}", attachment.name),
                "Composer",
                None,
                true,
                Run::Dispatch(Command::RemoveAttachment(i)),
            ));
        }
    }
    let theme = state.prefs.theme;
    if let Some(conv) = state
        .current()
        .filter(|conv| conv.subagents.enabled.is_some())
    {
        v.push(cmd(
            "Show all subagent tasks",
            "Subagents",
            None,
            true,
            Run::InspectSubagent(None),
        ));
        let sub = &conv.subagents;
        // A paged (native) directory pages its history, so the palette follows it: all
        // current work, then finished tasks newest first through the revealed pages (at least
        // the first, so finished work stays inspectable with history collapsed). Pi's
        // unpaged directory keeps its first-100 bound.
        let reachable: Vec<&pipkin_core::SubagentInfo> = if sub.paged {
            let current = sub
                .children
                .iter()
                .filter(|c| !pipkin_core::subagent_finished(&c.status));
            let finished = sub
                .children
                .iter()
                .rev()
                .filter(|c| pipkin_core::subagent_finished(&c.status))
                .take(sub.history_shown().max(pipkin_core::SUBAGENT_HISTORY_PAGE));
            current.chain(finished).collect()
        } else {
            sub.children.iter().take(100).collect()
        };
        if sub.paged {
            v.push(cmd(
                "Show more finished tasks",
                "Subagents",
                None,
                sub.can_show_more_history(),
                Run::Dispatch(Command::ShowMoreSubagentHistory),
            ));
            v.push(cmd(
                "Load earlier subagent activity",
                "Subagents",
                None,
                sub.can_load_older() && state.connection.is_ready(),
                Run::Dispatch(Command::LoadOlderSubagent),
            ));
        }
        for child in reachable {
            v.push(cmd(
                &format!("Inspect subagent {}: {}", child.id, child.task),
                "Subagents",
                None,
                true,
                Run::InspectSubagent(Some(child.id)),
            ));
        }
    }
    let mut c = cmd(
        "Theme: Dark",
        "Preferences",
        None,
        true,
        Run::Dispatch(Command::SetTheme(ThemeChoice::Dark)),
    );
    c.checked = theme == ThemeChoice::Dark;
    v.push(c);
    let mut c = cmd(
        "Theme: Light",
        "Preferences",
        None,
        true,
        Run::Dispatch(Command::SetTheme(ThemeChoice::Light)),
    );
    c.checked = theme == ThemeChoice::Light;
    v.push(c);
    for (title, size) in [
        ("Text size: Small", TextSize::Small),
        ("Text size: Normal", TextSize::Normal),
        ("Text size: Large", TextSize::Large),
    ] {
        let mut c = cmd(
            title,
            "Preferences",
            None,
            true,
            Run::Dispatch(Command::SetTextSize(size)),
        );
        c.checked = state.prefs.text_size == size;
        v.push(c);
    }
    let rm = state.prefs.reduced_motion;
    let mut c = cmd(
        "Reduced motion",
        "Preferences",
        None,
        true,
        Run::Dispatch(Command::SetReducedMotion(!rm)),
    );
    c.checked = rm;
    v.push(c);
    v.push(cmd(
        "Log performance stats",
        "Developer",
        None,
        true,
        Run::Action(Box::new(LogStats)),
    ));
    if let Some(d) = demo {
        let cur = (d.current_scenario)();
        for (name, desc) in &d.scenarios {
            let mut c = cmd(
                &format!("Demo scenario: {name} — {desc}"),
                "Developer",
                None,
                true,
                Run::DemoScenario(name),
            );
            c.checked = cur == *name;
            v.push(c);
        }
        let mut c = cmd(
            "Inject draft save failure",
            "Developer",
            None,
            true,
            Run::ToggleSaveFailure,
        );
        c.checked = (d.save_failure)();
        v.push(c);
    }
    v
}

/// Case-insensitive subsequence match; lower scores are better.
pub fn fuzzy_score(query: &str, text: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    let mut pos = 0;
    let mut score = 0;
    let mut last: Option<usize> = None;
    for qc in query.to_lowercase().chars() {
        let idx = text[pos..].find(qc)? + pos;
        score += match last {
            Some(l) if idx == l + qc.len_utf8() => 0,
            _ => idx - pos + 1,
        };
        last = Some(idx);
        pos = idx + qc.len_utf8();
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_subsequence_and_ranks_contiguous_first() {
        assert!(fuzzy_score("nc", "New conversation").is_some());
        assert!(fuzzy_score("zz", "New conversation").is_none());
        assert!(fuzzy_score("new", "New conversation") < fuzzy_score("nwc", "New conversation"));
        assert_eq!(fuzzy_score("", "x"), Some(0));
    }
}

#[cfg(test)]
mod removal_tests {
    use pipkin_core::*;

    use super::*;

    fn state_with_queue_and_attachments() -> AppState {
        let boot = Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "p".into(),
                path: "/p".into(),
            }],
            models: vec![],
            conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 1)],
            now: 1,
        };
        let mut s = AppState::new(boot, Prefs::default());
        s.prefs.selected_project = Some(ProjectId(1));
        s.dispatch(Command::SelectConversation(ConversationId(1)));
        let c = &mut s.conversations[0];
        c.queue = vec![
            QueuedPrompt {
                id: QueueId(7),
                text: "tidy the imports".into(),
                mode: QueueMode::FollowUp,
                source: None,
            },
            QueuedPrompt {
                id: QueueId(9),
                text: "x".repeat(80),
                mode: QueueMode::Steer,
                source: None,
            },
        ];
        c.draft.attachments = vec![Attachment {
            path: "/p/notes.txt".into(),
            name: "notes.txt".into(),
            size: Some(3),
            error: None,
        }];
        s
    }

    fn find<'a>(commands: &'a [Cmd], title: &str) -> &'a Cmd {
        commands
            .iter()
            .find(|c| c.title == title)
            .unwrap_or_else(|| panic!("no command {title:?}"))
    }

    #[test]
    fn diagnostics_is_available_in_real_demo_and_offline_states_without_a_conversation() {
        let mut state = state_with_queue_and_attachments();
        state.conversations.clear();
        state.selected = None;
        for mode in [Mode::Real, Mode::Demo] {
            state.mode = mode;
            let commands = build(&state, None);
            let command = find(&commands, "Copy diagnostics");
            assert!(command.enabled);
            assert!(matches!(command.run, Run::CopyDiagnostics));
            assert!(command.group.contains("metadata only"));
        }
    }

    #[test]
    fn task_activity_is_keyboard_reachable_only_for_an_advertised_directory() {
        let mut state = state_with_queue_and_attachments();
        assert!(
            !build(&state, None)
                .iter()
                .any(|c| matches!(c.run, Run::InspectSubagent(_)))
        );
        state.conversations[0].subagents.enabled = Some(false);
        state.conversations[0].subagents.children = vec![SubagentInfo {
            id: 9,
            task_id: 1,
            call_id: "call".into(),
            task: "Review Rust tests.".into(),
            status: "done".into(),
            profile: None,
            ..SubagentInfo::default()
        }];
        let commands = build(&state, None);
        assert!(matches!(
            find(&commands, "Inspect subagent 9: Review Rust tests.").run,
            Run::InspectSubagent(Some(9))
        ));
        assert!(matches!(
            find(&commands, "Show all subagent tasks").run,
            Run::InspectSubagent(None)
        ));
    }

    #[test]
    fn paged_directories_offer_history_and_older_activity_paging_by_keyboard() {
        let mut state = state_with_queue_and_attachments();
        let sub = &mut state.conversations[0].subagents;
        sub.enabled = Some(true);
        sub.children = (1..=250)
            .map(|id| SubagentInfo {
                id,
                task_id: id,
                call_id: format!("call-{id}"),
                task: format!("Task {id}."),
                status: if id == 7 { "running" } else { "done" }.into(),
                profile: None,
                ..SubagentInfo::default()
            })
            .collect();
        // Pi (unpaged): no paging entries, first 100 tasks only.
        let commands = build(&state, None);
        assert!(
            !commands
                .iter()
                .any(|c| c.title == "Show more finished tasks")
        );
        assert!(
            !commands
                .iter()
                .any(|c| c.title == "Load earlier subagent activity")
        );
        assert!(
            !commands
                .iter()
                .any(|c| c.title.starts_with("Inspect subagent 250:"))
        );
        // Native (paged): current work plus the newest finished page, then more as revealed.
        state.conversations[0].subagents.paged = true;
        let commands = build(&state, None);
        let more = find(&commands, "Show more finished tasks");
        assert!(more.enabled);
        assert!(matches!(
            more.run,
            Run::Dispatch(Command::ShowMoreSubagentHistory)
        ));
        assert!(!find(&commands, "Load earlier subagent activity").enabled);
        let inspect = |commands: &[Cmd]| {
            commands
                .iter()
                .filter(|c| matches!(c.run, Run::InspectSubagent(Some(_))))
                .count()
        };
        assert_eq!(inspect(&commands), 101);
        find(&commands, "Inspect subagent 250: Task 250.");
        find(&commands, "Inspect subagent 7: Task 7.");
        state.conversations[0].subagents.history = 300;
        let commands = build(&state, None);
        assert_eq!(inspect(&commands), 250);
        assert!(!find(&commands, "Show more finished tasks").enabled);
        find(&commands, "Inspect subagent 1: Task 1.");
    }

    #[test]
    fn rename_is_disabled_when_the_real_engine_cannot_rename_sessions() {
        let mut state = state_with_queue_and_attachments();
        assert!(find(&build(&state, None), "Rename conversation").enabled);
        state.mode = Mode::Real;
        assert!(!find(&build(&state, None), "Rename conversation").enabled);
    }

    #[test]
    fn queued_input_and_attachments_can_be_removed_from_the_palette() {
        let state = state_with_queue_and_attachments();
        let commands = build(&state, None);
        assert!(matches!(
            find(&commands, "Remove queued: tidy the imports").run,
            Run::Dispatch(Command::RemoveQueued(QueueId(7)))
        ));
        // Long text is shortened so the entry stays one line.
        let long = commands
            .iter()
            .find(|c| c.title.starts_with("Remove queued: xxxx"))
            .unwrap();
        assert_eq!(long.title.chars().count(), "Remove queued: ".len() + 48 + 1);
        assert!(long.title.ends_with('\u{2026}'));
        assert!(matches!(
            find(&commands, "Remove attachment: notes.txt").run,
            Run::Dispatch(Command::RemoveAttachment(0))
        ));
    }

    #[test]
    fn pi_import_commands_exist_only_for_the_built_in_engine_and_confirm_explicitly() {
        let mut state = state_with_queue_and_attachments();
        state.mode = Mode::Real;
        let titles = |s: &AppState| -> Vec<String> {
            build(s, None)
                .into_iter()
                .filter(|c| c.group == "Import")
                .map(|c| c.title)
                .collect()
        };
        assert!(titles(&state).is_empty(), "this engine offers no import");
        state.import_available = true;
        assert_eq!(
            titles(&state),
            vec![
                "Import legacy conversations\u{2026}",
                "Import legacy provider settings and credentials\u{2026}"
            ]
        );
        let commands = build(&state, None);
        let start = find(&commands, "Import legacy conversations\u{2026}");
        assert!(start.enabled);
        assert!(matches!(
            start.run,
            Run::Dispatch(Command::ImportFromPi(ImportKind::Conversations))
        ));
        assert!(matches!(
            find(
                &commands,
                "Import legacy provider settings and credentials\u{2026}"
            )
            .run,
            Run::Dispatch(Command::ImportFromPi(ImportKind::Credentials))
        ));

        state.dispatch(Command::ImportFromPi(ImportKind::Conversations));
        assert!(
            !find(&build(&state, None), "Import legacy conversations\u{2026}").enabled,
            "one import at a time"
        );
        state.import_discovered(Ok(ImportOffer {
            kind: ImportKind::Conversations,
            summary: "1 conversation can be imported.".into(),
            sessions: vec!["s1".into()],
            providers: vec![],
            config: vec![],
        }));
        let commands = build(&state, None);
        assert!(matches!(
            find(
                &commands,
                "Confirm: import 1 conversation from the legacy profile"
            )
            .run,
            Run::Dispatch(Command::ConfirmImport)
        ));
        assert!(matches!(
            find(&commands, "Cancel the legacy import").run,
            Run::Dispatch(Command::CancelImport)
        ));
        state.connection = Connection::Offline("x".into());
        let commands = build(&state, None);
        assert!(!find(&commands, "Import legacy conversations\u{2026}").enabled);
        assert!(
            !find(
                &commands,
                "Confirm: import 1 conversation from the legacy profile"
            )
            .enabled
        );
    }

    #[test]
    fn transcripts_do_not_render_legacy_history_badges() {
        // Provenance remains in the model for imports, not repeated above every entry.
        assert!(!include_str!("../transcript/view.rs").contains("Native-only history"));
        assert!(!include_str!("subagents.rs").contains("Native-only history"));
    }

    #[test]
    fn an_owned_conversation_offers_release_with_a_confirmation_and_nothing_that_writes() {
        let mut state = state_with_queue_and_attachments();
        state.mode = Mode::Real;
        let release = |s: &AppState| -> Vec<String> {
            build(s, None)
                .into_iter()
                .filter(|c| {
                    c.title.contains("back to the legacy engine") || c.title == RELEASE_CANCEL_TITLE
                })
                .map(|c| c.title)
                .collect()
        };
        assert!(release(&state).is_empty(), "nothing to release");
        state.restore_owner(
            ConversationId(1),
            ConversationOwner::NativeEngine { imported_at: 1 },
        );
        assert_eq!(release(&state), vec![RELEASE_TITLE]);
        let commands = build(&state, None);
        assert!(find(&commands, RELEASE_TITLE).enabled);
        assert!(!find(&commands, "Remove queued: tidy the imports").enabled);
        for title in [
            "Send follow-up (queue)",
            "Steer current run",
            "Retry last prompt",
        ] {
            assert!(!find(&commands, title).enabled, "{title}");
        }
        state.dispatch(Command::ReleaseToPi);
        assert_eq!(
            release(&state),
            vec![RELEASE_TITLE, RELEASE_CONFIRM_TITLE, RELEASE_CANCEL_TITLE]
        );
        assert!(matches!(
            find(&build(&state, None), RELEASE_CONFIRM_TITLE).run,
            Run::Dispatch(Command::ConfirmRelease)
        ));
        assert!(matches!(
            find(&build(&state, None), RELEASE_CANCEL_TITLE).run,
            Run::Dispatch(Command::CancelRelease)
        ));
        state.dispatch(Command::ConfirmRelease);
        assert!(!find(&build(&state, None), RELEASE_TITLE).enabled, "once");
        state.ownership_released(ConversationId(1), Ok(()));
        assert!(release(&state).is_empty());
    }

    #[test]
    fn with_nothing_queued_or_attached_there_are_no_removal_entries() {
        let mut state = state_with_queue_and_attachments();
        state.conversations[0].queue.clear();
        state.conversations[0].draft.attachments.clear();
        let commands = build(&state, None);
        assert!(!commands.iter().any(|c| c.title.starts_with("Remove ")));
    }
}

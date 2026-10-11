use crate::ids::*;
use crate::model::*;
use crate::protocol::*;

/// Largest tool output kept for display, in bytes.
pub const TOOL_OUTPUT_PREVIEW_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub struct ConversationState {
    pub id: ConversationId,
    pub project: ProjectId,
    pub title: String,
    pub updated_at: i64,
    pub items: Vec<TranscriptItem>,
    /// Child work belongs to this session; it never changes the parent draft or run state.
    pub subagents: SubagentState,
    pub has_older: bool,
    pub loading_older: bool,
    pub opened: bool,
    /// Attachment generation. Events carrying another generation are dropped.
    pub generation: u64,
    pub run: RunState,
    /// Latest committed usage from Pi for this conversation. None means not reported.
    pub usage: Option<SessionUsage>,
    /// A successful run finished while another conversation was selected.
    pub unseen_completion: bool,
    /// User-owned continuous goal. A paused goal never schedules another prompt.
    pub goal: Option<Goal>,
    goal_op: Option<OperationId>,
    pub recovery_notice: bool,
    pub stop_error: Option<String>,
    pub status_error: Option<String>,
    pub status_check_pending: bool,
    pub queue: Vec<QueuedPrompt>,
    /// Steers and follow-ups sent to the engine and not yet confirmed (real mode).
    pub pending_queue: Vec<PendingQueued>,
    /// Known only from its saved copy: the engine has not (yet) listed it. It goes away when the
    /// engine's list arrives without it.
    pub cached_only: bool,
    /// What is shown is a saved copy from this time (Unix seconds), not the engine's own state.
    pub cached_at: Option<i64>,
    /// Why the engine's own state could not be had while a saved copy is shown.
    pub stale_reason: Option<String>,
    /// Why the last attempt to load older history failed.
    pub older_error: Option<String>,
    /// Questions extensions are waiting on, in the order asked.
    pub ui_requests: Vec<UiRequest>,
    /// Answers sent and not yet taken by the engine.
    pub ui_answering: Vec<String>,
    pub ui_status: Vec<(String, String)>,
    pub ui_notices: Vec<UiNotice>,
    /// Why the engine refused the last answer.
    pub ui_error: Option<String>,
    /// A complete tool output has been asked for and is on its way.
    pub pending_output: Option<(String, OutputUse)>,
    pub draft: Draft,
    pub prompt_history: crate::prompt_history::PromptHistory,
    prompt_history_snapshot: Option<(usize, Option<ItemId>, Option<ItemId>)>,
    pub changes: Vec<FileChange>,
    pub changes_state: ChangesState,
    /// Invalidates cached diff rows even when paths and addition/removal counts are unchanged.
    pub changes_revision: u64,
    pub selected_change: Option<usize>,
    /// Reported by Pi for this conversation, not a global preference.
    pub thinking_level: Option<String>,
    pub thinking_levels: Vec<String>,
    /// Text of the last submission, kept for explicit retry.
    pub last_submission: Option<(String, Vec<Attachment>)>,
    /// A submission waiting for its journal commit. While set, nothing else can be submitted.
    pub pending_intent: Option<PendingIntent>,
    /// Why the last submission could not be recorded; its draft was kept.
    pub intent_error: Option<String>,
    /// Request behind the live (or unresolved) operation, for journal updates.
    current_request: Option<RequestId>,
    streaming_item: Option<usize>,
    /// The run shown as `Running` was not started by this window (the engine reported it busy),
    /// so only the engine's own state can end it.
    adopted: Option<OperationId>,
}

impl ConversationState {
    pub fn new(id: ConversationId, project: ProjectId, title: String, updated_at: i64) -> Self {
        ConversationState {
            id,
            project,
            title,
            updated_at,
            items: Vec::new(),
            subagents: SubagentState::default(),
            has_older: false,
            loading_older: false,
            opened: false,
            generation: 0,
            run: RunState::Idle,
            usage: None,
            unseen_completion: false,
            goal: None,
            goal_op: None,
            recovery_notice: false,
            stop_error: None,
            status_error: None,
            status_check_pending: false,
            queue: Vec::new(),
            pending_queue: Vec::new(),
            cached_only: false,
            cached_at: None,
            stale_reason: None,
            older_error: None,
            ui_requests: Vec::new(),
            ui_answering: Vec::new(),
            ui_status: Vec::new(),
            ui_notices: Vec::new(),
            ui_error: None,
            pending_output: None,
            draft: Draft::default(),
            prompt_history: crate::prompt_history::PromptHistory::default(),
            prompt_history_snapshot: None,
            changes: Vec::new(),
            changes_state: ChangesState::Unscanned,
            changes_revision: 0,
            selected_change: None,
            thinking_level: None,
            thinking_levels: Vec::new(),
            last_submission: None,
            pending_intent: None,
            intent_error: None,
            current_request: None,
            streaming_item: None,
            adopted: None,
        }
    }

    fn import_prompt_history(&mut self, older: bool) {
        // Streaming assistant updates must not repeatedly clone and merge every old prompt.
        let mut signature = (0, None, None);
        for item in &self.items {
            if matches!(&item.kind, ItemKind::User { .. }) {
                signature.0 += 1;
                signature.1.get_or_insert(item.id);
                signature.2 = Some(item.id);
            }
        }
        if self.prompt_history_snapshot == Some(signature) {
            return;
        }
        self.prompt_history_snapshot = Some(signature);
        let prompts = self
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::User { text, .. } if !text.starts_with("Pursue this ongoing goal: ") => {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect();
        self.prompt_history.import(prompts, older);
    }

    /// Short activity label for the navigation list.
    pub fn activity(&self) -> Option<&'static str> {
        match self.run {
            RunState::Idle if self.unseen_completion => Some("Done!"),
            RunState::Idle => None,
            ref r => Some(r.label()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goal {
    pub text: String,
    pub paused: Option<String>,
    /// Number of goal turns admitted to the submission journal in this process.
    pub turns: u64,
}

pub struct AppState {
    pub projects: Vec<Project>,
    pub models: Vec<ModelInfo>,
    pub conversations: Vec<ConversationState>,
    pub selected: Option<ConversationId>,
    pub search: String,
    pub prefs: Prefs,
    pub mode: Mode,
    pub connection: Connection,
    /// Why this build cannot send prompts, if it cannot. Shown wherever sending is offered.
    pub read_only: Option<String>,
    /// Whether the backend can create conversations (real mode); demo mode creates them locally.
    pub can_create: bool,
    /// A background action failed and there is no conversation to show it on.
    pub notice: Option<String>,
    /// Saved state could not be read or written as usual; shown until dismissed.
    pub storage_issue: Option<String>,
    /// The result of searching saved history for the text in the search box.
    pub history_search: SearchResults,
    /// The conversation to go back to after opening a search hit.
    pub search_return: Option<ConversationId>,
    /// A message the view is to scroll to once its conversation shows it.
    pub scroll_target: Option<(ConversationId, ItemId)>,
    target_pages: u32,
    /// Project folders the user opened, kept across catalogs.
    bookmarks: Vec<Project>,
    /// A conversation creation waiting for the backend to report it.
    pending_create: Option<RequestId>,
    request_prefix: String,
    next_request: u64,
    next_op: u64,
    next_local_item: u64,
    next_queue: u64,
    next_conversation: u64,
    now: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bootstrap {
    pub projects: Vec<Project>,
    pub models: Vec<ModelInfo>,
    pub conversations: Vec<(ConversationId, ProjectId, String, i64)>,
    pub now: i64,
}

impl AppState {
    pub fn new(boot: Bootstrap, prefs: Prefs) -> Self {
        let next_conversation = boot.conversations.iter().map(|c| c.0.0).max().unwrap_or(0) + 1;
        let conversations: Vec<_> = boot
            .conversations
            .into_iter()
            .map(|(id, p, t, at)| ConversationState::new(id, p, t, at))
            .collect();
        let mut prefs = prefs;
        if prefs.model.is_none() {
            prefs.model = boot.models.first().map(|m| m.id.clone());
        }
        let selected = prefs
            .selected_conversation
            .filter(|id| conversations.iter().any(|c| c.id == *id))
            .or_else(|| conversations.first().map(|c| c.id));
        AppState {
            projects: boot.projects,
            models: boot.models,
            conversations,
            selected,
            search: String::new(),
            prefs,
            mode: Mode::Demo,
            connection: Connection::Ready,
            read_only: None,
            can_create: false,
            notice: None,
            storage_issue: None,
            history_search: SearchResults::default(),
            search_return: None,
            scroll_target: None,
            target_pages: 0,
            bookmarks: Vec::new(),
            pending_create: None,
            request_prefix: "req".into(),
            next_request: 1,
            next_op: 1,
            next_local_item: LOCAL_ITEM_BASE,
            next_queue: 1,
            next_conversation,
            now: boot.now,
        }
    }

    pub fn now(&self) -> i64 {
        self.now
    }

    pub fn set_now(&mut self, now: i64) {
        self.now = now;
    }

    /// Restore existing journal history after this conversation appears in the catalog.
    pub fn restore_prompt_history(&mut self, id: ConversationId, prompts: Vec<String>) {
        if let Some(c) = self.conv_mut(id) {
            c.prompt_history.import(prompts, false);
        }
    }

    pub fn conversation(&self, id: ConversationId) -> Option<&ConversationState> {
        self.conversations.iter().find(|c| c.id == id)
    }

    fn conv_mut(&mut self, id: ConversationId) -> Option<&mut ConversationState> {
        self.conversations.iter_mut().find(|c| c.id == id)
    }

    pub fn current(&self) -> Option<&ConversationState> {
        self.selected.and_then(|id| self.conversation(id))
    }

    pub fn current_project(&self) -> Option<&Project> {
        // Only consult the current conversation when no project is selected: `current()?`
        // must not run eagerly, or an empty project would report no project at all.
        let p = match self.prefs.selected_project {
            Some(p) => p,
            None => self.current()?.project,
        };
        self.projects.iter().find(|x| x.id == p)
    }

    /// Conversations of the selected project that match the search, newest first.
    pub fn visible_conversations(&self) -> Vec<&ConversationState> {
        let project = self.current_project().map(|p| p.id);
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<_> = self
            .conversations
            .iter()
            .filter(|c| Some(c.project) == project)
            .filter(|c| q.is_empty() || c.title.to_lowercase().contains(&q))
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        v
    }

    /// Merge the backend's catalog into state. Existing conversations keep their local state.
    pub fn apply_catalog(&mut self, boot: Bootstrap) -> Outcome {
        let mut out = Outcome::default();
        self.projects = merge_projects(boot.projects, &self.bookmarks);
        self.models = boot.models;
        self.now = self.now.max(boot.now);
        // The engine's list is the truth: a conversation known only from a saved copy stays
        // only if the engine lists it.
        let listed: std::collections::HashSet<ConversationId> =
            boot.conversations.iter().map(|c| c.0).collect();
        self.conversations
            .retain(|c| !c.cached_only || listed.contains(&c.id));
        for c in &mut self.conversations {
            if listed.contains(&c.id) {
                c.cached_only = false;
            }
        }
        if self
            .selected
            .is_some_and(|id| self.conversation(id).is_none())
        {
            self.selected = None;
        }
        let mut created: Vec<(ConversationId, i64)> = Vec::new();
        for (id, project, title, at) in boot.conversations {
            if self.conv_mut(id).is_none() {
                self.conversations
                    .push(ConversationState::new(id, project, title, at));
                self.next_conversation = self.next_conversation.max(id.0 + 1);
                created.push((id, at));
            }
        }
        // A conversation we asked the backend to create has arrived: show it.
        if self.pending_create.is_some()
            && let Some((id, _)) = created.iter().max_by_key(|(_, at)| *at)
        {
            let id = *id;
            self.pending_create = None;
            out.merge(self.dispatch(Command::SelectConversation(id)));
        }
        if self.prefs.model.is_none() {
            self.prefs.model = self.models.first().map(|m| m.id.clone());
        }
        out.notes.push(Note::ConversationsChanged);
        out
    }

    /// The engine reports which model is selected; that, not a local choice, is what is shown.
    pub fn set_engine_model(&mut self, model: Option<String>) -> Outcome {
        let mut out = Outcome::default();
        if model.is_some() && self.prefs.model != model {
            self.prefs.model = model;
            out.notes.push(Note::Other);
        }
        out
    }

    /// Show a failure that has no conversation to attach to, and stop waiting on anything it ended.
    pub fn set_notice(&mut self, message: String) -> Outcome {
        self.pending_create = None;
        self.notice = Some(message);
        Outcome {
            notes: vec![Note::Other],
            ..Outcome::default()
        }
    }

    /// Restore a project folder remembered from an earlier run (no effects).
    pub fn restore_project(&mut self, path: &str) {
        let id = project_id_for_path(path);
        if self.bookmarks.iter().all(|p| p.id != id) {
            self.bookmarks.push(Project {
                id,
                name: project_name_for_path(path),
                path: path.to_owned(),
            });
        }
        self.projects = merge_projects(std::mem::take(&mut self.projects), &self.bookmarks);
    }

    /// Select the remembered (or first) conversation, if nothing is selected, so its history loads.
    pub fn select_initial(&mut self) -> Outcome {
        if self.selected.is_some() {
            return Outcome::default();
        }
        let id = self
            .prefs
            .selected_conversation
            .filter(|id| self.conversation(*id).is_some())
            .or_else(|| self.conversations.first().map(|c| c.id));
        match id {
            Some(id) => self.dispatch(Command::SelectConversation(id)),
            None => Outcome::default(),
        }
    }

    pub fn set_connection(&mut self, connection: Connection) -> Outcome {
        let mut out = Outcome::default();
        if self.connection != connection {
            let was_ready = self.connection.is_ready();
            self.connection = connection;
            // Questions from extensions cannot be answered without the engine; its own state is
            // read again when it is reachable.
            if !self.connection.is_ready() {
                for c in &mut self.conversations {
                    c.ui_requests.clear();
                    c.ui_answering.clear();
                    c.subagents = SubagentState::default();
                    if self.mode == Mode::Real && was_ready && c.run.op().is_some() {
                        c.recovery_notice = true;
                    }
                    if c.status_check_pending {
                        c.status_check_pending = false;
                        c.status_error = Some(
                            "The connection was interrupted before the status check finished."
                                .into(),
                        );
                    }
                    if c.changes_state == ChangesState::Loading {
                        c.changes_state = ChangesState::Unavailable(
                            "The engine connection was interrupted. Refresh when it reconnects."
                                .into(),
                        );
                    }
                }
            }
            out.notes.push(Note::Other);
            // Asking the engine what became of a submission is read-only, so it needs no
            // confirmation once the engine is reachable again.
            if !was_ready && self.connection.is_ready() {
                out.merge(self.check_unknown());
            }
        }
        out
    }

    /// Ask about the submission in doubt in the open conversation, if there is one.
    fn check_unknown(&mut self) -> Outcome {
        let mut out = Outcome::default();
        if self
            .current()
            .is_some_and(|c| c.opened && matches!(c.run, RunState::OutcomeUnknown { .. }))
        {
            out.merge(self.dispatch(Command::CheckStatus));
        }
        out
    }

    /// Record that saved state could not be used as usual.
    pub fn set_storage_issue(&mut self, message: String) -> Outcome {
        self.storage_issue = Some(message);
        Outcome {
            notes: vec![Note::Other],
            ..Outcome::default()
        }
    }

    pub fn availability(&self) -> Availability {
        // Every action below reaches the backend, so none is offered until it is connected.
        if !self.connection.is_ready() {
            return Availability::default();
        }
        // Real sessions are created by the engine, so creation is offered only once it can be asked.
        let new_conversation = (self.mode == Mode::Demo || self.can_create)
            && self.pending_create.is_none()
            && self.current_project().is_some();
        let Some(c) = self.current() else {
            return Availability {
                new_conversation,
                ..Availability::default()
            };
        };
        let has_text = !c.draft.text.trim().is_empty();
        let attachments_ok = c.draft.attachments.iter().all(|a| a.error.is_none());
        let ready = has_text && attachments_ok;
        let idle =
            matches!(c.run, RunState::Idle | RunState::Failed { .. }) && c.pending_intent.is_none();
        // A read-only build offers no way to send, steer, queue or retry.
        let can_send = self.read_only.is_none();
        // Real steers and follow-ups are journaled one at a time, like prompts.
        let intent_free = self.mode == Mode::Demo || c.pending_intent.is_none();
        Availability {
            new_conversation,
            // A conversation showing only a saved copy cannot be written to.
            submit: can_send && idle && ready && (self.mode == Mode::Demo || c.opened),
            steer: can_send && intent_free && matches!(c.run, RunState::Running { .. }) && ready,
            queue: can_send
                && intent_free
                && matches!(
                    c.run,
                    RunState::Running { .. } | RunState::Submitting { .. }
                )
                && ready,
            refresh_models: self.mode == Mode::Real && c.opened,
            refresh_changes: self.mode == Mode::Real
                && c.opened
                && c.changes_state != ChangesState::Loading,
            retry_save: matches!(c.draft.save, SaveState::Failed(_)),
            open_in_editor: self.mode == Mode::Real
                && self.current_project().is_some()
                && c.selected_change.is_some_and(|i| i < c.changes.len()),
            open_terminal: self.mode == Mode::Real && self.current_project().is_some(),
            cancel: matches!(
                c.run,
                RunState::Running { .. } | RunState::Submitting { .. }
            ),
            check_status: !c.status_check_pending
                && (matches!(c.run, RunState::OutcomeUnknown { .. })
                    || (self.mode == Mode::Real
                        && c.opened
                        && matches!(c.run, RunState::Running { .. } | RunState::Stopping { .. }))),
            retry_stop: self.mode == Mode::Real
                && c.opened
                && matches!(c.run, RunState::Stopping { .. })
                && c.stop_error.is_some(),
            retry: can_send
                && c.last_submission.is_some()
                && c.pending_intent.is_none()
                && (matches!(c.run, RunState::Failed { .. })
                    || matches!(
                        c.items.last().map(|i| &i.kind),
                        Some(ItemKind::User {
                            delivery: Delivery::Rejected,
                            ..
                        })
                    )),
            load_older: c.has_older && !c.loading_older,
        }
    }

    /// Prefix that makes request IDs unique across restarts (the controller supplies entropy;
    /// the core never reads a clock or random source).
    pub fn set_request_prefix(&mut self, prefix: String) {
        self.request_prefix = prefix;
    }

    fn alloc_request(&mut self) -> RequestId {
        let id = RequestId(format!("{}-{}", self.request_prefix, self.next_request));
        self.next_request += 1;
        id
    }

    fn alloc_op(&mut self) -> OperationId {
        let id = OperationId(self.next_op);
        self.next_op += 1;
        id
    }

    fn alloc_item(&mut self) -> ItemId {
        let id = ItemId(self.next_local_item);
        self.next_local_item += 1;
        id
    }

    fn push_item(
        &mut self,
        conv: ConversationId,
        kind: ItemKind,
        out: &mut Outcome,
    ) -> Option<usize> {
        let id = self.alloc_item();
        let at = self.now;
        let c = self.conv_mut(conv)?;
        c.items.push(TranscriptItem { id, at, kind });
        out.notes.push(Note::ItemsAppended(conv, 1));
        Some(c.items.len() - 1)
    }

    // ---------------------------------------------------------------- commands

    pub fn dispatch(&mut self, cmd: Command) -> Outcome {
        let mut out = self.dispatch_inner(cmd);
        if let Some(id) = self.selected
            && self.retitle(id)
        {
            out.notes.push(Note::ConversationsChanged);
        }
        out
    }

    /// In real mode a conversation is named for the last prompt sent in it: the engine gives a
    /// session only an id and a time, which say nothing about it. (Renaming is the engine's, and
    /// is not offered here.) Returns whether the title changed. A conversation with no prompt in
    /// view keeps whatever name it has.
    pub fn retitle(&mut self, id: ConversationId) -> bool {
        if self.mode != Mode::Real {
            return false;
        }
        let Some(c) = self.conv_mut(id) else {
            return false;
        };
        let last = c.items.iter().rev().find_map(|i| match &i.kind {
            ItemKind::User {
                text, steer: false, ..
            } => Some(text.as_str()),
            _ => None,
        });
        let Some(title) = last.map(prompt_title).filter(|t| !t.is_empty()) else {
            return false;
        };
        if c.title == title {
            return false;
        }
        c.title = title;
        true
    }

    fn dispatch_inner(&mut self, cmd: Command) -> Outcome {
        let mut out = Outcome::default();
        match cmd {
            Command::RemoveSignIn(provider)
                if self.mode == Mode::Real && self.connection.is_ready() =>
            {
                out.effects
                    .push(Effect::Backend(BackendRequest::RemoveSignIn(provider)));
            }
            Command::StartSignIn(provider)
                if self.mode == Mode::Real && self.connection.is_ready() =>
            {
                out.effects
                    .push(Effect::Backend(BackendRequest::StartSignIn(provider)));
            }
            Command::ReuseSignIn(provider)
                if self.mode == Mode::Real && self.connection.is_ready() =>
            {
                out.effects
                    .push(Effect::Backend(BackendRequest::ReuseSignIn(provider)));
            }
            Command::AnswerSignIn {
                attempt,
                challenge,
                response,
            } if self.mode == Mode::Real && self.connection.is_ready() => {
                out.effects
                    .push(Effect::Backend(BackendRequest::AnswerSignIn {
                        attempt,
                        challenge,
                        response,
                    }));
            }
            Command::CancelSignIn { attempt } if self.mode == Mode::Real => {
                out.effects
                    .push(Effect::Backend(BackendRequest::CancelSignIn { attempt }));
            }
            Command::RemoveSignIn(_)
            | Command::StartSignIn(_)
            | Command::ReuseSignIn(_)
            | Command::AnswerSignIn { .. }
            | Command::CancelSignIn { .. } => {}
            Command::SelectProject(p) => {
                self.prefs.selected_project = Some(p);
                let first = self.visible_conversations().first().map(|c| c.id);
                match first {
                    Some(id) => out.merge(self.dispatch(Command::SelectConversation(id))),
                    None => {
                        self.selected = None;
                        out.notes.push(Note::SelectionChanged);
                    }
                }
                out.effects.push(Effect::SavePrefs(self.prefs.clone()));
            }
            Command::SelectConversation(id) => {
                let real = self.mode == Mode::Real;
                let prev = self.selected;
                let Some(c) = self.conv_mut(id) else {
                    return out;
                };
                let project = c.project;
                // Flush the outgoing draft before leaving it.
                if let Some(prev) = prev.filter(|p| *p != id) {
                    out.merge(self.flush_draft(prev));
                }
                self.selected = Some(id);
                self.prefs.selected_project = Some(project);
                self.conv_mut(id).unwrap().unseen_completion = false;
                self.prefs.selected_conversation = Some(id);
                let c = self.conv_mut(id).unwrap();
                // Finished-task history starts collapsed whenever a conversation is entered.
                if prev != Some(id) {
                    c.subagents.history = 0;
                }
                // A real engine is attached to one session at a time, so coming back to a
                // conversation attaches it again; the demo keeps what it already loaded.
                if !c.opened || (real && prev != Some(id)) {
                    c.generation += 1;
                    if real {
                        c.changes_state = ChangesState::Unscanned;
                        c.status_check_pending = false;
                    }
                    let generation = c.generation;
                    // Until the engine answers, a saved copy is better than an empty screen.
                    if real && !c.opened && c.items.is_empty() {
                        out.effects.push(Effect::LoadCache { conversation: id });
                    }
                    out.effects.push(Effect::Backend(BackendRequest::Open {
                        conversation: id,
                        generation,
                    }));
                }
                out.notes.push(Note::SelectionChanged);
                out.effects.push(Effect::SavePrefs(self.prefs.clone()));
            }
            // Demo child reads require an explicitly advertised simulated service, not
            // merely Demo mode. Keep real attachment/selection behavior unchanged.
            Command::SelectSubagent(child)
                if self.mode == Mode::Real
                    || (self.mode == Mode::Demo
                        && self
                            .current()
                            .is_some_and(|c| c.opened && c.subagents.enabled.is_some())) =>
            {
                let Some(c) = self.selected.and_then(|id| self.conv_mut(id)) else {
                    return out;
                };
                if child.is_some_and(|id| !c.subagents.children.iter().any(|row| row.id == id)) {
                    return out;
                }
                c.subagents.select(child);
                if let Some(child) = child
                    && c.opened
                    && c.subagents.enabled.is_some()
                {
                    c.subagents.loading = true;
                    out.effects
                        .push(Effect::Backend(BackendRequest::OpenSubagent {
                            conversation: c.id,
                            generation: c.generation,
                            child,
                        }));
                }
                out.notes.push(Note::Other);
            }
            Command::SetSubagentsEnabled(enabled)
                if self.mode == Mode::Real && self.connection.is_ready() =>
            {
                if let Some(c) = self.selected.map(|id| self.conv(id))
                    && c.opened
                    && c.subagents.enabled.is_some()
                {
                    out.effects
                        .push(Effect::Backend(BackendRequest::SetSubagentsEnabled {
                            conversation: c.id,
                            generation: c.generation,
                            enabled,
                        }));
                }
            }
            Command::SelectSubagent(_) | Command::SetSubagentsEnabled(_) => {}
            // A projection of the directory the engine sent; nothing is read or deleted.
            Command::ToggleSubagentHistory => {
                if let Some(c) = self.selected.and_then(|id| self.conv_mut(id))
                    && c.subagents.enabled.is_some()
                {
                    c.subagents.toggle_history();
                    out.notes.push(Note::Other);
                }
            }
            Command::ShowMoreSubagentHistory => {
                if let Some(c) = self.selected.and_then(|id| self.conv_mut(id))
                    && c.subagents.show_more_history()
                {
                    out.notes.push(Note::Other);
                }
            }
            Command::LoadOlderSubagent if self.connection.is_ready() => {
                if let Some(c) = self.selected.and_then(|id| self.conv_mut(id))
                    && c.opened
                    && let Some((child, before)) = c.subagents.begin_older()
                {
                    out.effects
                        .push(Effect::Backend(BackendRequest::LoadOlderSubagent {
                            conversation: c.id,
                            generation: c.generation,
                            child,
                            before,
                        }));
                    out.notes.push(Note::Other);
                }
            }
            Command::LoadOlderSubagent => {}
            // Real sessions are created and renamed by the engine; a local-only row would be
            // simulated success.
            Command::RenameConversation(..) if self.mode == Mode::Real => {}
            Command::NewConversation if self.mode == Mode::Real => {
                if self.availability().new_conversation
                    && let Some(project) = self.current_project().cloned()
                {
                    let request = self.alloc_request();
                    self.pending_create = Some(request.clone());
                    out.effects
                        .push(Effect::Backend(BackendRequest::CreateConversation {
                            project: project.id,
                            cwd: project.path,
                            request,
                        }));
                    out.notes.push(Note::Other);
                }
            }
            Command::AddProject(path) => {
                let path = path.trim().trim_end_matches('/').to_string();
                if !path.starts_with('/') {
                    return self.set_notice("A project folder must be an absolute path.".into());
                }
                let id = project_id_for_path(&path);
                if self.bookmarks.iter().all(|p| p.id != id) {
                    out.effects.push(Effect::SaveProject { path: path.clone() });
                }
                self.restore_project(&path);
                out.notes.push(Note::ConversationsChanged);
                out.merge(self.dispatch(Command::SelectProject(id)));
            }
            Command::DismissNotice => {
                if self.notice.take().is_some() {
                    out.notes.push(Note::Other);
                }
            }
            Command::NewConversation => {
                let Some(project) = self.current_project().map(|p| p.id) else {
                    return out;
                };
                let id = ConversationId(self.next_conversation);
                self.next_conversation += 1;
                let mut c =
                    ConversationState::new(id, project, "New conversation".into(), self.now);
                c.opened = true;
                self.conversations.push(c);
                out.effects
                    .push(Effect::SaveConversation { conversation: id });
                out.notes.push(Note::ConversationsChanged);
                out.merge(self.dispatch(Command::SelectConversation(id)));
            }
            Command::RenameConversation(id, title) => {
                let title = title.trim().to_string();
                if title.is_empty() {
                    return out;
                }
                if let Some(c) = self.conv_mut(id) {
                    c.title = title;
                    out.effects
                        .push(Effect::SaveConversation { conversation: id });
                    out.notes.push(Note::ConversationsChanged);
                }
            }
            Command::SetSearch(q) => {
                self.search = q;
                out.notes.push(Note::ConversationsChanged);
                // Titles are filtered at once; saved messages are searched by the controller.
                let query = self.search.trim().to_owned();
                if self.mode == Mode::Real && query.chars().count() >= 2 {
                    out.effects.push(Effect::SearchHistory { query });
                } else if !self.history_search.query.is_empty()
                    || !self.history_search.hits.is_empty()
                {
                    self.history_search = SearchResults::default();
                }
            }

            direction @ (Command::PreviousPrompt | Command::NextPrompt) => {
                if let Some(id) = self.selected {
                    let c = self.conv_mut(id).unwrap();
                    if c.pending_intent.is_none()
                        && !matches!(
                            c.run,
                            RunState::Submitting { .. } | RunState::OutcomeUnknown { .. }
                        )
                    {
                        c.import_prompt_history(false);
                        let text = if direction == Command::PreviousPrompt {
                            c.prompt_history.previous(&c.draft.text)
                        } else {
                            c.prompt_history.next_prompt()
                        };
                        if let Some(text) = text
                            && c.draft.text != text
                        {
                            c.draft.text = text;
                            c.draft.rev += 1;
                            c.draft.sync_epoch += 1;
                            c.draft.save = SaveState::Dirty;
                            out.notes.push(Note::Other);
                        }
                    }
                }
            }
            Command::EditDraft(text) => {
                if let Some(id) = self.selected {
                    let c = self.conv_mut(id).unwrap();
                    if c.draft.text != text {
                        c.prompt_history.reset_navigation();
                        c.draft.text = text;
                        c.draft.rev += 1;
                        c.draft.save = SaveState::Dirty;
                        out.notes.push(Note::Other);
                    }
                }
            }
            Command::FlushDraft(id) => out.merge(self.flush_draft(id)),
            Command::AddAttachments(list) => {
                if let Some(id) = self.selected {
                    let c = self.conv_mut(id).unwrap();
                    for a in list {
                        if !c.draft.attachments.iter().any(|x| x.path == a.path) {
                            c.draft.attachments.push(a);
                        }
                    }
                    c.draft.rev += 1;
                    c.draft.save = SaveState::Dirty;
                    out.notes.push(Note::Other);
                }
            }
            Command::RemoveAttachment(i) => {
                if let Some(id) = self.selected {
                    let c = self.conv_mut(id).unwrap();
                    if i < c.draft.attachments.len() {
                        c.draft.attachments.remove(i);
                        c.draft.rev += 1;
                        c.draft.save = SaveState::Dirty;
                        out.notes.push(Note::Other);
                    }
                }
            }
            Command::SetThinkingLevel(level) => {
                if self.mode == Mode::Real
                    && let Some(c) = self.current()
                    && c.opened
                    && c.thinking_levels.contains(&level)
                {
                    out.effects
                        .push(Effect::Backend(BackendRequest::SetThinkingLevel {
                            conversation: c.id,
                            generation: c.generation,
                            level,
                        }));
                }
            }
            Command::SetModel(m) => {
                if self.models.iter().any(|x| x.id == m) {
                    if self.mode == Mode::Real {
                        // Ask the engine; the selection changes only when it reports it.
                        if let Some(id) = self.selected.filter(|id| self.conv(*id).opened) {
                            out.effects.push(Effect::Backend(BackendRequest::SetModel {
                                conversation: id,
                                generation: self.conv(id).generation,
                                model: m,
                            }));
                        }
                    } else {
                        self.prefs.model = Some(m);
                        out.effects.push(Effect::SavePrefs(self.prefs.clone()));
                        out.notes.push(Note::Other);
                    }
                }
            }

            Command::CopyToolOutput(item) => out.merge(self.tool_output(item, OutputUse::Copy)),
            Command::SaveToolOutput(item) => out.merge(self.tool_output(item, OutputUse::Save)),
            Command::AnswerUiRequest { id, answer } => {
                if let Some(conv) = self.selected {
                    let c = self.conv_mut(conv).unwrap();
                    if c.ui_requests.iter().any(|r| r.id == id) && !c.ui_answering.contains(&id) {
                        c.ui_answering.push(id.clone());
                        c.ui_error = None;
                        let generation = c.generation;
                        out.effects.push(Effect::Backend(BackendRequest::UiRespond {
                            conversation: conv,
                            generation,
                            id,
                            answer,
                        }));
                        out.notes.push(Note::Other);
                    }
                }
            }
            Command::CancelUiRequest(id) => {
                if let Some(conv) = self.selected {
                    let c = self.conv_mut(conv).unwrap();
                    if c.ui_requests.iter().any(|r| r.id == id) {
                        let generation = c.generation;
                        out.effects.push(Effect::Backend(BackendRequest::UiCancel {
                            conversation: conv,
                            generation,
                            id,
                        }));
                    }
                }
            }
            Command::DismissUiNotices => {
                if let Some(conv) = self.selected {
                    let c = self.conv_mut(conv).unwrap();
                    if !c.ui_notices.is_empty() || c.ui_error.is_some() {
                        c.ui_notices.clear();
                        c.ui_error = None;
                        out.notes.push(Note::Other);
                    }
                }
            }
            Command::OpenSearchHit(i) => {
                if let Some(hit) = self.history_search.hits.get(i).cloned()
                    && self.conversation(hit.conversation).is_some()
                {
                    if self.selected != Some(hit.conversation) {
                        self.search_return = self.selected;
                    }
                    self.scroll_target = Some((hit.conversation, hit.item));
                    self.target_pages = 0;
                    // Showing the hit needs its conversation open, so a search never leaves the
                    // person looking at the wrong place.
                    self.search.clear();
                    self.history_search = SearchResults::default();
                    out.merge(self.dispatch(Command::SelectConversation(hit.conversation)));
                    out.merge(self.continue_to_target(hit.conversation));
                    out.notes.push(Note::ConversationsChanged);
                }
            }
            Command::ReturnFromSearch => {
                if let Some(back) = self.search_return.take() {
                    self.scroll_target = None;
                    out.merge(self.dispatch(Command::SelectConversation(back)));
                }
            }
            Command::ClearScrollTarget => {
                if self.scroll_target.take().is_some() {
                    out.notes.push(Note::Other);
                }
            }
            Command::OpenInEditor(i) => {
                if self.availability().open_in_editor
                    && let Some(project) = self.current_project()
                    && let Some(change) = self.current().and_then(|c| c.changes.get(i))
                {
                    let path = format!("{}/{}", project.path.trim_end_matches('/'), change.path);
                    out.effects.push(Effect::Launch(Launch::Editor {
                        path,
                        root: project.path.clone(),
                    }));
                }
            }
            Command::OpenTerminal => {
                if self.availability().open_terminal
                    && let Some(project) = self.current_project()
                {
                    out.effects.push(Effect::Launch(Launch::Terminal {
                        cwd: project.path.clone(),
                    }));
                }
            }

            Command::Submit => {
                if self.availability().submit {
                    out.merge(self.submit_from_draft(IntentOrigin::Draft));
                }
            }
            Command::SetGoal(text) => {
                let text = text.trim().to_string();
                if !text.is_empty()
                    && let Some(id) = self.selected
                {
                    if let Some(c) = self.conv_mut(id) {
                        c.goal = Some(Goal {
                            text: text.clone(),
                            paused: None,
                            turns: 0,
                        });
                        out.effects.push(Effect::SaveGoal {
                            conversation: id,
                            text: Some(text),
                            paused: None,
                        });
                        out.notes.push(Note::Other);
                    }
                    out.merge(self.start_goal_if_idle(id));
                }
            }
            Command::ClearGoal => {
                if let Some(id) = self.selected {
                    let (was_active, was_running_goal) = self
                        .conv_mut(id)
                        .map(|c| {
                            let running = c.goal_op.is_some() && c.run.op() == c.goal_op;
                            (c.goal.take().is_some(), running)
                        })
                        .unwrap_or((false, false));
                    if was_active {
                        out.notes.push(Note::Other);
                        out.effects.push(Effect::SaveGoal {
                            conversation: id,
                            text: None,
                            paused: None,
                        });
                        if was_running_goal && self.availability().cancel {
                            out.merge(self.dispatch(Command::Cancel));
                        }
                    }
                }
            }
            Command::Steer => {
                if self.availability().steer {
                    if self.mode == Mode::Real {
                        out.merge(self.submit_from_draft(IntentOrigin::Steer));
                    } else {
                        out.merge(self.steer());
                    }
                }
            }
            Command::QueueFollowUp => {
                if self.availability().queue && self.mode == Mode::Real {
                    out.merge(self.submit_from_draft(IntentOrigin::FollowUp));
                } else if self.availability().queue {
                    let id = self.selected.unwrap();
                    let qid = QueueId(self.next_queue);
                    self.next_queue += 1;
                    let c = self.conv_mut(id).unwrap();
                    let text = std::mem::take(&mut c.draft.text);
                    c.prompt_history.remember(text.clone());
                    c.draft.attachments.clear();
                    c.queue.push(QueuedPrompt {
                        id: qid,
                        text,
                        mode: QueueMode::FollowUp,
                    });
                    out.merge(self.reset_draft(id));
                }
            }
            Command::RemoveQueued(q) => {
                if let Some(id) = self.selected {
                    let real = self.mode == Mode::Real;
                    let c = self.conv_mut(id).unwrap();
                    if real {
                        // The engine owns the queue: ask it, and let its answer remove the row.
                        if c.opened && c.queue.iter().any(|x| x.id == q) {
                            let generation = c.generation;
                            out.effects
                                .push(Effect::Backend(BackendRequest::CancelQueued {
                                    conversation: id,
                                    generation,
                                    entry: q,
                                }));
                        }
                    } else {
                        c.queue.retain(|x| x.id != q);
                        out.notes.push(Note::Other);
                    }
                }
            }
            Command::DismissStorageIssue => {
                if self.storage_issue.take().is_some() {
                    out.notes.push(Note::Other);
                }
            }
            Command::RefreshChanges => {
                if self.availability().refresh_changes {
                    let c = self.conv_mut(self.selected.unwrap()).unwrap();
                    c.changes_state = ChangesState::Loading;
                    out.effects
                        .push(Effect::Backend(BackendRequest::RefreshChanges {
                            conversation: c.id,
                            generation: c.generation,
                        }));
                    out.notes.push(Note::Other);
                }
            }
            Command::RefreshModels => {
                if self.availability().refresh_models {
                    let id = self.selected.unwrap();
                    let generation = self.conv(id).generation;
                    out.effects
                        .push(Effect::Backend(BackendRequest::RefreshModels {
                            conversation: id,
                            generation,
                        }));
                }
            }
            Command::RetrySave => {
                if let Some(id) = self.selected
                    && matches!(self.conv(id).draft.save, SaveState::Failed(_))
                {
                    self.conv_mut(id).unwrap().draft.save = SaveState::Dirty;
                    out.merge(self.flush_draft(id));
                }
            }
            Command::DismissRecoveryNotice => {
                if let Some(id) = self.selected {
                    self.conv_mut(id).unwrap().recovery_notice = false;
                    out.notes.push(Note::Other);
                }
            }
            Command::Cancel | Command::RetryStop => {
                let availability = self.availability();
                if (matches!(cmd, Command::Cancel) && availability.cancel)
                    || (matches!(cmd, Command::RetryStop) && availability.retry_stop)
                {
                    let id = self.selected.unwrap();
                    let c = self.conv_mut(id).unwrap();
                    let op = c.run.op().unwrap();
                    c.run = RunState::Stopping { op };
                    c.stop_error = None;
                    // Pause the goal that exists at the instant Stop is requested. A goal set
                    // afterwards is a replacement task and must not be paused by the old ack.
                    if matches!(cmd, Command::Cancel)
                        && let Some(goal) = &mut c.goal
                    {
                        goal.paused = Some("Stop requested.".into());
                        out.effects.push(Effect::SaveGoal {
                            conversation: id,
                            text: Some(goal.text.clone()),
                            paused: goal.paused.clone(),
                        });
                    }
                    let generation = c.generation;
                    out.effects.push(Effect::Backend(BackendRequest::Cancel {
                        conversation: id,
                        generation,
                        op,
                    }));
                    out.notes.push(Note::Other);
                }
            }
            Command::CheckStatus => {
                if self.availability().check_status {
                    let id = self.selected.unwrap();
                    let c = self.conv_mut(id).unwrap();
                    c.status_check_pending = true;
                    c.status_error = None;
                    out.effects
                        .push(Effect::Backend(BackendRequest::CheckStatus {
                            conversation: id,
                            generation: c.generation,
                            op: c.run.op().unwrap(),
                            request: c.current_request.clone(),
                        }));
                    out.notes.push(Note::Other);
                }
            }
            Command::Retry => {
                if self.availability().retry {
                    let id = self.selected.unwrap();
                    let c = self.conv_mut(id).unwrap();
                    if let Some((text, atts)) = c.last_submission.clone() {
                        out.merge(self.submit(id, text, atts, IntentOrigin::Retry));
                    }
                }
            }
            Command::DismissFailure => {
                if let Some(id) = self.selected {
                    let c = self.conv_mut(id).unwrap();
                    if c.intent_error.take().is_some() {
                        out.notes.push(Note::Other);
                    }
                    if matches!(c.run, RunState::Failed { .. }) {
                        c.run = RunState::Idle;
                        out.notes.push(Note::Other);
                    }
                }
            }

            Command::LoadOlder => {
                if self.availability().load_older {
                    let id = self.selected.unwrap();
                    let c = self.conv_mut(id).unwrap();
                    c.loading_older = true;
                    c.older_error = None;
                    let before = c.items.first().map(|i| i.id);
                    let generation = c.generation;
                    out.effects.push(Effect::Backend(BackendRequest::LoadOlder {
                        conversation: id,
                        generation,
                        before,
                    }));
                }
            }
            Command::SelectChange(i) => {
                if let Some(id) = self.selected {
                    let c = self.conv_mut(id).unwrap();
                    if i < c.changes.len() {
                        c.selected_change = Some(i);
                        self.prefs.inspector_open = true;
                        out.notes.push(Note::Other);
                    }
                }
            }

            Command::SetTheme(t) => self.pref(&mut out, |p| p.theme = t),
            Command::SetTextSize(t) => self.pref(&mut out, |p| p.text_size = t),
            Command::SetReducedMotion(v) => self.pref(&mut out, |p| p.reduced_motion = v),
            Command::SetNavWidth(w) => self.pref(&mut out, |p| p.nav_width = w.clamp(180.0, 420.0)),
            Command::SetInspectorWidth(w) => {
                self.pref(&mut out, |p| p.inspector_width = w.clamp(280.0, 8192.0))
            }
            Command::SetInspectorOpen(v) => self.pref(&mut out, |p| p.inspector_open = v),
            Command::SetWindowSize(w, h) => {
                // A nonsense size (NaN, or tiny while a window is being created) is ignored,
                // and so is the size already stored, which would only write the same prefs.
                if w.is_finite() && h.is_finite() {
                    let size = (
                        w.clamp(480.0, 8192.0).round(),
                        h.clamp(480.0, 8192.0).round(),
                    );
                    if self.prefs.window_size != Some(size) {
                        self.pref(&mut out, |p| p.window_size = Some(size));
                    }
                }
            }
        }
        out
    }

    fn pref(&mut self, out: &mut Outcome, f: impl FnOnce(&mut Prefs)) {
        f(&mut self.prefs);
        out.effects.push(Effect::SavePrefs(self.prefs.clone()));
        out.notes.push(Note::Other);
    }

    /// Copy or save a tool call's complete output: at once when the preview holds all of it,
    /// otherwise by fetching it from the engine.
    fn tool_output(&mut self, item: ItemId, purpose: OutputUse) -> Outcome {
        let mut out = Outcome::default();
        let Some(conv) = self.selected else {
            return out;
        };
        let c = self.conv_mut(conv).unwrap();
        let Some(tool) = c.items.iter().find_map(|i| match &i.kind {
            ItemKind::Tool(t) if i.id == item => Some(t.clone()),
            _ => None,
        }) else {
            return out;
        };
        let name = format!("{}-output.txt", tool.name);
        let finish = |text: String, out: &mut Outcome| {
            out.effects.push(match purpose {
                OutputUse::Copy => Effect::CopyText(text),
                OutputUse::Save => Effect::SaveText {
                    suggested_name: name.clone(),
                    text,
                },
            });
        };
        match tool.call_id {
            // The preview is everything there is, or the engine cannot be asked about it.
            Some(_) if !tool.truncated => finish(tool.output, &mut out),
            None => finish(tool.output, &mut out),
            Some(call_id) => {
                c.pending_output = Some((call_id.clone(), purpose));
                let generation = c.generation;
                out.effects
                    .push(Effect::Backend(BackendRequest::FetchToolOutput {
                        conversation: conv,
                        generation,
                        call_id,
                    }));
            }
        }
        out
    }

    /// While a search hit's message is not among the loaded items, keep loading older history,
    /// up to a bound, and give up with a notice when it is not there.
    fn continue_to_target(&mut self, id: ConversationId) -> Outcome {
        let mut out = Outcome::default();
        let Some((target_conv, item)) = self.scroll_target else {
            return out;
        };
        if target_conv != id {
            return out;
        }
        let Some(c) = self.conversation(id) else {
            return out;
        };
        if !c.opened || c.items.iter().any(|i| i.id == item) {
            return out;
        }
        if c.has_older && !c.loading_older && self.target_pages < 60 {
            self.target_pages += 1;
            out.merge(self.dispatch(Command::LoadOlder));
        } else if !c.has_older || self.target_pages >= 60 {
            self.scroll_target = None;
            out.merge(
                self.set_notice("That message is no longer in this conversation's history.".into()),
            );
        }
        out
    }

    /// A conversation known only from its saved copy, so reading is possible before (or
    /// without) the engine's own list.
    pub fn restore_cached_conversation(
        &mut self,
        id: ConversationId,
        project_path: &str,
        title: String,
        updated_at: i64,
    ) {
        if self.conversation(id).is_some() || !project_path.starts_with('/') {
            return;
        }
        self.restore_project(project_path);
        let mut c =
            ConversationState::new(id, project_id_for_path(project_path), title, updated_at);
        c.cached_only = true;
        self.next_conversation = self.next_conversation.max(id.0 + 1);
        self.conversations.push(c);
    }

    /// Show a saved copy of a conversation until the engine's own state arrives. Ignored once
    /// the conversation has anything of its own to show.
    pub fn apply_cache(
        &mut self,
        id: ConversationId,
        items: Vec<TranscriptItem>,
        has_older: bool,
        synced_at: i64,
    ) -> Outcome {
        let mut out = Outcome::default();
        let Some(c) = self.conv_mut(id) else {
            return out;
        };
        // The engine's refusal to open can arrive before the saved copy is read; the copy is
        // still better than the notice, and keeps the reason it is shown.
        let refusal = match c.items.as_slice() {
            [
                TranscriptItem {
                    id: ItemId(i),
                    kind:
                        ItemKind::Notice {
                            text,
                            level: NoticeLevel::Error,
                        },
                    ..
                },
            ] if *i == LOCAL_ITEM_BASE - 1 => Some(text.clone()),
            _ => None,
        };
        if c.opened || (!c.items.is_empty() && refusal.is_none()) || items.is_empty() {
            return out;
        }
        if refusal.is_some() {
            c.stale_reason = refusal;
        }
        c.items = items;
        c.import_prompt_history(true);
        c.has_older = has_older;
        c.cached_at = Some(synced_at);
        out.notes.push(Note::ItemsReset(id));
        if self.retitle(id) {
            out.notes.push(Note::ConversationsChanged);
        }
        out
    }

    /// The controller's answer to `Effect::SearchHistory`. A result for text no longer in the
    /// search box is dropped.
    pub fn apply_search_results(&mut self, results: SearchResults) -> Outcome {
        let mut out = Outcome::default();
        if results.query != self.search.trim() {
            return out;
        }
        self.history_search = results;
        out.notes.push(Note::ConversationsChanged);
        out
    }

    fn conv(&self, id: ConversationId) -> &ConversationState {
        self.conversation(id).expect("conversation exists")
    }

    fn flush_draft(&mut self, id: ConversationId) -> Outcome {
        let mut out = Outcome::default();
        if let Some(c) = self.conv_mut(id)
            && c.draft.save == SaveState::Dirty
        {
            c.draft.save = SaveState::Saving;
            out.effects.push(Effect::SaveDraft {
                conversation: id,
                text: c.draft.text.clone(),
                attachments: c.draft.attachments.clone(),
                rev: c.draft.rev,
            });
            out.notes.push(Note::Other);
        }
        out
    }

    /// Core-originated draft change: the editor must resync.
    fn reset_draft(&mut self, id: ConversationId) -> Outcome {
        let c = self.conv_mut(id).unwrap();
        c.draft.rev += 1;
        c.draft.sync_epoch += 1;
        c.draft.save = SaveState::Dirty;
        let mut out = self.flush_draft(id);
        out.notes.push(Note::Other);
        out
    }

    fn submit_from_draft(&mut self, origin: IntentOrigin) -> Outcome {
        let id = self.selected.unwrap();
        let c = self.conv_mut(id).unwrap();
        let text = c.draft.text.trim_end().to_string();
        let atts = c.draft.attachments.clone();
        self.submit(id, text, atts, origin)
    }

    /// First half of a submission: record the intent. The draft stays and nothing is sent until
    /// `intent_persisted` reports the commit.
    fn submit(
        &mut self,
        id: ConversationId,
        text: String,
        atts: Vec<Attachment>,
        origin: IntentOrigin,
    ) -> Outcome {
        let mut out = Outcome::default();
        if self.conv_mut(id).unwrap().pending_intent.is_some() {
            return out;
        }
        let request = self.alloc_request();
        let model = self.prefs.model.clone();
        let c = self.conv_mut(id).unwrap();
        c.intent_error = None;
        c.pending_intent = Some(PendingIntent {
            request: request.clone(),
            origin,
            text: text.clone(),
            attachments: atts.clone(),
            model: model.clone(),
        });
        out.effects.push(Effect::JournalIntent {
            conversation: id,
            request,
            text,
            attachments: atts,
            model,
            origin,
        });
        out.notes.push(Note::Other);
        out
    }

    /// The journal reported the outcome of a `JournalIntent`. On success the draft is cleared
    /// (if the user has not edited it since) and the request is sent; on failure the draft is
    /// kept and nothing is sent.
    pub fn intent_persisted(
        &mut self,
        id: ConversationId,
        request: &RequestId,
        result: Result<(), String>,
    ) -> Outcome {
        let mut out = self.intent_persisted_inner(id, request, result);
        // The prompt joins the transcript once it is durable; that is when it names the chat.
        if self.retitle(id) {
            out.notes.push(Note::ConversationsChanged);
        }
        out
    }

    fn intent_persisted_inner(
        &mut self,
        id: ConversationId,
        request: &RequestId,
        result: Result<(), String>,
    ) -> Outcome {
        let mut out = Outcome::default();
        let Some(c) = self.conv_mut(id) else {
            return out;
        };
        if c.pending_intent.as_ref().map(|p| &p.request) != Some(request) {
            return out; // stale or duplicate acknowledgment
        }
        let intent = c.pending_intent.take().unwrap();
        if let Err(e) = result {
            c.intent_error = Some(format!(
                "Could not save the prompt before sending, so it was not sent: {e}"
            ));
            if intent.origin == IntentOrigin::Goal
                && let Some(goal) = &mut self.conv_mut(id).unwrap().goal
            {
                goal.paused = Some(format!("Could not save goal turn: {e}"));
                out.effects.push(Effect::SaveGoal {
                    conversation: id,
                    text: Some(goal.text.clone()),
                    paused: goal.paused.clone(),
                });
            }
            if intent.origin == IntentOrigin::Queue {
                let qid = QueueId(self.next_queue);
                self.next_queue += 1;
                self.conv_mut(id).unwrap().queue.insert(
                    0,
                    QueuedPrompt {
                        id: qid,
                        text: intent.text,
                        mode: QueueMode::FollowUp,
                    },
                );
            }
            out.notes.push(Note::Other);
            return out;
        }
        if intent.origin == IntentOrigin::Goal {
            let still_current = self
                .conversation(id)
                .and_then(|c| c.goal.as_ref())
                .is_some_and(|goal| {
                    intent
                        .text
                        .starts_with(&format!("Pursue this ongoing goal: {}\n\n", goal.text))
                });
            if !still_current {
                out.effects.push(Effect::JournalState {
                    conversation: id,
                    request: intent.request,
                    state: JournalState::Cancelled,
                });
                out.merge(self.start_goal_if_idle(id));
                return out;
            }
        }
        out.merge(self.dispatch_intent(id, intent));
        out
    }

    /// Second half of a submission, after the intent is durable.
    fn dispatch_intent(&mut self, id: ConversationId, intent: PendingIntent) -> Outcome {
        let mut out = Outcome::default();
        let op = self.alloc_op();
        let now = self.now;
        let c = self.conv_mut(id).unwrap();
        if intent.origin != IntentOrigin::Goal {
            c.prompt_history.remember(intent.text.clone());
        }
        let mut clear_draft = false;
        match intent.origin {
            IntentOrigin::Draft => {
                // Keep anything the user typed after pressing send.
                clear_draft = c.draft.text.trim_end() == intent.text
                    && c.draft
                        .attachments
                        .iter()
                        .map(|a| &a.path)
                        .eq(intent.attachments.iter().map(|a| &a.path));
                if clear_draft {
                    c.draft.text.clear();
                    c.draft.attachments.clear();
                }
            }
            IntentOrigin::Retry => {
                // Drop the rejected prompt row before resubmitting.
                if let Some(ItemKind::User {
                    delivery: Delivery::Rejected,
                    ..
                }) = c.items.last().map(|i| &i.kind)
                {
                    c.items.pop();
                    out.notes.push(Note::ItemsReset(id));
                }
                c.run = RunState::Idle;
            }
            IntentOrigin::Queue => {}
            IntentOrigin::Goal => {
                if let Some(goal) = &mut c.goal {
                    goal.turns += 1;
                }
            }
            IntentOrigin::Steer | IntentOrigin::FollowUp => {
                // The run is untouched: the engine owns this input from here, and shows it in
                // its queue and, once placed, in the transcript.
                let draft_text = c.draft.text.trim_end() == intent.text
                    && c.draft
                        .attachments
                        .iter()
                        .map(|a| &a.path)
                        .eq(intent.attachments.iter().map(|a| &a.path));
                if draft_text {
                    c.draft.text.clear();
                    c.draft.attachments.clear();
                }
                let mode = if intent.origin == IntentOrigin::Steer {
                    QueueMode::Steer
                } else {
                    QueueMode::FollowUp
                };
                c.pending_queue.push(PendingQueued {
                    request: intent.request.clone(),
                    text: intent.text.clone(),
                    attachments: intent.attachments.clone(),
                    mode,
                    state: QueueSend::Sending,
                });
                let generation = c.generation;
                out.effects.push(Effect::Backend(BackendRequest::Queue {
                    conversation: id,
                    generation,
                    request: intent.request,
                    mode,
                    text: intent.text,
                    attachments: intent.attachments,
                }));
                if draft_text {
                    out.merge(self.reset_draft(id));
                }
                out.notes.push(Note::Other);
                return out;
            }
        }
        c.run = RunState::Submitting { op };
        c.goal_op = (intent.origin == IntentOrigin::Goal).then_some(op);
        c.last_submission = Some((intent.text.clone(), intent.attachments.clone()));
        c.current_request = Some(intent.request.clone());
        c.updated_at = now;
        c.streaming_item = None;
        let generation = c.generation;
        let visible_text = if intent.origin == IntentOrigin::Goal {
            self.conversation(id)
                .and_then(|c| c.goal.as_ref())
                .map(|g| g.text.clone())
                .unwrap_or_else(|| intent.text.clone())
        } else {
            intent.text.clone()
        };
        self.push_item(
            id,
            ItemKind::User {
                text: visible_text,
                attachments: intent.attachments.clone(),
                delivery: Delivery::Pending,
                steer: false,
            },
            &mut out,
        );
        if clear_draft {
            out.merge(self.reset_draft(id));
        }
        out.effects.push(Effect::Backend(BackendRequest::Submit {
            conversation: id,
            generation,
            op,
            request: intent.request,
            text: intent.text,
            attachments: intent.attachments,
            model: intent.model,
        }));
        out.effects
            .push(Effect::SaveConversation { conversation: id });
        out.notes.push(Note::ConversationsChanged);
        out
    }

    fn steer(&mut self) -> Outcome {
        let mut out = Outcome::default();
        let id = self.selected.unwrap();
        let c = self.conv_mut(id).unwrap();
        let text = std::mem::take(&mut c.draft.text).trim_end().to_string();
        c.prompt_history.remember(text.clone());
        let op = c.run.op().unwrap();
        let generation = c.generation;
        c.streaming_item = None;
        self.push_item(
            id,
            ItemKind::User {
                text: text.clone(),
                attachments: vec![],
                delivery: Delivery::Pending,
                steer: true,
            },
            &mut out,
        );
        out.effects.push(Effect::Backend(BackendRequest::Steer {
            conversation: id,
            generation,
            op,
            text,
        }));
        out.merge(self.reset_draft(id));
        out
    }

    // ------------------------------------------------------------------ events

    pub fn apply_event(&mut self, ev: BackendEvent) -> Outcome {
        let id = ev.conversation;
        let mut out = self.apply_event_inner(ev);
        if self.retitle(id) {
            out.notes.push(Note::ConversationsChanged);
        }
        out
    }

    fn apply_event_inner(&mut self, ev: BackendEvent) -> Outcome {
        let mut out = Outcome::default();
        let id = ev.conversation;
        let real = self.mode == Mode::Real;
        let selected = self.selected == Some(id);
        // What is worth keeping a saved copy of: a settled view of the conversation.
        let cache = real
            && matches!(
                ev.kind,
                EventKind::Opened { .. }
                    | EventKind::OlderPage { .. }
                    | EventKind::Synced { .. }
                    | EventKind::Completed
                    | EventKind::Failed { .. }
                    | EventKind::Cancelled
            );
        let opened_event = matches!(ev.kind, EventKind::Opened { .. });
        let reached_target = matches!(
            ev.kind,
            EventKind::Opened { .. } | EventKind::OlderPage { .. }
        );
        let Some(c) = self.conv_mut(id) else {
            return out;
        };
        // Stale generation: an earlier attachment's content must never land here. What became of
        // a request or a run is not content: the operation or request key identifies it, so it
        // applies however many times the conversation was attached since.
        let by_identity = matches!(
            ev.kind,
            EventKind::Accepted
                | EventKind::Rejected { .. }
                | EventKind::AckLost
                | EventKind::StatusResolved { .. }
                | EventKind::Completed
                | EventKind::Failed { .. }
                | EventKind::Cancelled
                | EventKind::QueueAdmitted { .. }
                | EventKind::QueueRefused { .. }
                | EventKind::QueueAckLost { .. }
                | EventKind::QueueCancelled { .. }
                | EventKind::ToolOutputFull { .. }
                | EventKind::ToolOutputUnavailable { .. }
                | EventKind::UiRespondRefused { .. }
        );
        if ev.generation != c.generation && !by_identity {
            return out;
        }
        // Operation-scoped events must match the live operation.
        let scoped = !matches!(
            ev.kind,
            EventKind::Opened { .. }
                | EventKind::OlderPage { .. }
                | EventKind::Synced { .. }
                | EventKind::OpenFailed { .. }
                | EventKind::ChangesSynced(_)
                | EventKind::ChangesScanState(_)
                | EventKind::ThinkingState { .. }
                | EventKind::QueueAdmitted { .. }
                | EventKind::QueueRefused { .. }
                | EventKind::QueueAckLost { .. }
                | EventKind::QueueCancelled { .. }
                | EventKind::EngineState { .. }
                | EventKind::OlderFailed { .. }
                | EventKind::HasOlder(_)
                | EventKind::ToolOutputFull { .. }
                | EventKind::ToolOutputUnavailable { .. }
                | EventKind::UiState { .. }
                | EventKind::UsageSynced(_)
                | EventKind::UiRespondRefused { .. }
                | EventKind::SubagentsSynced { .. }
                | EventKind::SubagentsUnavailable(_)
                | EventKind::SubagentView { .. }
                | EventKind::SubagentViewFailed { .. }
                | EventKind::SubagentDirectory { .. }
                | EventKind::SubagentPage { .. }
                | EventKind::SubagentOlderPage { .. }
                | EventKind::SubagentOlderFailed { .. }
        );
        if scoped && ev.op != c.run.op() {
            return out;
        }
        match ev.kind {
            EventKind::SubagentsSynced { .. } | EventKind::SubagentDirectory { .. } if c.opened => {
                let (enabled, children, paged, truncated) = match ev.kind {
                    EventKind::SubagentDirectory {
                        enabled,
                        children,
                        truncated,
                    } => (enabled, children, true, truncated),
                    EventKind::SubagentsSynced { enabled, children } => {
                        (enabled, children, false, false)
                    }
                    _ => unreachable!(),
                };
                c.subagents.enabled = Some(enabled);
                c.subagents.children = children;
                c.subagents.paged = paged;
                c.subagents.truncated = truncated;
                if c.subagents
                    .selected
                    .is_some_and(|id| !c.subagents.children.iter().any(|row| row.id == id))
                {
                    c.subagents.selected = None;
                    c.subagents.clear_activity();
                }
                c.subagents.error = None;
                if let Some(child) = c.subagents.selected
                    && !c.subagents.loading
                    && c.subagents.viewed != Some(child)
                    && c.subagents.items.is_empty()
                {
                    c.subagents.loading = true;
                    out.effects
                        .push(Effect::Backend(BackendRequest::OpenSubagent {
                            conversation: id,
                            generation: c.generation,
                            child,
                        }));
                }
                out.notes.push(Note::Other);
            }
            EventKind::SubagentsUnavailable(reason) => {
                c.subagents.enabled = None;
                c.subagents.children.clear();
                c.subagents.paged = false;
                c.subagents.truncated = false;
                c.subagents.history = 0;
                c.subagents.select(None);
                c.subagents.error = Some(reason);
                out.notes.push(Note::Other);
            }
            EventKind::SubagentView { child, items, live }
                if c.subagents.selected == Some(child) =>
            {
                c.subagents.apply_view(child, items, live);
                out.notes.push(Note::Other);
            }
            EventKind::SubagentPage {
                child,
                items,
                live,
                has_older,
            } if c.subagents.selected == Some(child) => {
                c.subagents.apply_newest(child, items, live, has_older);
                out.notes.push(Note::Other);
            }
            EventKind::SubagentOlderPage {
                child,
                before,
                items,
                has_older,
            } => {
                if c.subagents
                    .apply_older(child, before, items, has_older)
                    .is_some()
                {
                    out.notes.push(Note::Other);
                }
            }
            EventKind::SubagentOlderFailed {
                child,
                before,
                reason,
            } => {
                if c.subagents.older_failed(child, before, reason) {
                    out.notes.push(Note::Other);
                }
            }
            EventKind::SubagentViewFailed { child, reason }
                if c.subagents.selected == Some(child) =>
            {
                c.subagents.error = Some(reason);
                c.subagents.live = false;
                c.subagents.loading = false;
                out.notes.push(Note::Other);
            }
            EventKind::SubagentsSynced { .. }
            | EventKind::SubagentDirectory { .. }
            | EventKind::SubagentView { .. }
            | EventKind::SubagentPage { .. }
            | EventKind::SubagentViewFailed { .. } => {}
            EventKind::UsageSynced(usage) => {
                if c.opened {
                    c.usage = Some(usage);
                    out.notes.push(Note::Other);
                }
            }
            EventKind::Opened {
                items,
                has_older,
                changes,
            } => {
                c.items = items;
                c.import_prompt_history(true);
                // Reattachment may land on an older engine with no child service. A prior
                // session's capability or child selection must not masquerade as current.
                c.subagents = SubagentState::default();
                c.streaming_item = None;
                c.usage = None;
                c.has_older = has_older;
                c.opened = true;
                c.cached_at = None;
                c.stale_reason = None;
                // Pi attaches with an empty diff, then scans separately. Reattachment must
                // not erase this conversation's last successful result if that scan fails.
                if !real || !changes.is_empty() {
                    c.selected_change = (!changes.is_empty()).then_some(0);
                    c.changes = changes;
                    c.changes_state = ChangesState::Ready;
                    c.changes_revision += 1;
                }
                out.notes.push(Note::ItemsReset(id));
                // A submission in doubt (restored from the journal, or cut off by a reconnect)
                // is resolved by asking the engine about its key; that never sends anything.
                if let RunState::OutcomeUnknown { op } = c.run
                    && !c.status_check_pending
                {
                    c.status_check_pending = true;
                    c.status_error = None;
                    out.effects
                        .push(Effect::Backend(BackendRequest::CheckStatus {
                            conversation: id,
                            generation: c.generation,
                            op,
                            request: c.current_request.clone(),
                        }));
                }
            }
            EventKind::OpenFailed { message } if c.cached_at.is_some() => {
                // The saved copy stays, labelled with why the engine's own state is missing.
                c.stale_reason = Some(message);
                out.notes.push(Note::Other);
            }
            EventKind::OpenFailed { message } => {
                if !c.opened {
                    c.items = vec![TranscriptItem {
                        id: ItemId(LOCAL_ITEM_BASE - 1),
                        at: 0,
                        kind: ItemKind::Notice {
                            text: message,
                            level: NoticeLevel::Error,
                        },
                    }];
                    out.notes.push(Note::ItemsReset(id));
                }
            }
            EventKind::Synced { items } => {
                if c.opened {
                    c.items = items;
                    c.import_prompt_history(false);
                    c.streaming_item = None;
                    out.notes.push(Note::ItemsReset(id));
                }
            }
            EventKind::OlderPage { items, has_older } => {
                let n = items.len();
                c.loading_older = false;
                c.older_error = None;
                c.has_older = has_older;
                let mut v = items;
                v.append(&mut c.items);
                c.items = v;
                c.import_prompt_history(true);
                if let Some(s) = c.streaming_item.as_mut() {
                    *s += n;
                }
                // A tool call and its result can fall on either side of the page boundary.
                if merge_boundary_tools(&mut c.items, n) {
                    c.streaming_item = None;
                    out.notes.push(Note::ItemsReset(id));
                } else {
                    out.notes.push(Note::ItemsPrepended(id, n));
                }
            }
            EventKind::HasOlder(has) => {
                if c.opened && c.has_older != has {
                    c.has_older = has;
                    out.notes.push(Note::Other);
                }
            }
            EventKind::OlderFailed { message } => {
                c.loading_older = false;
                c.older_error = Some(message);
                out.notes.push(Note::Other);
            }
            EventKind::ToolOutputFull { call_id, text } => {
                if let Some((wanted, purpose)) = c.pending_output.clone()
                    && wanted == call_id
                {
                    c.pending_output = None;
                    let name = c
                        .items
                        .iter()
                        .find_map(|i| match &i.kind {
                            ItemKind::Tool(t) if t.call_id.as_deref() == Some(call_id.as_str()) => {
                                Some(format!("{}-output.txt", t.name))
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| "tool-output.txt".into());
                    out.effects.push(match purpose {
                        OutputUse::Copy => Effect::CopyText(text),
                        OutputUse::Save => Effect::SaveText {
                            suggested_name: name,
                            text,
                        },
                    });
                }
            }
            EventKind::ToolOutputUnavailable { call_id, reason } => {
                if c.pending_output
                    .as_ref()
                    .is_some_and(|(w, _)| *w == call_id)
                {
                    c.pending_output = None;
                    out.merge(
                        self.set_notice(format!("Could not get the complete output: {reason}")),
                    );
                }
            }
            EventKind::UiState {
                requests,
                status,
                notices,
            } => {
                c.ui_answering
                    .retain(|a| requests.iter().any(|r| &r.id == a));
                c.ui_requests = requests;
                c.ui_status = status;
                c.ui_notices = notices;
                out.notes.push(Note::Other);
            }
            EventKind::UiRespondRefused {
                id: request,
                reason,
            } => {
                c.ui_answering.retain(|a| *a != request);
                c.ui_error = Some(reason);
                out.notes.push(Note::Other);
            }
            EventKind::Accepted => {
                if let RunState::Submitting { op } = c.run {
                    c.run = RunState::Running { op };
                    mark_last_user(c, Delivery::Sent, &mut out, id);
                    journal(c, id, JournalState::Accepted, &mut out);
                }
            }
            EventKind::SteerAccepted => {
                mark_last_user(c, Delivery::Sent, &mut out, id);
            }
            EventKind::Rejected { reason } => {
                if let RunState::Submitting { .. } = c.run {
                    journal(c, id, JournalState::Rejected, &mut out);
                    mark_last_user(c, Delivery::Rejected, &mut out, id);
                    c.run = RunState::Failed {
                        message: reason.clone(),
                    };
                    // Retain the user's text: restore it to the draft if the editor is empty.
                    if c.draft.text.is_empty()
                        && let Some((t, a)) = c.last_submission.clone()
                    {
                        c.draft.text = t;
                        c.draft.attachments = a;
                        c.draft.sync_epoch += 1;
                        c.draft.rev += 1;
                        c.draft.save = SaveState::Dirty;
                    }
                    self.push_item(
                        id,
                        ItemKind::Notice {
                            text: reason,
                            level: NoticeLevel::Error,
                        },
                        &mut out,
                    );
                }
            }
            EventKind::AckLost => {
                if let RunState::Submitting { op } = c.run {
                    c.run = RunState::OutcomeUnknown { op };
                    c.recovery_notice |= real;
                    journal(c, id, JournalState::Unknown, &mut out);
                    mark_last_user(c, Delivery::Unknown, &mut out, id);
                    out.notes.push(Note::Other);
                }
            }
            EventKind::StopFailed { message } => {
                if matches!(c.run, RunState::Stopping { .. }) {
                    c.stop_error = Some(message);
                    out.notes.push(Note::Other);
                }
            }
            EventKind::StatusCheckFailed { message } => {
                c.status_check_pending = false;
                c.status_error = Some(message);
                out.notes.push(Note::Other);
            }
            EventKind::StatusResolved { accepted } => {
                c.status_check_pending = false;
                c.status_error = None;
                out.notes.push(Note::Other);
                if let RunState::OutcomeUnknown { op } = c.run {
                    if accepted {
                        c.run = RunState::Running { op };
                        journal(c, id, JournalState::Accepted, &mut out);
                        mark_last_user(c, Delivery::Sent, &mut out, id);
                    } else {
                        journal(c, id, JournalState::Rejected, &mut out);
                        mark_last_user(c, Delivery::Rejected, &mut out, id);
                        c.run = RunState::Failed {
                            message: "Pi has no record of this prompt. Inspect tool output and project Changes before deciding whether to retry.".into(),
                        };
                    }
                    out.notes.push(Note::Other);
                }
            }
            EventKind::Token(t) => match c.streaming_item {
                Some(i) => {
                    if let ItemKind::Assistant { text, .. } = &mut c.items[i].kind {
                        text.push_str(&t);
                    }
                    out.notes.push(Note::ItemChanged(id, i));
                }
                None => {
                    let idx = self.push_item(
                        id,
                        ItemKind::Assistant {
                            text: t,
                            streaming: true,
                        },
                        &mut out,
                    );
                    self.conv_mut(id).unwrap().streaming_item = idx;
                }
            },
            EventKind::ToolStarted { call, name, input } => {
                self.finish_streaming(id, &mut out);
                let op = ev.op.unwrap();
                self.push_item(
                    id,
                    ItemKind::Tool(ToolCall {
                        call_ref: Some((op, call)),
                        call_id: None,
                        name,
                        input,
                        output: String::new(),
                        truncated: false,
                        full_len: 0,
                        status: ToolStatus::Running,
                    }),
                    &mut out,
                );
            }
            EventKind::ToolOutput { call, chunk } => {
                let op = ev.op.unwrap();
                if let Some((i, t)) = find_tool(c, op, call) {
                    t.full_len += chunk.len();
                    if t.output.len() < TOOL_OUTPUT_PREVIEW_BYTES {
                        let room = TOOL_OUTPUT_PREVIEW_BYTES - t.output.len();
                        let cut = floor_boundary(&chunk, room);
                        t.output.push_str(&chunk[..cut]);
                        if cut < chunk.len() {
                            t.truncated = true;
                        }
                    } else {
                        t.truncated = true;
                    }
                    out.notes.push(Note::ItemChanged(id, i));
                }
            }
            EventKind::ToolFinished { call, ok } => {
                let op = ev.op.unwrap();
                if let Some((i, t)) = find_tool(c, op, call) {
                    t.status = if ok {
                        ToolStatus::Ok
                    } else {
                        ToolStatus::Failed
                    };
                    out.notes.push(Note::ItemChanged(id, i));
                }
            }
            EventKind::ThinkingState { level, levels } => {
                if c.opened {
                    c.thinking_level = Some(level);
                    c.thinking_levels = levels;
                    out.notes.push(Note::Other);
                }
            }
            EventKind::ChangesScanState(state) => {
                if c.opened
                    && (c.changes_state != state
                        || (state == ChangesState::NotARepository && !c.changes.is_empty()))
                {
                    if state == ChangesState::NotARepository && !c.changes.is_empty() {
                        c.changes.clear();
                        c.selected_change = None;
                        c.changes_revision += 1;
                    }
                    c.changes_state = state;
                    out.notes.push(Note::Other);
                }
            }
            EventKind::ChangesSynced(changes) => {
                if c.opened && (c.changes_state != ChangesState::Ready || c.changes != changes) {
                    c.changes_state = ChangesState::Ready;
                    if c.changes != changes {
                        c.changes_revision += 1;
                        c.selected_change = c
                            .selected_change
                            .and_then(|i| c.changes.get(i))
                            .and_then(|previous| {
                                changes.iter().position(|f| f.path == previous.path)
                            })
                            .or((!changes.is_empty()).then_some(0));
                        c.changes = changes;
                    }
                    out.notes.push(Note::Other);
                }
            }
            EventKind::ChangesReported(changes) => {
                c.changes_state = ChangesState::Ready;
                c.changes_revision += 1;
                c.selected_change = c
                    .selected_change
                    .filter(|i| *i < changes.len())
                    .or((!changes.is_empty()).then_some(0));
                c.changes = changes;
                out.notes.push(Note::Other);
            }
            EventKind::QueueAdmitted { request, entry } => {
                let pending = c.pending_queue.iter().position(|p| p.request == request);
                if let Some(i) = pending {
                    let p = c.pending_queue.remove(i);
                    if c.queue.iter().all(|q| q.id != entry) {
                        c.queue.push(QueuedPrompt {
                            id: entry,
                            text: p.text,
                            mode: p.mode,
                        });
                    }
                    out.effects.push(Effect::JournalState {
                        conversation: id,
                        request,
                        state: JournalState::Queued,
                    });
                    out.notes.push(Note::Other);
                }
            }
            EventKind::QueueRefused { request, reason } => {
                if let Some(i) = c.pending_queue.iter().position(|p| p.request == request) {
                    let p = c.pending_queue.remove(i);
                    out.effects.push(Effect::JournalState {
                        conversation: id,
                        request,
                        state: JournalState::Rejected,
                    });
                    // The text comes back to the draft if the user has not started another.
                    if c.draft.text.is_empty() && c.draft.attachments.is_empty() {
                        c.draft.text = p.text;
                        c.draft.attachments = p.attachments;
                        c.draft.sync_epoch += 1;
                        c.draft.rev += 1;
                        c.draft.save = SaveState::Dirty;
                    }
                    c.intent_error = Some(format!("Not queued: {reason}"));
                    out.notes.push(Note::Other);
                }
            }
            EventKind::QueueAckLost { request } => {
                if let Some(p) = c.pending_queue.iter_mut().find(|p| p.request == request) {
                    p.state = QueueSend::Unknown;
                    out.effects.push(Effect::JournalState {
                        conversation: id,
                        request,
                        state: JournalState::Unknown,
                    });
                    out.notes.push(Note::Other);
                }
            }
            EventKind::QueueCancelled { entry, outcome } => {
                match outcome {
                    CancelOutcome::Cancelled | CancelOutcome::NotFound => {
                        c.queue.retain(|q| q.id != entry);
                    }
                    CancelOutcome::AlreadyConsumed => {
                        c.intent_error = Some(
                            "That input had already started, so it could not be removed.".into(),
                        );
                    }
                }
                out.notes.push(Note::Other);
            }
            EventKind::EngineState { busy, queue } => {
                if real {
                    if busy {
                        c.unseen_completion = false;
                    }
                    if c.current_request.is_none() {
                        c.status_check_pending = false;
                        c.status_error = None;
                    }
                    c.queue = queue;
                    match (busy, c.run.clone()) {
                        // Work this window did not start: show it and let the engine end it.
                        (true, RunState::Idle) => {
                            let op = self.alloc_op();
                            let c = self.conv_mut(id).unwrap();
                            c.run = RunState::Running { op };
                            c.adopted = Some(op);
                        }
                        (false, RunState::Running { op } | RunState::Stopping { op })
                            if c.adopted == Some(op) =>
                        {
                            c.run = RunState::Idle;
                            c.adopted = None;
                            c.stop_error = None;
                            c.status_error = None;
                            c.status_check_pending = false;
                        }
                        _ => {}
                    }
                    out.notes.push(Note::Other);
                    if !busy
                        && self
                            .conversation(id)
                            .and_then(|c| c.goal.as_ref())
                            .is_some_and(|goal| goal.turns == 0)
                    {
                        out.merge(self.start_goal_if_idle(id));
                    }
                }
            }
            EventKind::Completed => {
                c.unseen_completion = !selected;
                c.stop_error = None;
                c.status_error = None;
                c.status_check_pending = false;
                c.run = RunState::Idle;
                journal(c, id, JournalState::Completed, &mut out);
                self.finish_streaming(id, &mut out);
                out.merge(self.after_settled(id));
                out.merge(self.advance_goal(id, ev.op));
            }
            EventKind::Failed { message } => {
                c.unseen_completion = false;
                if let Some(goal) = &mut c.goal {
                    goal.paused = Some(format!("Run failed: {message}"));
                    out.effects.push(Effect::SaveGoal {
                        conversation: id,
                        text: Some(goal.text.clone()),
                        paused: goal.paused.clone(),
                    });
                }
                c.stop_error = None;
                c.status_error = None;
                c.status_check_pending = false;
                c.run = RunState::Failed {
                    message: message.clone(),
                };
                journal(c, id, JournalState::Failed, &mut out);
                self.finish_streaming(id, &mut out);
                self.push_item(
                    id,
                    ItemKind::Notice {
                        text: message,
                        level: NoticeLevel::Error,
                    },
                    &mut out,
                );
                out.notes.push(Note::Other);
            }
            EventKind::Cancelled => {
                c.unseen_completion = false;
                if c.goal_op == ev.op
                    && let Some(goal) = &mut c.goal
                    && goal.turns > 0
                {
                    goal.paused = Some("Run stopped.".into());
                    out.effects.push(Effect::SaveGoal {
                        conversation: id,
                        text: Some(goal.text.clone()),
                        paused: goal.paused.clone(),
                    });
                }
                if matches!(
                    c.run,
                    RunState::Stopping { .. }
                        | RunState::Running { .. }
                        | RunState::Submitting { .. }
                ) {
                    c.run = RunState::Idle;
                    c.stop_error = None;
                    c.status_error = None;
                    c.status_check_pending = false;
                    journal(c, id, JournalState::Cancelled, &mut out);
                    self.finish_streaming(id, &mut out);
                    self.push_item(
                        id,
                        ItemKind::Notice {
                            text: "Run stopped.".into(),
                            level: NoticeLevel::Info,
                        },
                        &mut out,
                    );
                    // Real-mode abort withdraws engine-owned inputs; its replicated queue
                    // remains authoritative. A replacement goal starts only after settlement.
                    out.notes.push(Note::Other);
                    out.merge(self.start_goal_if_idle(id));
                }
            }
        }
        if cache && self.conversation(id).is_some_and(|c| c.opened) {
            out.effects.push(Effect::SaveCache { conversation: id });
        }
        if reached_target {
            out.merge(self.continue_to_target(id));
        }
        if opened_event
            && self
                .conversation(id)
                .and_then(|c| c.goal.as_ref())
                .is_some_and(|g| g.turns == 0)
        {
            out.merge(self.start_goal_if_idle(id));
        }
        out
    }

    fn finish_streaming(&mut self, id: ConversationId, out: &mut Outcome) {
        let c = self.conv_mut(id).unwrap();
        if let Some(i) = c.streaming_item.take() {
            if let ItemKind::Assistant { streaming, .. } = &mut c.items[i].kind {
                *streaming = false;
            }
            out.notes.push(Note::ItemChanged(id, i));
        }
    }

    /// Schedule a goal turn only when no other input is already waiting. An ambiguous model
    /// reply pauses the runner; it must not blindly issue an unbounded series of prompts.
    fn advance_goal(&mut self, id: ConversationId, settled_op: Option<OperationId>) -> Outcome {
        let Some(c) = self.conversation(id) else {
            return Outcome::default();
        };
        let Some(goal) = &c.goal else {
            return Outcome::default();
        };
        if goal.paused.is_some()
            || c.goal_op.is_some_and(|op| Some(op) != settled_op)
            || c.pending_intent.is_some()
            || !c.queue.is_empty()
            || !c.pending_queue.is_empty()
        {
            return Outcome::default();
        }
        // The last assistant message of this turn must explicitly authorize continuation.
        // A model that ignores the protocol cannot keep the runner spinning.
        if goal.turns == 0 || c.goal_op.is_none() {
            return self.start_goal_if_idle(id);
        }
        let verdict = c
            .items
            .iter()
            .rev()
            .take_while(|item| !matches!(item.kind, ItemKind::User { .. }))
            .find_map(|item| match &item.kind {
                ItemKind::Assistant { text, .. } => Some(text.trim_end().to_string()),
                _ => None,
            });
        let Some(goal) = &mut self.conv_mut(id).unwrap().goal else {
            return Outcome::default();
        };
        let marker = verdict.as_deref().and_then(|s| s.lines().last());
        let mut result = match marker {
            Some("[PIPKIN_GOAL_CONTINUE]") => self.start_goal_if_idle(id),
            Some("[PIPKIN_GOAL_MET]") => {
                goal.paused = Some("Goal met.".into());
                Self::goal_paused_outcome(id, goal)
            }
            Some("[PIPKIN_GOAL_BLOCKED]") => {
                goal.paused = Some("Blocked; input needed.".into());
                Self::goal_paused_outcome(id, goal)
            }
            Some("[PIPKIN_GOAL_UNREACHABLE]") => {
                goal.paused = Some("Goal unreachable.".into());
                Self::goal_paused_outcome(id, goal)
            }
            _ => {
                goal.paused = Some("Paused: Pi did not give a clear goal status.".into());
                Self::goal_paused_outcome(id, goal)
            }
        };
        if matches!(
            marker,
            Some(
                "[PIPKIN_GOAL_CONTINUE]"
                    | "[PIPKIN_GOAL_MET]"
                    | "[PIPKIN_GOAL_BLOCKED]"
                    | "[PIPKIN_GOAL_UNREACHABLE]"
            )
        ) {
            let c = self.conv_mut(id).unwrap();
            if let Some(i) = c
                .items
                .iter()
                .rposition(|item| matches!(item.kind, ItemKind::Assistant { .. }))
                && let ItemKind::Assistant { text, .. } = &mut c.items[i].kind
            {
                if let Some(line_start) = text.trim_end().rfind('\n') {
                    text.truncate(line_start);
                } else {
                    text.clear();
                }
                result.notes.push(Note::ItemChanged(id, i));
            }
        }
        result
    }

    /// Restore a saved goal without issuing any input. A crash or relaunch cannot prove the
    /// previous turn settled, so continuing requires an explicit /goal update.
    pub fn restore_goal(&mut self, id: ConversationId, text: String, paused: Option<String>) {
        if let Some(c) = self.conv_mut(id) {
            c.goal =
                Some(Goal {
                    text,
                    paused: Some(paused.unwrap_or_else(|| {
                        "Paused after restart; use /goal <goal> to resume.".into()
                    })),
                    turns: 0,
                });
        }
    }

    fn goal_paused_outcome(id: ConversationId, goal: &Goal) -> Outcome {
        Outcome {
            notes: vec![Note::Other],
            effects: vec![Effect::SaveGoal {
                conversation: id,
                text: Some(goal.text.clone()),
                paused: goal.paused.clone(),
            }],
        }
    }

    fn start_goal_if_idle(&mut self, id: ConversationId) -> Outcome {
        let Some(c) = self.conversation(id) else {
            return Outcome::default();
        };
        let Some(goal) = &c.goal else {
            return Outcome::default();
        };
        if goal.paused.is_some()
            || !matches!(c.run, RunState::Idle | RunState::Failed { .. })
            || c.pending_intent.is_some()
            || !c.queue.is_empty()
            || !c.pending_queue.is_empty()
            || !c.opened && self.mode == Mode::Real
            || self.read_only.is_some()
            || !self.connection.is_ready()
        {
            return Outcome::default();
        }
        let text = format!(
            "Pursue this ongoing goal: {}\n\nKeep working until it is met, blocked, or unreachable. At the end of your response, put exactly one status marker on its own final line: [PIPKIN_GOAL_CONTINUE] if there is more work you can do now; [PIPKIN_GOAL_MET] if done; [PIPKIN_GOAL_BLOCKED] if you need the user's input; [PIPKIN_GOAL_UNREACHABLE] if it cannot be done. Explain the result or blocker before that marker. Never claim work was done unless it was.",
            goal.text
        );
        self.submit(id, text, vec![], IntentOrigin::Goal)
    }

    /// After a run completes normally, start the next queued prompt. Only the demo keeps its own
    /// queue: a real engine consumes the queue it owns, and starting one of its entries from
    /// here too would run it twice.
    fn after_settled(&mut self, id: ConversationId) -> Outcome {
        if self.mode == Mode::Real {
            return Outcome::default();
        }
        let c = self.conv_mut(id).unwrap();
        if c.queue.is_empty() {
            return Outcome::default();
        }
        let next = c.queue.remove(0);
        self.submit(id, next.text, vec![], IntentOrigin::Queue)
    }

    // ------------------------------------------------------------- persistence

    pub fn draft_saved(&mut self, id: ConversationId, rev: u64, result: Result<(), String>) {
        if let Some(c) = self.conv_mut(id) {
            match result {
                Ok(()) if c.draft.rev == rev => c.draft.save = SaveState::Saved,
                Ok(()) => c.draft.save = SaveState::Dirty,
                Err(e) => c.draft.save = SaveState::Failed(e),
            }
        }
    }

    /// Restore a draft (text and attachments) read from storage at startup.
    pub fn restore_draft(
        &mut self,
        id: ConversationId,
        text: String,
        attachments: Vec<Attachment>,
    ) {
        if let Some(c) = self.conv_mut(id)
            && (!text.is_empty() || !attachments.is_empty())
            && c.draft.text.is_empty()
            && c.draft.attachments.is_empty()
        {
            c.draft.text = text;
            c.draft.attachments = attachments;
            c.draft.sync_epoch += 1;
            c.draft.save = SaveState::Saved;
        }
    }

    /// Restore a journaled request that never reached a terminal state before the last exit.
    /// Whether the engine accepted it is unknown, so the conversation enters `OutcomeUnknown`
    /// and the user decides (check status, then retry). Nothing is resent here. If the
    /// conversation already has an older unresolved request, that one is superseded.
    pub fn restore_unresolved(
        &mut self,
        id: ConversationId,
        request: RequestId,
        text: String,
        attachments: Vec<Attachment>,
    ) -> Outcome {
        let mut out = Outcome::default();
        if self.conv_mut(id).is_none() {
            return out;
        }
        let op = self.alloc_op();
        let c = self.conv_mut(id).unwrap();
        if let Some(previous) = c.current_request.replace(request) {
            out.effects.push(Effect::JournalState {
                conversation: id,
                request: previous,
                state: JournalState::Superseded,
            });
        }
        c.run = RunState::OutcomeUnknown { op };
        c.recovery_notice = true;
        c.last_submission = Some((text, attachments));
        out.notes.push(Note::Other);
        out
    }

    pub fn restore_conversation(
        &mut self,
        id: ConversationId,
        project: ProjectId,
        title: String,
        at: i64,
    ) {
        if let Some(c) = self.conv_mut(id) {
            c.title = title;
            c.updated_at = at;
        } else {
            let mut c = ConversationState::new(id, project, title, at);
            c.opened = true;
            self.conversations.push(c);
            self.next_conversation = self.next_conversation.max(id.0 + 1);
        }
    }
}

/// A page of older history can end on a tool call whose result is the first thing already
/// shown (or the other way round). Join the two so one call is one item. Returns whether
/// anything was joined.
fn merge_boundary_tools(items: &mut Vec<TranscriptItem>, new: usize) -> bool {
    let mut merged = false;
    let mut i = 0;
    while i < new.min(items.len()) {
        let call = match &items[i].kind {
            ItemKind::Tool(t) if t.status == ToolStatus::Running && t.output.is_empty() => {
                t.call_id.clone()
            }
            _ => None,
        };
        if let Some(call_id) = call {
            let found = (new..items.len().min(new + 8)).find(|j| {
                matches!(&items[*j].kind, ItemKind::Tool(t)
                    if t.call_id.as_deref() == Some(call_id.as_str()) && t.status != ToolStatus::Running)
            });
            if let Some(j) = found {
                let ItemKind::Tool(done) = items.remove(j).kind else {
                    unreachable!()
                };
                if let ItemKind::Tool(t) = &mut items[i].kind {
                    t.output = done.output;
                    t.truncated = done.truncated;
                    t.full_len = done.full_len;
                    t.status = done.status;
                    if t.input.is_empty() {
                        t.input = done.input;
                    }
                }
                merged = true;
            }
        }
        i += 1;
    }
    merged
}

/// Record a journal transition for the conversation's current request. Terminal states end
/// the association, so a later event can never rewrite a settled request.
fn journal(c: &mut ConversationState, id: ConversationId, state: JournalState, out: &mut Outcome) {
    let Some(request) = c.current_request.clone() else {
        return;
    };
    if state.is_terminal() {
        c.current_request = None;
    }
    out.effects.push(Effect::JournalState {
        conversation: id,
        request,
        state,
    });
}

fn mark_last_user(c: &mut ConversationState, d: Delivery, out: &mut Outcome, id: ConversationId) {
    if let Some((i, item)) = c
        .items
        .iter_mut()
        .enumerate()
        .rev()
        .find(|(_, it)| matches!(it.kind, ItemKind::User { .. }))
    {
        if let ItemKind::User { delivery, .. } = &mut item.kind {
            *delivery = d;
        }
        out.notes.push(Note::ItemChanged(id, i));
    }
}

fn find_tool(
    c: &mut ConversationState,
    op: OperationId,
    call: u32,
) -> Option<(usize, &mut ToolCall)> {
    c.items
        .iter_mut()
        .enumerate()
        .rev()
        .find_map(|(i, it)| match &mut it.kind {
            ItemKind::Tool(t) if t.call_ref == Some((op, call)) => Some((i, t)),
            _ => None,
        })
}

/// Backend projects plus the folders the user opened, without duplicates (by id).
fn merge_projects(mut from_backend: Vec<Project>, bookmarks: &[Project]) -> Vec<Project> {
    for bookmark in bookmarks {
        if from_backend.iter().all(|p| p.id != bookmark.id) {
            from_backend.push(bookmark.clone());
        }
    }
    from_backend
}

/// Bound a tool output to the preview size, never splitting a character. Returns the preview
/// and whether anything was cut.
pub fn preview_output(text: &str) -> (String, bool) {
    if text.len() <= TOOL_OUTPUT_PREVIEW_BYTES {
        return (text.to_owned(), false);
    }
    let cut = floor_boundary(text, TOOL_OUTPUT_PREVIEW_BYTES);
    (text[..cut].to_owned(), true)
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

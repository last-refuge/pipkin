//! The open session as the engine last described it: a snapshot plus the committed changes
//! published since. Pure; the adapter decides what to emit from what changed.

use std::collections::HashMap;

use pipkin_engine_protocol::{
    Block, Delta, Entry, EntryId, EntryKind, Message, QueueEntryView, RunView, SessionChange,
    SessionSnapshot, SubagentsView, ThinkingView, UiView, UsageView,
};
use serde_json::{Map, Value};

/// What applying one change touched.
#[derive(Clone, Debug, PartialEq)]
pub enum Touched {
    Nothing,
    /// The history or a running tool's output: the transcript needs remapping.
    Transcript,
    /// The run or queue: the engine state needs reporting.
    Engine,
    Usage,
    Thinking,
    Ui,
    Subagents,
    Plan,
    Goal,
    Jobs,
    Context,
    WebSources,
    Child(u64),
    HasOlder(bool),
    Model(Option<String>),
    /// A delta for an entry this mirror does not hold: re-read the session.
    Lost,
}

#[derive(Clone, Debug)]
pub struct Mirror {
    pub native_history_start: u64,
    pub entries: Vec<Entry>,
    pub has_older: bool,
    pub run: Option<RunView>,
    pub queue: Vec<QueueEntryView>,
    pub usage: UsageView,
    pub thinking: ThinkingView,
    pub ui: UiView,
    pub subagents: Option<SubagentsView>,
    pub plan: pipkin_engine_protocol::PlanView,
    pub goal: Option<pipkin_engine_protocol::GoalView>,
    pub jobs: pipkin_engine_protocol::JobsView,
    pub context: pipkin_engine_protocol::ContextUsageView,
    pub web_sources: Vec<pipkin_engine_protocol::WebSourceView>,
    /// Streamed output of running tool calls, by call id.
    pub progress: HashMap<String, String>,
    /// The snapshot's revision. Committed changes at or below it are already in the snapshot.
    floor: u64,
}

impl Mirror {
    pub fn new(snapshot: SessionSnapshot) -> Mirror {
        let mut entries = snapshot.entries;
        entries.sort_by_key(|e| e.id);
        Mirror {
            native_history_start: snapshot.native_history_start,
            entries,
            has_older: snapshot.has_older,
            run: snapshot.run,
            queue: snapshot.queue,
            usage: snapshot.usage,
            thinking: snapshot.thinking,
            ui: snapshot.ui,
            subagents: snapshot.subagents,
            plan: snapshot.plan,
            goal: snapshot.goal,
            jobs: snapshot.jobs,
            context: snapshot.context,
            web_sources: snapshot.web_sources,
            progress: HashMap::new(),
            floor: snapshot.revision,
        }
    }

    pub fn busy(&self) -> bool {
        self.run.is_some()
    }

    /// Insert or replace entries by id, keeping id order.
    pub fn upsert(&mut self, entry: Entry) {
        match self.entries.binary_search_by_key(&entry.id, |e| e.id) {
            Ok(i) => self.entries[i] = entry,
            Err(i) => self.entries.insert(i, entry),
        }
    }

    /// Older history paged in by the person, so later updates keep showing it.
    pub fn prepend(&mut self, entries: Vec<Entry>, has_older: bool) {
        for entry in entries {
            if self
                .entries
                .binary_search_by_key(&entry.id, |e| e.id)
                .is_err()
            {
                self.upsert(entry);
            }
        }
        self.has_older = has_older;
    }

    pub fn oldest(&self) -> Option<EntryId> {
        self.entries.first().map(|e| e.id)
    }

    /// Apply one change published at `revision`. Volatile changes carry revision 0 and always
    /// apply; committed ones already reflected in the snapshot are skipped.
    pub fn apply(&mut self, revision: u64, change: SessionChange) -> Touched {
        if revision != 0 && revision <= self.floor {
            return Touched::Nothing;
        }
        match change {
            SessionChange::EntryAppended { entry } => {
                self.settle_progress(&entry);
                self.upsert(entry);
                Touched::Transcript
            }
            SessionChange::EntryReplaced { entry } => {
                self.settle_progress(&entry);
                let known = self
                    .entries
                    .binary_search_by_key(&entry.id, |e| e.id)
                    .is_ok();
                // A replacement of something older than the window is not shown here.
                if known || self.oldest().is_none_or(|oldest| entry.id > oldest) {
                    self.upsert(entry);
                    Touched::Transcript
                } else {
                    Touched::Nothing
                }
            }
            SessionChange::EntryDelta { id, delta } => {
                match self.entries.binary_search_by_key(&id, |e| e.id) {
                    Ok(i) => {
                        apply_delta(&mut self.entries[i], delta);
                        Touched::Transcript
                    }
                    Err(_) => Touched::Lost,
                }
            }
            SessionChange::ToolProgress { call_id, text } => {
                self.progress.insert(call_id, text);
                Touched::Transcript
            }
            SessionChange::RunChanged { run } => {
                if run.is_none() {
                    self.progress.clear();
                }
                self.run = run;
                Touched::Engine
            }
            SessionChange::RunSettled { .. } | SessionChange::QueueAdmitted { .. } => {
                Touched::Nothing
            }
            SessionChange::QueueChanged { queue } => {
                self.queue = queue;
                Touched::Engine
            }
            SessionChange::UsageChanged { usage } => {
                self.usage = usage;
                Touched::Usage
            }
            SessionChange::ModelChanged { model } => Touched::Model(model),
            SessionChange::ThinkingChanged { thinking } => {
                self.thinking = thinking;
                Touched::Thinking
            }
            SessionChange::UiChanged { ui } => {
                self.ui = ui;
                Touched::Ui
            }
            SessionChange::SubagentsChanged { subagents } => {
                self.subagents = Some(subagents);
                Touched::Subagents
            }
            SessionChange::ChildChanged { child } => Touched::Child(child),
            SessionChange::BoundaryMoved { has_older } => {
                self.has_older = has_older;
                Touched::HasOlder(has_older)
            }
            SessionChange::PlanChanged { plan } => {
                self.plan = plan;
                Touched::Plan
            }
            SessionChange::GoalChanged { goal } => {
                self.goal = goal.map(|g| *g);
                Touched::Goal
            }
            SessionChange::JobsChanged { jobs } => {
                self.jobs = jobs;
                Touched::Jobs
            }
            SessionChange::ContextChanged { context } => {
                self.context = context;
                Touched::Context
            }
            SessionChange::WebSourcesChanged { sources } => {
                self.web_sources = sources;
                Touched::WebSources
            }
            SessionChange::SummaryChanged { .. } => Touched::Nothing,
        }
    }

    /// A tool result supersedes the call's streamed progress.
    fn settle_progress(&mut self, entry: &Entry) {
        if let EntryKind::Message {
            message: Message::ToolResult { tool_call_id, .. },
            ..
        } = &entry.kind
        {
            self.progress.remove(tool_call_id);
        }
    }
}

fn apply_delta(entry: &mut Entry, delta: Delta) {
    let EntryKind::Message {
        message: Message::Assistant { content, .. },
        ..
    } = &mut entry.kind
    else {
        return;
    };
    match delta {
        Delta::Text { block, text } => {
            let block = block as usize;
            match content.get_mut(block) {
                Some(Block::Text { text: t, .. }) => t.push_str(&text),
                Some(_) => {}
                None => content.push(Block::text(text)),
            }
        }
        Delta::Thinking { block, text } => {
            let block = block as usize;
            match content.get_mut(block) {
                Some(Block::Thinking { thinking, .. }) => thinking.push_str(&text),
                Some(_) => {}
                None => content.push(Block::Thinking {
                    thinking: text,
                    signature: None,
                    redacted: false,
                    extra: Map::new(),
                }),
            }
        }
        Delta::ToolCall { block, id, name } => {
            if content.get(block as usize).is_none() {
                content.push(Block::tool_call(id, name, Value::Object(Map::new())));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{InputMode, RunPhase, StopReason};

    use super::*;

    fn snapshot(revision: u64, entries: Vec<Entry>) -> SessionSnapshot {
        SessionSnapshot {
            native_history_start: 1,
            summary: pipkin_engine_protocol::SessionSummary {
                id: "s".into(),
                cwd: "/w".into(),
                title: None,
                created_at_ms: 0,
                updated_at_ms: 0,
                origin: Default::default(),
                model: None,
                blocked: None,
            },
            revision,
            entries,
            has_older: false,
            run: None,
            queue: vec![],
            usage: Default::default(),
            thinking: Default::default(),
            ui: Default::default(),
            subagents: None,
            plan: pipkin_engine_protocol::PlanView::default(),
            goal: None,
            jobs: Default::default(),
            context: Default::default(),
            web_sources: vec![],
        }
    }

    fn entry(id: u64, message: Message, partial: bool) -> Entry {
        Entry {
            id,
            at_ms: 0,
            run: Some(1),
            retired: false,
            kind: EntryKind::Message {
                message,
                mode: InputMode::Prompt,
                request: None,
                partial,
            },
        }
    }

    fn assistant(content: Vec<Block>) -> Message {
        Message::Assistant {
            content,
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            extra: Map::new(),
        }
    }

    fn text_of(m: &Mirror, id: u64) -> String {
        let e = m.entries.iter().find(|e| e.id == id).unwrap();
        match &e.kind {
            EntryKind::Message { message, .. } => message.text(),
            _ => String::new(),
        }
    }

    #[test]
    fn deltas_stream_into_the_partial_entry_and_the_final_form_replaces_it() {
        let mut m = Mirror::new(snapshot(
            5,
            vec![entry(1, Message::user_text("hi", 0), false)],
        ));
        assert_eq!(
            m.apply(
                6,
                SessionChange::EntryAppended {
                    entry: entry(2, assistant(vec![]), true)
                }
            ),
            Touched::Transcript
        );
        for (rev, piece) in [(7, "Hel"), (8, "lo")] {
            m.apply(
                rev,
                SessionChange::EntryDelta {
                    id: 2,
                    delta: Delta::Text {
                        block: 0,
                        text: piece.into(),
                    },
                },
            );
        }
        m.apply(
            9,
            SessionChange::EntryDelta {
                id: 2,
                delta: Delta::Thinking {
                    block: 1,
                    text: "hm".into(),
                },
            },
        );
        m.apply(
            9,
            SessionChange::EntryDelta {
                id: 2,
                delta: Delta::ToolCall {
                    block: 2,
                    id: "c".into(),
                    name: "bash".into(),
                },
            },
        );
        assert_eq!(text_of(&m, 2), "Hello");
        let EntryKind::Message { message, .. } = &m.entries[1].kind else {
            panic!()
        };
        assert_eq!(message.content().len(), 3);
        m.apply(
            10,
            SessionChange::EntryReplaced {
                entry: entry(2, assistant(vec![Block::text("Hello there")]), false),
            },
        );
        assert_eq!(text_of(&m, 2), "Hello there");
        assert_eq!(
            m.apply(
                11,
                SessionChange::EntryDelta {
                    id: 99,
                    delta: Delta::Text {
                        block: 0,
                        text: "x".into()
                    }
                }
            ),
            Touched::Lost
        );
    }

    #[test]
    fn changes_already_in_the_snapshot_are_skipped_but_volatile_progress_applies() {
        let mut m = Mirror::new(snapshot(
            5,
            vec![entry(1, Message::user_text("hi", 0), false)],
        ));
        assert_eq!(
            m.apply(
                5,
                SessionChange::EntryAppended {
                    entry: entry(1, Message::user_text("dup", 0), false)
                }
            ),
            Touched::Nothing
        );
        assert_eq!(text_of(&m, 1), "hi");
        assert_eq!(
            m.apply(
                0,
                SessionChange::ToolProgress {
                    call_id: "c".into(),
                    text: "50%".into()
                }
            ),
            Touched::Transcript
        );
        assert_eq!(m.progress["c"], "50%");
        m.apply(
            6,
            SessionChange::EntryAppended {
                entry: entry(
                    3,
                    Message::ToolResult {
                        tool_call_id: "c".into(),
                        tool_name: "bash".into(),
                        content: vec![],
                        is_error: false,
                        timestamp: 0,
                        extra: Map::new(),
                    },
                    false,
                ),
            },
        );
        assert!(m.progress.is_empty(), "the result supersedes progress");
    }

    #[test]
    fn run_queue_and_settings_are_tracked() {
        let mut m = Mirror::new(snapshot(1, vec![]));
        assert!(!m.busy());
        m.apply(
            0,
            SessionChange::ToolProgress {
                call_id: "c".into(),
                text: "x".into(),
            },
        );
        assert_eq!(
            m.apply(
                2,
                SessionChange::RunChanged {
                    run: Some(RunView {
                        id: 1,
                        request: Some("r".into()),
                        phase: RunPhase::Running,
                        started_at_ms: 0
                    })
                }
            ),
            Touched::Engine
        );
        assert!(m.busy());
        m.apply(3, SessionChange::RunChanged { run: None });
        assert!(!m.busy() && m.progress.is_empty());
        assert_eq!(
            m.apply(4, SessionChange::BoundaryMoved { has_older: true }),
            Touched::HasOlder(true)
        );
        assert_eq!(
            m.apply(
                5,
                SessionChange::ModelChanged {
                    model: Some("a/b".into())
                }
            ),
            Touched::Model(Some("a/b".into()))
        );
        assert_eq!(
            m.apply(6, SessionChange::ChildChanged { child: 4 }),
            Touched::Child(4)
        );
    }

    #[test]
    fn older_pages_are_kept_in_order_and_old_replacements_outside_the_window_are_ignored() {
        let mut m = Mirror::new(snapshot(
            1,
            vec![entry(10, Message::user_text("new", 0), false)],
        ));
        assert_eq!(
            m.apply(
                2,
                SessionChange::EntryReplaced {
                    entry: entry(3, Message::user_text("old", 0), false)
                }
            ),
            Touched::Nothing
        );
        m.prepend(
            vec![
                entry(4, Message::user_text("b", 0), false),
                entry(2, Message::user_text("a", 0), false),
            ],
            true,
        );
        let ids: Vec<u64> = m.entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, [2, 4, 10]);
        assert!(m.has_older);
    }
}

//! `DemoBackend`: a deterministic simulated Pi engine.
//!
//! One worker thread owns all simulated runs. Scripted events are emitted with real sleeps
//! scaled by `speed` (0.0 means never sleep). Between steps the worker polls its request
//! queue, so Cancel, Steer and CheckStatus act on the live script rather than a replay.
//! The clock is never read for content: ids, timestamps and text are functions of the seed.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipkin_core::*;

use super::fixtures::{DEFAULT_BASE_TIME, Fixtures};
use super::rng::Rng;
use super::script::{self, Action, Step, Subst};

/// How long a cancelled run takes to settle before `Cancelled` is emitted (at speed 1.0).
pub const SETTLE_MS: u64 = 350;

#[derive(Clone, Debug)]
pub struct DemoOptions {
    /// 1.0 is real time; 0.0 never sleeps (tests).
    pub speed: f32,
    pub seed: u64,
    /// Scenario that answers the next Submit.
    pub scenario: String,
    /// Fixed "now" for fixture timestamps. Never the wall clock unless the caller passes it.
    pub base_time: i64,
    /// At speed 0, `await_steer` steps wait for a Steer (tests) instead of passing through.
    pub hold_steer: bool,
}

impl Default for DemoOptions {
    fn default() -> Self {
        DemoOptions {
            speed: 1.0,
            seed: 0x5EED,
            scenario: "normal".into(),
            base_time: DEFAULT_BASE_TIME,
            hold_steer: false,
        }
    }
}

/// One Submit as the backend saw it.
#[derive(Clone, Debug, PartialEq)]
pub struct SubmitRecord {
    pub conversation: ConversationId,
    pub op: OperationId,
    pub text: String,
}

struct Shared {
    scenario: Mutex<String>,
    submit_count: AtomicUsize,
    submissions: Mutex<Vec<SubmitRecord>>,
}

pub struct DemoBackend {
    requests: mpsc::Sender<BackendRequest>,
    shared: Arc<Shared>,
    fixtures: Fixtures,
}

impl DemoBackend {
    pub fn new(events: impl Into<crate::event_queue::EventSender>, options: DemoOptions) -> Self {
        let events = events.into();
        let mut scenario = options.scenario.clone();
        if !script::scenario_names().contains(&scenario.as_str()) {
            log::warn!("unknown demo scenario {scenario:?}; using normal");
            scenario = "normal".into();
        }
        let shared = Arc::new(Shared {
            scenario: Mutex::new(scenario),
            submit_count: AtomicUsize::new(0),
            submissions: Mutex::new(Vec::new()),
        });
        let (requests, inbox) = mpsc::channel();
        let worker = Worker {
            ctx: Ctx {
                events,
                speed: options.speed.max(0.0),
                hold_steer: options.hold_steer,
                closed: Cell::new(false),
            },
            inbox,
            shared: shared.clone(),
            fixtures: Fixtures::new(options.seed, options.base_time),
            seed: options.seed,
            runs: Vec::new(),
            attempts: HashMap::new(),
            seen_ops: HashSet::new(),
            scripts: HashMap::new(),
            subagent_sessions: HashMap::new(),
            demo_plans: HashMap::new(),
        };
        std::thread::Builder::new()
            .name("pi-demo-backend".into())
            .spawn(move || worker.run())
            .expect("spawn demo backend thread");
        DemoBackend {
            requests,
            shared,
            fixtures: Fixtures::new(options.seed, options.base_time),
        }
    }

    pub fn bootstrap(&self) -> Bootstrap {
        let mut bootstrap = self.fixtures.bootstrap();
        if self.scenario() == "subagents" {
            for (id, _, title, _) in &mut bootstrap.conversations {
                if *id == super::fixtures::STORY_CONVERSATION {
                    *title = "Demo subagents · simulated counters".into();
                }
            }
        }
        bootstrap
    }

    /// Scenario that will answer the next Submit.
    pub fn scenario(&self) -> String {
        self.shared.scenario.lock().unwrap().clone()
    }

    /// Returns false (and changes nothing) for an unknown scenario name.
    pub fn set_scenario(&self, name: &str) -> bool {
        if script::scenario_names().contains(&name) {
            *self.shared.scenario.lock().unwrap() = name.to_string();
            true
        } else {
            false
        }
    }

    /// Number of Submit requests the backend has received. A repeated Submit is counted, never
    /// folded into the original.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn submit_count(&self) -> usize {
        self.shared.submit_count.load(Ordering::SeqCst)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn submissions(&self) -> Vec<SubmitRecord> {
        self.shared.submissions.lock().unwrap().clone()
    }
}

impl Backend for DemoBackend {
    fn start(&self, sink: LifecycleSink) {
        sink(LifecycleEvent::Catalog(self.bootstrap()));
        sink(LifecycleEvent::Connection(Connection::Ready));
    }

    fn request(&self, request: BackendRequest) {
        // The worker only goes away when the backend is dropped.
        let _ = self.requests.send(request);
    }
}

// ------------------------------------------------------------------------------ worker

#[derive(Clone, Copy, PartialEq)]
enum Block {
    None,
    /// Waiting for a steer, optionally until a deadline.
    Steer(Option<Instant>),
    /// Waiting for CheckStatus.
    Status,
    /// Cancelled; waiting out the settle delay.
    Settling,
}

struct Run {
    conversation: ConversationId,
    generation: u64,
    op: OperationId,
    steps: Vec<Step>,
    next_step: usize,
    queue: VecDeque<Action>,
    due: Instant,
    block: Block,
    steered: bool,
    /// A steer arrived while the script was not waiting; the next `await_steer` consumes it.
    pending_steer: bool,
    subst: Subst,
    rng: Rng,
    ended: bool,
}

struct Ctx {
    events: crate::event_queue::EventSender,
    speed: f32,
    hold_steer: bool,
    closed: Cell<bool>,
}

impl Ctx {
    fn scale(&self, ms: u64) -> Duration {
        if self.speed <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(ms as f64 / 1000.0 * self.speed as f64)
        }
    }

    fn emit(
        &self,
        conversation: ConversationId,
        generation: u64,
        op: Option<OperationId>,
        kind: EventKind,
    ) {
        let event = BackendEvent {
            conversation,
            generation,
            op,
            kind,
        };
        if self.events.send_blocking(event).is_err() {
            self.closed.set(true);
        }
    }

    fn runnable(&self, run: &Run, now: Instant) -> bool {
        match run.block {
            Block::None | Block::Settling => self.speed <= 0.0 || now >= run.due,
            Block::Steer(deadline) => deadline.is_some_and(|d| now >= d),
            Block::Status => false,
        }
    }

    fn wake_at(&self, run: &Run) -> Option<Instant> {
        match run.block {
            Block::None | Block::Settling => Some(run.due),
            Block::Steer(deadline) => deadline,
            Block::Status => None,
        }
    }

    /// Advance one run by one action.
    fn step(&self, run: &mut Run) {
        match run.block {
            Block::Settling => {
                self.emit(
                    run.conversation,
                    run.generation,
                    Some(run.op),
                    EventKind::Cancelled,
                );
                run.ended = true;
            }
            Block::Steer(_) => run.block = Block::None,
            Block::Status => {}
            Block::None => {
                let action = loop {
                    if let Some(a) = run.queue.pop_front() {
                        break Some(a);
                    }
                    let Some(step) = run.steps.get(run.next_step).cloned() else {
                        break None;
                    };
                    run.next_step += 1;
                    script::expand(&step, run.steered, &run.subst, &mut run.rng, &mut run.queue);
                };
                match action {
                    None => run.ended = true,
                    Some(Action::Sleep(ms)) => run.due = Instant::now() + self.scale(ms),
                    Some(Action::Emit(kind)) => {
                        let mut kind = *kind;
                        if let EventKind::JobsSynced { jobs } = &mut kind {
                            // Scripted job times are relative to the step; the demo never
                            // runs anything, so the clock is the only thing that moves.
                            super::script::stamp_jobs(jobs, unix_ms());
                        }
                        let terminal = matches!(
                            kind,
                            EventKind::Completed
                                | EventKind::Failed { .. }
                                | EventKind::Rejected { .. }
                        );
                        self.emit(run.conversation, run.generation, Some(run.op), kind);
                        run.ended |= terminal;
                    }
                    Some(Action::AwaitSteer(timeout)) => {
                        if run.pending_steer {
                            run.pending_steer = false;
                        } else if self.speed > 0.0 {
                            let deadline = timeout.map(|ms| Instant::now() + self.scale(ms));
                            run.block = Block::Steer(deadline);
                        } else if self.hold_steer {
                            run.block = Block::Steer(None);
                        }
                    }
                    Some(Action::AwaitStatus) => run.block = Block::Status,
                }
            }
        }
    }
}

struct Worker {
    ctx: Ctx,
    inbox: mpsc::Receiver<BackendRequest>,
    shared: Arc<Shared>,
    fixtures: Fixtures,
    seed: u64,
    runs: Vec<Run>,
    /// Submits seen per conversation; selects the variant for `failure`.
    attempts: HashMap<ConversationId, usize>,
    seen_ops: HashSet<(ConversationId, OperationId)>,
    scripts: HashMap<String, Vec<Vec<Step>>>,
    /// Only explicitly opened subagent fixtures may answer child requests. The generation
    /// is the attachment identity, not a real-mode or engine-evidence override.
    subagent_sessions: HashMap<ConversationId, (u64, bool)>,
    /// The simulated plan of an opened subagent fixture; edits change only this copy.
    demo_plans: HashMap<ConversationId, (u64, Vec<PlanTask>)>,
}

impl Worker {
    fn run(mut self) {
        loop {
            loop {
                match self.inbox.try_recv() {
                    Ok(request) => self.handle(request),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            let now = Instant::now();
            let mut progressed = false;
            for run in &mut self.runs {
                if self.ctx.runnable(run, now) {
                    self.ctx.step(run);
                    progressed = true;
                }
            }
            self.runs.retain(|r| !r.ended);
            if self.ctx.closed.get() {
                return;
            }
            if progressed {
                continue;
            }
            let wake = self.runs.iter().filter_map(|r| self.ctx.wake_at(r)).min();
            let request = match wake {
                Some(at) => match self
                    .inbox
                    .recv_timeout(at.saturating_duration_since(Instant::now()))
                {
                    Ok(r) => Some(r),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match self.inbox.recv() {
                    Ok(r) => Some(r),
                    Err(_) => return,
                },
            };
            if let Some(request) = request {
                self.handle(request);
            }
        }
    }

    fn variants(&mut self, scenario: &str) -> &Vec<Vec<Step>> {
        self.scripts.entry(scenario.to_string()).or_insert_with(|| {
            script::load(scenario)
                .or_else(|e| {
                    log::error!("scenario {scenario}: {e}");
                    script::load("normal")
                })
                .unwrap_or_else(|e| {
                    log::error!("builtin scenario failed to load: {e}");
                    vec![vec![]]
                })
        })
    }

    fn find_run(&mut self, conversation: ConversationId, op: OperationId) -> Option<&mut Run> {
        self.runs
            .iter_mut()
            .find(|r| r.conversation == conversation && r.op == op && !r.ended)
    }

    fn handle(&mut self, request: BackendRequest) {
        match request {
            BackendRequest::CliApproval(request) => {
                let (conversation, generation, op) = request.tuple();
                self.ctx.emit(conversation, generation, Some(op), EventKind::CliApproval(Box::new(request.failed())));
            },
            BackendRequest::RetireLocalCliInspect { .. } => {},
            BackendRequest::LocalCli { conversation, generation, op, request } => {
                self.ctx.emit(conversation, generation, Some(op), EventKind::LocalCli(LocalCliEvent::Failed {
                    phase: request.phase(),
                    failure: LocalCliFailure::Rejected("Local CLI setup requires the native engine; this is a simulated backend.".into()),
                }));
            }
            BackendRequest::Open {
                conversation,
                generation,
            } => {
                let simulated = *self.shared.scenario.lock().unwrap() == "subagents"
                    && conversation == super::fixtures::STORY_CONVERSATION;
                self.subagent_sessions.remove(&conversation);
                let (items, has_older, changes) = if simulated {
                    self.subagent_sessions
                        .insert(conversation, (generation, true));
                    (demo_parent_items(self.fixtures.base), false, Vec::new())
                } else {
                    self.fixtures.open(conversation)
                };
                self.ctx.emit(
                    conversation,
                    generation,
                    None,
                    EventKind::Opened {
                        items,
                        has_older,
                        changes,
                    },
                );
                if simulated {
                    self.ctx.emit(
                        conversation,
                        generation,
                        None,
                        EventKind::SubagentsSynced {
                            enabled: true,
                            children: demo_subagent_directory(),
                        },
                    );
                    self.demo_plans.insert(conversation, (1, demo_plan_tasks()));
                    self.emit_plan(conversation, generation);
                }
            }
            BackendRequest::UpdatePlan {
                conversation,
                generation,
                request,
                task,
                status,
                reason,
                expected_revision,
            } => {
                let attached = self
                    .subagent_sessions
                    .get(&conversation)
                    .is_some_and(|(current, _)| *current == generation);
                let outcome = match self.demo_plans.get_mut(&conversation) {
                    Some((revision, tasks)) if attached => {
                        if *revision != expected_revision {
                            Err(format!(
                                "the plan is at revision {revision}, not {expected_revision}"
                            ))
                        } else if let Some(t) = tasks.iter_mut().find(|t| t.id == task) {
                            *revision += 1;
                            t.status = status;
                            t.reason = reason;
                            t.revision = *revision;
                            Ok(())
                        } else {
                            Err(format!("task #{task} does not exist"))
                        }
                    }
                    _ => Err("Demo plan unavailable for this attachment.".into()),
                };
                let kind = match outcome {
                    Ok(()) => {
                        self.emit_plan(conversation, generation);
                        EventKind::PlanEditApplied { request }
                    }
                    Err(reason) => EventKind::PlanEditFailed { request, reason },
                };
                self.ctx.emit(conversation, generation, None, kind);
            }
            BackendRequest::OpenSubagent {
                conversation,
                generation,
                child,
            } => {
                let attached = self
                    .subagent_sessions
                    .get(&conversation)
                    .is_some_and(|(current, _)| *current == generation);
                let kind = if attached && (1..=3).contains(&child) {
                    EventKind::SubagentView {
                        child,
                        items: demo_child_items(child, self.fixtures.base),
                        // These are frozen snapshots, not a live child subscription.
                        live: false,
                    }
                } else {
                    EventKind::SubagentViewFailed {
                        child,
                        reason: "Demo child snapshot unavailable for this attachment.".into(),
                    }
                };
                self.ctx.emit(conversation, generation, None, kind);
            }
            // The simulated agent has no per-child controls: say so, never pretend.
            BackendRequest::StopSubagent {
                conversation,
                generation,
                request,
                ..
            }
            | BackendRequest::SteerSubagent {
                conversation,
                generation,
                request,
                ..
            }
            | BackendRequest::LaunchSubagent {
                conversation,
                generation,
                request,
                ..
            } => self.ctx.emit(
                conversation,
                generation,
                None,
                EventKind::SubagentControlFailed {
                    request,
                    reason: "Demo · simulated agent: subagent controls are not available.".into(),
                },
            ),
            // Demo child views are frozen, unpaged snapshots.
            BackendRequest::LoadOlderSubagent {
                conversation,
                generation,
                child,
                before,
            } => self.ctx.emit(
                conversation,
                generation,
                None,
                EventKind::SubagentOlderFailed {
                    child,
                    before,
                    reason: "Demo child snapshots have no earlier activity.".into(),
                },
            ),
            BackendRequest::SetSubagentsEnabled {
                conversation,
                generation,
                enabled,
            } => {
                if let Some((current, setting)) = self.subagent_sessions.get_mut(&conversation)
                    && *current == generation
                {
                    *setting = enabled;
                    self.ctx.emit(
                        conversation,
                        generation,
                        None,
                        EventKind::SubagentsSynced {
                            enabled,
                            children: demo_subagent_directory(),
                        },
                    );
                }
            }
            BackendRequest::LoadOlder {
                conversation,
                generation,
                before,
            } => {
                let (items, has_older) = if self
                    .subagent_sessions
                    .get(&conversation)
                    .is_some_and(|(current, _)| *current == generation)
                {
                    (Vec::new(), false)
                } else {
                    self.fixtures.older(conversation, before)
                };
                self.ctx.emit(
                    conversation,
                    generation,
                    None,
                    EventKind::OlderPage { items, has_older },
                );
            }
            BackendRequest::Submit {
                conversation,
                generation,
                op,
                text,
                model,
                ..
            } => {
                self.shared.submit_count.fetch_add(1, Ordering::SeqCst);
                self.shared.submissions.lock().unwrap().push(SubmitRecord {
                    conversation,
                    op,
                    text: text.clone(),
                });
                self.seen_ops.insert((conversation, op));
                let attempt = {
                    let n = self.attempts.entry(conversation).or_insert(0);
                    *n += 1;
                    *n
                };
                let scenario = self.shared.scenario.lock().unwrap().clone();
                let variants = self.variants(&scenario);
                let steps = variants[(attempt - 1) % variants.len()].clone();
                self.runs.push(Run {
                    conversation,
                    generation,
                    op,
                    steps,
                    next_step: 0,
                    queue: VecDeque::new(),
                    due: Instant::now(),
                    block: Block::None,
                    steered: false,
                    pending_steer: false,
                    subst: Subst {
                        prompt: single_line(&text, 100),
                        steer: String::new(),
                        model: model.unwrap_or_else(|| "default".into()),
                    },
                    rng: Rng::from_parts(&[self.seed, conversation.0, op.0, attempt as u64]),
                    ended: false,
                });
            }
            // Unsupported real-mode requests never perform any work in the demo.
            BackendRequest::Queue { .. }
            | BackendRequest::RemoveSignIn(_)
            | BackendRequest::StartSignIn(_)
            | BackendRequest::ReuseSignIn(_)
            | BackendRequest::AnswerSignIn { .. }
            | BackendRequest::CancelSignIn { .. }
            | BackendRequest::CancelQueued { .. }
            | BackendRequest::RefreshModels { .. }
            | BackendRequest::RefreshChanges { .. }
            | BackendRequest::FetchToolOutput { .. }
            | BackendRequest::UiRespond { .. }
            | BackendRequest::UiCancel { .. }
            // The demo keeps the desktop goal runner; it never reports an engine-owned goal.
            | BackendRequest::SetGoal { .. }
            | BackendRequest::GoalControl { .. }
            | BackendRequest::MigrateGoal { .. }
            // The demo runs no background jobs; the inspector says so instead of simulating them.
            | BackendRequest::JobLog { .. }
            | BackendRequest::CancelJob { .. }
            | BackendRequest::Diagnostics { .. }
            | BackendRequest::ExportDiagnostics { .. }
            | BackendRequest::ExportConfigure(_)
            | BackendRequest::ExportSetCredential { .. }
            | BackendRequest::ExportClearCredential
            | BackendRequest::ExportPreview { .. }
            // The demo manages no MCP servers; the page says so instead of simulating one.
            | BackendRequest::McpRefresh { .. }
            | BackendRequest::McpUpsert { .. }
            | BackendRequest::McpRemove { .. }
            | BackendRequest::McpTest { .. }
            | BackendRequest::McpReconnect { .. }
            | BackendRequest::McpSetDisabledTools { .. }
            | BackendRequest::McpAuthStart { .. }
            | BackendRequest::McpAuthAnswer { .. }
            | BackendRequest::McpAuthCancel { .. }
            | BackendRequest::McpAuthRevoke { .. }
            | BackendRequest::McpImportPreview { .. }
            | BackendRequest::McpImportApply { .. } => {}
            BackendRequest::Steer {
                conversation,
                generation,
                op,
                text,
            } => {
                let accepted = match self.find_run(conversation, op) {
                    Some(run) if run.block != Block::Settling => {
                        run.steered = true;
                        run.subst.steer = single_line(&text, 100);
                        if matches!(run.block, Block::Steer(_)) {
                            run.block = Block::None;
                        } else {
                            run.pending_steer = true;
                        }
                        true
                    }
                    _ => false,
                };
                if accepted {
                    self.ctx
                        .emit(conversation, generation, Some(op), EventKind::SteerAccepted);
                }
            }
            BackendRequest::Cancel {
                conversation,
                generation,
                op,
            } => {
                let due = Instant::now() + self.ctx.scale(SETTLE_MS);
                match self.find_run(conversation, op) {
                    Some(run) => {
                        run.queue.clear();
                        run.next_step = run.steps.len();
                        run.generation = generation;
                        run.block = Block::Settling;
                        run.due = due;
                    }
                    None => self.runs.push(Run {
                        conversation,
                        generation,
                        op,
                        steps: Vec::new(),
                        next_step: 0,
                        queue: VecDeque::new(),
                        due,
                        block: Block::Settling,
                        steered: false,
                        pending_steer: false,
                        subst: Subst::default(),
                        rng: Rng::from_parts(&[self.seed, op.0]),
                        ended: false,
                    }),
                }
            }
            // The demo creates conversations and chooses models locally in the core, so these
            // are never sent to it.
            BackendRequest::CreateConversation { .. }
            | BackendRequest::SetModel { .. }
            | BackendRequest::SetThinkingLevel { .. } => {}
            BackendRequest::CheckStatus {
                conversation,
                generation,
                op,
                ..
            } => {
                let known = self.seen_ops.contains(&(conversation, op));
                if let Some(run) = self.find_run(conversation, op)
                    && run.block == Block::Status
                {
                    run.block = Block::None;
                }
                self.ctx.emit(
                    conversation,
                    generation,
                    Some(op),
                    EventKind::StatusResolved { accepted: known },
                );
            }
        }
    }
}

/// Synthetic backend-owned directory. IDs are local to each demo parent attachment;
/// no real call IDs, usage ledger, tool results or execution evidence are fabricated.
impl Worker {
    fn emit_plan(&self, conversation: ConversationId, generation: u64) {
        let Some((revision, tasks)) = self.demo_plans.get(&conversation) else {
            return;
        };
        let mut counts = PlanCounts::default();
        for t in tasks {
            match t.status {
                PlanTaskStatus::Pending => counts.pending += 1,
                PlanTaskStatus::Active => counts.active += 1,
                PlanTaskStatus::Blocked => counts.blocked += 1,
                PlanTaskStatus::Completed => counts.completed += 1,
                PlanTaskStatus::Skipped => counts.skipped += 1,
                PlanTaskStatus::Cancelled => counts.cancelled += 1,
            }
        }
        self.ctx.emit(
            conversation,
            generation,
            None,
            EventKind::PlanSynced {
                revision: *revision,
                tasks: tasks.clone(),
                counts,
                truncated: false,
            },
        );
    }
}

/// A static simulated plan; no engine applied these tasks.
fn demo_plan_tasks() -> Vec<PlanTask> {
    let task = |id: u64, title: &str, status: PlanTaskStatus, deps: &[u64]| PlanTask {
        id,
        parent: None,
        title: format!("{title} · simulated"),
        detail: "Demo · simulated agent. This task was not executed.".into(),
        status,
        reason: None,
        depends_on: deps.to_vec(),
        criteria: "Visual fixture only".into(),
        evidence: Vec::new(),
        owner: None,
        archived: false,
        stale: false,
        revision: 1,
    };
    vec![
        task(1, "Count with Counter A", PlanTaskStatus::Completed, &[]),
        task(2, "Count with Counter B", PlanTaskStatus::Active, &[1]),
        task(3, "Compare the counters", PlanTaskStatus::Blocked, &[2]),
        task(4, "Write the report", PlanTaskStatus::Pending, &[3]),
    ]
}

fn demo_subagent_directory() -> Vec<SubagentInfo> {
    [
        (1, "Counter A", "running"),
        (2, "Counter B", "done"),
        (3, "Counter C", "failed"),
    ]
    .into_iter()
    .map(|(id, label, status)| SubagentInfo {
        id,
        task_id: id,
        call_id: format!("demo-simulated-counter-{id}"),
        task: format!("{label} · simulated: count from 1 to 5"),
        status: status.into(),
        profile: None,
        ..SubagentInfo::default()
    })
    .collect()
}

fn demo_item(id: u64, at: i64, kind: ItemKind) -> TranscriptItem {
    TranscriptItem {
        native_only: false,
        id: ItemId(id),
        at,
        kind,
    }
}

fn demo_parent_items(at: i64) -> Vec<TranscriptItem> {
    vec![
        demo_item(100_001, at - 60, ItemKind::User {
            text: "Show the simulated counter task snapshots for visual testing.".into(),
            attachments: Vec::new(), delivery: Delivery::Sent, steer: false,
        }),
        demo_item(100_002, at, ItemKind::Assistant {
            text: "Demo · simulated agent\n\nVisual-only synthetic directory: Counter A running, Counter B completed, Counter C failed. Select a task to inspect its frozen child snapshot. Nothing was executed and no provider was contacted.".into(),
            streaming: false,
        }),
    ]
}

/// Small plain-text payloads through the normal child-view event contract. Core prepares
/// the bounded SubagentPreview outside rendering. Even the running row is deliberately frozen.
fn demo_child_items(child: u64, at: i64) -> Vec<TranscriptItem> {
    let (label, text) = match child {
        1 => (
            "Counter A",
            "Synthetic running snapshot: 1, 2, 3.\nPaused here for visual testing; no background counter is executing.",
        ),
        2 => (
            "Counter B",
            "Synthetic completed snapshot: 1, 2, 3, 4, 5.\nSimulated result: counted five numbers. This is not real execution evidence.",
        ),
        _ => (
            "Counter C",
            "Synthetic failed snapshot: 1, 2.\nSimulated failure: counter stopped before 3. No real process failed.",
        ),
    };
    let base = 900_000 + child * 10;
    vec![
        demo_item(
            base,
            at - 30,
            ItemKind::Notice {
                text: "Demo · simulated agent — frozen child snapshot, visual testing only.".into(),
                level: NoticeLevel::Info,
            },
        ),
        demo_item(
            base + 1,
            at - 20,
            ItemKind::User {
                text: format!("{label}: count from 1 to 5 (simulated task)."),
                attachments: Vec::new(),
                delivery: Delivery::Sent,
                steer: false,
            },
        ),
        demo_item(
            base + 2,
            at - 10,
            ItemKind::Assistant {
                text: text.into(),
                streaming: false,
            },
        ),
    ]
}

/// Collapse whitespace, neutralize backticks and cap the length, for echoing user text as
/// inert, single-line Markdown.
fn single_line(text: &str, max_chars: usize) -> String {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "'");
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        let mut s: String = collapsed.chars().take(max_chars).collect();
        s.push('\u{2026}');
        s
    }
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod unit {
    use super::*;

    fn receive(rx: &async_channel::Receiver<BackendEvent>) -> BackendEvent {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(event) = rx.try_recv() {
                return event;
            }
            assert!(Instant::now() < deadline, "demo event timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn subagents_backend() -> (DemoBackend, async_channel::Receiver<BackendEvent>) {
        let (tx, rx) = async_channel::unbounded();
        (
            DemoBackend::new(
                tx,
                DemoOptions {
                    scenario: "subagents".into(),
                    speed: 0.0,
                    ..Default::default()
                },
            ),
            rx,
        )
    }

    fn snapshot() -> String {
        let (backend, rx) = subagents_backend();
        assert_eq!(backend.scenario(), "subagents");
        assert!(backend.bootstrap().conversations[0].2.contains("simulated"));
        let conversation = super::super::fixtures::STORY_CONVERSATION;
        backend.request(BackendRequest::Open {
            conversation,
            generation: 7,
        });
        let opened = receive(&rx);
        assert_eq!(
            (opened.conversation, opened.generation, opened.op),
            (conversation, 7, None)
        );
        let EventKind::Opened {
            ref items,
            has_older,
            ref changes,
        } = opened.kind
        else {
            panic!("expected opened")
        };
        assert!(!has_older && changes.is_empty());
        assert!(format!("{items:?}").contains("Demo · simulated agent"));
        let directory = receive(&rx);
        let EventKind::SubagentsSynced {
            enabled,
            ref children,
        } = directory.kind
        else {
            panic!("expected directory")
        };
        assert!(enabled);
        assert_eq!(
            children
                .iter()
                .map(|c| c.status.as_str())
                .collect::<Vec<_>>(),
            ["running", "done", "failed"]
        );
        let plan = receive(&rx);
        let EventKind::PlanSynced {
            revision,
            ref tasks,
            counts,
            truncated,
        } = plan.kind
        else {
            panic!("expected the simulated plan")
        };
        assert_eq!(
            (revision, tasks.len(), counts.live(), truncated),
            (1, 4, 4, false)
        );
        assert!(tasks.iter().all(|t| t.title.ends_with("· simulated")));
        let mut snapshot = format!("{opened:?}\n{directory:?}\n{plan:?}");
        for child in 1..=3 {
            backend.request(BackendRequest::OpenSubagent {
                conversation,
                generation: 7,
                child,
            });
            let event = receive(&rx);
            assert_eq!(
                (event.conversation, event.generation, event.op),
                (conversation, 7, None)
            );
            let EventKind::SubagentView {
                child: returned,
                ref items,
                live,
            } = event.kind
            else {
                panic!("expected child view")
            };
            assert_eq!(returned, child);
            assert!(!live, "fixture must not claim a real live subscription");
            assert_eq!(items.len(), 3);
            for item in items {
                let text = match &item.kind {
                    ItemKind::User {
                        text, attachments, ..
                    } => {
                        assert!(attachments.is_empty());
                        text
                    }
                    ItemKind::Assistant { text, streaming } => {
                        assert!(!streaming);
                        text
                    }
                    ItemKind::Notice { text, .. } => text,
                    ItemKind::Background { .. } => panic!("no fabricated background evidence"),
                    ItemKind::Job(_) => panic!("no fabricated job evidence"),
                    ItemKind::Tool(_) => panic!("no fabricated tool evidence"),
                };
                assert!(text.len() < 1024);
                assert!(!text.contains('`'));
            }
            snapshot.push_str(&format!("\n{event:?}"));
        }
        snapshot
    }

    #[test]
    fn subagents_snapshots_are_deterministic_honest_and_bounded() {
        assert_eq!(snapshot(), snapshot());
    }

    #[test]
    fn subagents_reject_unknown_children_and_stale_attachments() {
        let (backend, rx) = subagents_backend();
        let conversation = super::super::fixtures::STORY_CONVERSATION;
        backend.request(BackendRequest::Open {
            conversation,
            generation: 9,
        });
        // Opened, the directory and the simulated plan.
        receive(&rx);
        receive(&rx);
        assert!(matches!(receive(&rx).kind, EventKind::PlanSynced { .. }));
        for (generation, child) in [(8, 1), (9, 99)] {
            backend.request(BackendRequest::OpenSubagent {
                conversation,
                generation,
                child,
            });
            let event = receive(&rx);
            assert_eq!(event.generation, generation);
            assert!(
                matches!(event.kind, EventKind::SubagentViewFailed { child: id, .. } if id == child)
            );
        }
        backend.request(BackendRequest::SetSubagentsEnabled {
            conversation,
            generation: 9,
            enabled: false,
        });
        assert!(matches!(
            receive(&rx).kind,
            EventKind::SubagentsSynced { enabled: false, .. }
        ));
        assert!(backend.set_scenario("normal"));
        backend.request(BackendRequest::Open {
            conversation,
            generation: 10,
        });
        receive(&rx);
        backend.request(BackendRequest::OpenSubagent {
            conversation,
            generation: 10,
            child: 1,
        });
        assert!(matches!(
            receive(&rx).kind,
            EventKind::SubagentViewFailed { .. }
        ));
    }

    #[test]
    fn subagents_submit_uses_visual_only_script() {
        let (backend, rx) = subagents_backend();
        backend.request(BackendRequest::Submit {
            conversation: ConversationId(1),
            generation: 4,
            op: OperationId(2),
            text: "do not execute anything".into(),
            model: None,
            // Use the existing request contract without real-mode setup.
            request: RequestId("demo-subagents-test".into()),
            attachments: Vec::new(),
        });
        let mut text = String::new();
        loop {
            let event = receive(&rx);
            assert_eq!(event.generation, 4);
            assert_eq!(event.op, Some(OperationId(2)));
            match event.kind {
                EventKind::Accepted => {}
                EventKind::Token(chunk) => text.push_str(&chunk),
                EventKind::Completed => break,
                other => panic!("unexpected execution-like event: {other:?}"),
            }
        }
        assert!(text.contains("Demo · simulated agent"));
        assert!(text.contains("No agents, tools, or paid providers were executed"));
    }

    #[test]
    fn single_line_is_inert_and_bounded() {
        assert_eq!(single_line("a\n\nb   `rm -rf`", 50), "a b 'rm -rf'");
        assert_eq!(single_line(&"x".repeat(300), 10).chars().count(), 11);
    }
}

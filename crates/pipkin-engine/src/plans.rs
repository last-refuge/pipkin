//! Plans: the task-list domain shared by ordinary work and (later) goals. Pure: no storage,
//! no I/O. [`apply`] validates a batch of [`PlanOp`]s against a [`PlanState`] and returns the
//! new state, a receipt and the events to record, or a rejection that leaves the state
//! untouched.
//!
//! Rules enforced here (the plan in `llm-docs/native-capabilities-implementation-plan.md`, §8):
//!
//! - ids are stable and never reused, even after archival;
//! - a batch is atomic: the first rejected operation rejects the whole batch;
//! - an `expected_revision` mismatch is a `conflict`, never a lost update;
//! - dependencies must exist, cannot be the task itself and cannot form a cycle; a task can
//!   be `active` or `completed` only when every dependency is completed or skipped
//!   (explicitly waived); cancelled work never satisfies a dependent;
//! - `skipped` and `cancelled` need a reason, and so does reopening finished work;
//! - reopening a task marks completed dependents `stale`, as does revising a completed
//!   task's criteria; completing a task again clears the flag;
//! - archival is separate from status: only finished tasks can be archived, and archived
//!   tasks are read-only but still satisfy dependents;
//! - several tasks may be `active` at once (parallel runs are real);
//! - text and count bounds are the protocol's `limits`.
//!
//! Rendering helpers at the bottom produce the compact text the model sees.

use std::collections::{BTreeMap, HashMap, HashSet};

use pipkin_engine_protocol::limits::*;
use pipkin_engine_protocol::{
    ErrorCode, PlanCounts, PlanOp, PlanReceipt, PlanRejected, PlanTaskView, PlanView, TaskEvidence,
    TaskRef, TaskStatus,
};
use serde_json::{Value, json};

/// A plan's complete state: every task (live and archived) in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanState {
    pub revision: u64,
    /// The next task id to allocate (ids start at 1).
    pub next_id: u64,
    pub tasks: Vec<PlanTaskView>,
}

impl Default for PlanState {
    fn default() -> Self {
        PlanState {
            revision: 0,
            next_id: 1,
            tasks: Vec::new(),
        }
    }
}

/// A recorded change to one task.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanEvent {
    pub task: u64,
    pub kind: &'static str,
    pub data: Value,
}

/// The outcome of an accepted batch.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub state: PlanState,
    pub receipt: PlanReceipt,
    pub events: Vec<PlanEvent>,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

struct Batch<'a> {
    state: PlanState,
    refs: HashMap<String, u64>,
    created: BTreeMap<String, u64>,
    changed: HashSet<u64>,
    /// Tasks whose status or dependencies an operation set: the dependency rule applies.
    rule_check: HashSet<u64>,
    events: Vec<PlanEvent>,
    now: i64,
    op: usize,
    ops: &'a [PlanOp],
}

type Step = Result<(), String>;

impl Batch<'_> {
    fn find(&self, id: u64) -> Option<usize> {
        self.state.tasks.iter().position(|t| t.id == id)
    }

    fn resolve(&self, r: &TaskRef) -> Result<u64, String> {
        match r {
            TaskRef::Id(id) => {
                if self.find(*id).is_some() {
                    Ok(*id)
                } else {
                    Err(format!("task #{id} does not exist"))
                }
            }
            TaskRef::New(name) => self
                .refs
                .get(name)
                .copied()
                .ok_or_else(|| format!("ref \"{name}\" was not created earlier in this batch")),
        }
    }

    fn resolve_live(&self, r: &TaskRef) -> Result<usize, String> {
        let id = self.resolve(r)?;
        let at = self.find(id).expect("resolved");
        if self.state.tasks[at].archived {
            return Err(format!("task #{id} is archived"));
        }
        Ok(at)
    }

    fn text(field: &str, value: &str, max: usize) -> Result<String, String> {
        let value = value.trim();
        if utf16_len(value) > max {
            return Err(format!("{field} can be at most {max} characters"));
        }
        Ok(value.to_owned())
    }

    fn title(value: &str) -> Result<String, String> {
        let title = Self::text("title", value, MAX_TASK_TITLE_CHARS)?;
        if title.is_empty() {
            return Err("title must not be empty".into());
        }
        Ok(title)
    }

    fn deps(&self, refs: &[TaskRef], this: Option<u64>) -> Result<Vec<u64>, String> {
        if refs.len() > MAX_TASK_DEPENDENCIES {
            return Err(format!(
                "a task can depend on at most {MAX_TASK_DEPENDENCIES} tasks"
            ));
        }
        let mut deps = Vec::with_capacity(refs.len());
        for r in refs {
            let id = self.resolve(r)?;
            if Some(id) == this {
                return Err(format!("task #{id} cannot depend on itself"));
            }
            if !deps.contains(&id) {
                deps.push(id);
            }
        }
        Ok(deps)
    }

    fn evidence(items: &[TaskEvidence]) -> Result<Vec<TaskEvidence>, String> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let kind = Self::text("evidence kind", &item.kind, MAX_EVIDENCE_CHARS)?;
            let reference = Self::text("evidence reference", &item.reference, MAX_EVIDENCE_CHARS)?;
            if kind.is_empty() || reference.is_empty() {
                return Err("evidence needs a kind and a reference".into());
            }
            let note = match &item.note {
                Some(note) => Some(Self::text("evidence note", note, MAX_EVIDENCE_CHARS)?)
                    .filter(|n| !n.is_empty()),
                None => None,
            };
            out.push(TaskEvidence {
                kind,
                reference,
                note,
            });
        }
        Ok(out)
    }

    fn parent_ok(&self, task: u64, parent: u64) -> Step {
        if parent == task {
            return Err(format!("task #{task} cannot be its own parent"));
        }
        let mut cursor = Some(parent);
        let mut seen = 0;
        while let Some(id) = cursor {
            if id == task {
                return Err(format!(
                    "task #{parent} cannot be the parent of #{task}: that would be a cycle"
                ));
            }
            let at = self
                .find(id)
                .ok_or_else(|| format!("task #{id} does not exist"))?;
            if self.state.tasks[at].archived {
                return Err(format!("task #{id} is archived and cannot be a parent"));
            }
            cursor = self.state.tasks[at].parent;
            seen += 1;
            if seen > self.state.tasks.len() {
                return Err("parent chain is cyclic".into());
            }
        }
        Ok(())
    }

    fn touch(&mut self, at: usize) {
        let revision = self.state.revision + 1;
        let task = &mut self.state.tasks[at];
        task.revision = revision;
        task.updated_at_ms = self.now;
        self.changed.insert(task.id);
    }

    fn create(&mut self, op: &PlanOp) -> Step {
        let PlanOp::Create {
            r#ref,
            title,
            detail,
            criteria,
            depends_on,
            parent,
            status,
            owner,
        } = op
        else {
            unreachable!()
        };
        let title = Self::title(title)?;
        let detail = Self::text(
            "detail",
            detail.as_deref().unwrap_or(""),
            MAX_TASK_DETAIL_CHARS,
        )?;
        let criteria = Self::text(
            "criteria",
            criteria.as_deref().unwrap_or(""),
            MAX_TASK_CRITERIA_CHARS,
        )?;
        let owner = match owner {
            Some(owner) => {
                Some(Self::text("owner", owner, MAX_TASK_OWNER_CHARS)?).filter(|o| !o.is_empty())
            }
            None => None,
        };
        let status = status.unwrap_or(TaskStatus::Pending);
        if status.is_terminal() {
            return Err(format!(
                "a task cannot be created {}; create it open and finish it in a later operation",
                status.as_str()
            ));
        }
        let id = self.state.next_id;
        let depends_on = self.deps(depends_on, Some(id))?;
        let parent = match parent {
            Some(p) => {
                let parent = self.resolve(p)?;
                self.parent_ok(id, parent)?;
                Some(parent)
            }
            None => None,
        };
        if let Some(name) = r#ref {
            let name = name.trim();
            if name.is_empty() || name.len() > 64 {
                return Err("ref must be 1 to 64 bytes".into());
            }
            if self.refs.contains_key(name) {
                return Err(format!("ref \"{name}\" is used twice in this batch"));
            }
            self.refs.insert(name.to_owned(), id);
            self.created.insert(name.to_owned(), id);
        } else {
            self.created.insert(format!("#{}", self.op), id);
        }
        self.state.next_id += 1;
        let revision = self.state.revision + 1;
        self.state.tasks.push(PlanTaskView {
            id,
            parent,
            title,
            detail,
            status,
            reason: None,
            depends_on,
            criteria,
            evidence: Vec::new(),
            owner,
            archived: false,
            stale: false,
            revision,
            created_at_ms: self.now,
            updated_at_ms: self.now,
            completed_at_ms: None,
        });
        self.changed.insert(id);
        self.rule_check.insert(id);
        self.events.push(PlanEvent {
            task: id,
            kind: "created",
            data: json!({ "status": status.as_str() }),
        });
        Ok(())
    }

    fn update(&mut self, op: &PlanOp) -> Step {
        let PlanOp::Update {
            task,
            title,
            detail,
            criteria,
            depends_on,
            parent,
            status,
            reason,
            owner,
            add_evidence,
            clear_evidence,
        } = op
        else {
            unreachable!()
        };
        let at = self.resolve_live(task)?;
        let id = self.state.tasks[at].id;
        let reason = match reason {
            Some(reason) => {
                Some(Self::text("reason", reason, MAX_TASK_REASON_CHARS)?).filter(|r| !r.is_empty())
            }
            None => None,
        };
        let mut changes = serde_json::Map::new();
        if let Some(title) = title {
            let title = Self::title(title)?;
            if title != self.state.tasks[at].title {
                changes.insert("title".into(), json!(self.state.tasks[at].title));
                self.state.tasks[at].title = title;
            }
        }
        if let Some(detail) = detail {
            let detail = Self::text("detail", detail, MAX_TASK_DETAIL_CHARS)?;
            if detail != self.state.tasks[at].detail {
                changes.insert("detail".into(), Value::Bool(true));
                self.state.tasks[at].detail = detail;
            }
        }
        if let Some(criteria) = criteria {
            let criteria = Self::text("criteria", criteria, MAX_TASK_CRITERIA_CHARS)?;
            if criteria != self.state.tasks[at].criteria {
                changes.insert("criteria".into(), json!(self.state.tasks[at].criteria));
                self.state.tasks[at].criteria = criteria;
                if self.state.tasks[at].status == TaskStatus::Completed && status.is_none() {
                    // The completion was judged against other criteria.
                    self.state.tasks[at].stale = true;
                    changes.insert("stale".into(), Value::Bool(true));
                }
            }
        }
        if let Some(owner) = owner {
            let owner =
                Some(Self::text("owner", owner, MAX_TASK_OWNER_CHARS)?).filter(|o| !o.is_empty());
            if owner != self.state.tasks[at].owner {
                changes.insert("owner".into(), json!(self.state.tasks[at].owner));
                self.state.tasks[at].owner = owner;
            }
        }
        if let Some(refs) = depends_on {
            let deps = self.deps(refs, Some(id))?;
            if deps != self.state.tasks[at].depends_on {
                changes.insert("dependsOn".into(), json!(self.state.tasks[at].depends_on));
                self.state.tasks[at].depends_on = deps;
                self.rule_check.insert(id);
            }
        }
        if let Some(parent) = parent {
            let parent = self.resolve(parent)?;
            self.parent_ok(id, parent)?;
            if self.state.tasks[at].parent != Some(parent) {
                changes.insert("parent".into(), json!(self.state.tasks[at].parent));
                self.state.tasks[at].parent = Some(parent);
            }
        }
        if *clear_evidence && !self.state.tasks[at].evidence.is_empty() {
            changes.insert(
                "evidenceCleared".into(),
                json!(self.state.tasks[at].evidence),
            );
            self.state.tasks[at].evidence.clear();
        }
        if !add_evidence.is_empty() {
            let more = Self::evidence(add_evidence)?;
            if self.state.tasks[at].evidence.len() + more.len() > MAX_TASK_EVIDENCE {
                return Err(format!(
                    "a task can carry at most {MAX_TASK_EVIDENCE} evidence items"
                ));
            }
            changes.insert("evidenceAdded".into(), json!(more));
            self.state.tasks[at].evidence.extend(more);
        }
        if let Some(new) = *status {
            let old = self.state.tasks[at].status;
            if new != old {
                let reopening = old.is_terminal() && !new.is_terminal();
                if new.needs_reason() && reason.is_none() {
                    return Err(format!(
                        "a reason is required to mark task #{id} {}",
                        new.as_str()
                    ));
                }
                if old.is_terminal() && reason.is_none() {
                    return Err(format!(
                        "a reason is required to change task #{id} from {} to {}",
                        old.as_str(),
                        new.as_str()
                    ));
                }
                let prior = json!({
                    "status": old.as_str(),
                    "reason": self.state.tasks[at].reason,
                    "evidence": self.state.tasks[at].evidence,
                    "completedAtMs": self.state.tasks[at].completed_at_ms,
                    "revision": self.state.tasks[at].revision,
                });
                self.state.tasks[at].status = new;
                self.state.tasks[at].reason = reason.clone();
                match new {
                    TaskStatus::Completed => {
                        self.state.tasks[at].completed_at_ms = Some(self.now);
                        self.state.tasks[at].stale = false;
                    }
                    _ if reopening => {
                        self.state.tasks[at].completed_at_ms = None;
                        self.state.tasks[at].stale = false;
                    }
                    _ => {}
                }
                self.rule_check.insert(id);
                self.events.push(PlanEvent {
                    task: id,
                    kind: if reopening { "reopened" } else { "status" },
                    data: json!({ "to": new.as_str(), "reason": reason, "prior": prior }),
                });
                if reopening {
                    // Work that was completed on top of this task now stands on changed ground.
                    let dependents: Vec<usize> = self
                        .state
                        .tasks
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| {
                            !t.archived
                                && t.status == TaskStatus::Completed
                                && t.depends_on.contains(&id)
                                && !t.stale
                        })
                        .map(|(i, _)| i)
                        .collect();
                    for dep_at in dependents {
                        self.state.tasks[dep_at].stale = true;
                        let dep_id = self.state.tasks[dep_at].id;
                        self.touch(dep_at);
                        self.events.push(PlanEvent {
                            task: dep_id,
                            kind: "stale",
                            data: json!({ "because": id }),
                        });
                    }
                }
            } else if let Some(reason) = &reason
                && self.state.tasks[at].reason.as_deref() != Some(reason)
            {
                changes.insert("reason".into(), json!(self.state.tasks[at].reason));
                self.state.tasks[at].reason = Some(reason.clone());
            }
        } else if let Some(reason) = &reason
            && self.state.tasks[at].reason.as_deref() != Some(reason)
        {
            changes.insert("reason".into(), json!(self.state.tasks[at].reason));
            self.state.tasks[at].reason = Some(reason.clone());
        }
        if !changes.is_empty() {
            self.events.push(PlanEvent {
                task: id,
                kind: "updated",
                data: Value::Object(changes),
            });
        }
        if self.events.iter().any(|e| e.task == id) || status.is_some() {
            self.touch(at);
        }
        Ok(())
    }

    fn reorder(&mut self, op: &PlanOp) -> Step {
        let PlanOp::Reorder { task, after } = op else {
            unreachable!()
        };
        let at = self.resolve_live(task)?;
        let id = self.state.tasks[at].id;
        let anchor = match after {
            None => None,
            Some(anchor) => {
                let anchor_id = self.state.tasks[self.resolve_live(anchor)?].id;
                if anchor_id == id {
                    return Err(format!("task #{id} cannot be placed after itself"));
                }
                Some(anchor_id)
            }
        };
        let moving = self.state.tasks.remove(at);
        let to = match anchor {
            None => self
                .state
                .tasks
                .iter()
                .position(|t| !t.archived)
                .unwrap_or(self.state.tasks.len()),
            Some(anchor_id) => self.find(anchor_id).expect("anchor stays") + 1,
        };
        self.state.tasks.insert(to, moving);
        let at = self.find(id).expect("reinserted");
        self.touch(at);
        self.events.push(PlanEvent {
            task: id,
            kind: "reordered",
            data: json!({ "position": to }),
        });
        Ok(())
    }

    fn archive_at(&mut self, at: usize) -> Step {
        let task = &self.state.tasks[at];
        if !task.status.is_terminal() {
            return Err(format!(
                "task #{} is {} and cannot be archived until it is finished",
                task.id,
                task.status.as_str()
            ));
        }
        let id = task.id;
        self.state.tasks[at].archived = true;
        self.touch(at);
        self.events.push(PlanEvent {
            task: id,
            kind: "archived",
            data: Value::Null,
        });
        Ok(())
    }

    fn archive(&mut self, op: &PlanOp) -> Step {
        let PlanOp::Archive { task } = op else {
            unreachable!()
        };
        let at = self.resolve_live(task)?;
        self.archive_at(at)
    }

    fn archive_finished(&mut self) -> Step {
        let finished: Vec<usize> = self
            .state
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.archived && t.status.is_terminal())
            .map(|(i, _)| i)
            .collect();
        for at in finished {
            self.archive_at(at)?;
        }
        Ok(())
    }

    /// Whole-plan rules after every operation: no dependency cycles, the dependency rule for
    /// tasks the batch touched, and the live-task bound.
    fn check(&self) -> Result<(), String> {
        let live = self.state.tasks.iter().filter(|t| !t.archived).count();
        if live > MAX_PLAN_TASKS {
            return Err(format!(
                "a plan can have at most {MAX_PLAN_TASKS} live tasks; archive finished work"
            ));
        }
        let by_id: HashMap<u64, &PlanTaskView> =
            self.state.tasks.iter().map(|t| (t.id, t)).collect();
        // Cycle detection over the dependency graph.
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Visiting,
            Done,
        }
        let mut marks: HashMap<u64, Mark> = HashMap::new();
        fn visit(
            id: u64,
            by_id: &HashMap<u64, &PlanTaskView>,
            marks: &mut HashMap<u64, Mark>,
            path: &mut Vec<u64>,
        ) -> Result<(), String> {
            match marks.get(&id) {
                Some(Mark::Done) => return Ok(()),
                Some(Mark::Visiting) => {
                    let start = path.iter().position(|p| *p == id).unwrap_or(0);
                    let cycle: Vec<String> = path[start..]
                        .iter()
                        .chain(std::iter::once(&id))
                        .map(|p| format!("#{p}"))
                        .collect();
                    return Err(format!("dependencies form a cycle: {}", cycle.join(" -> ")));
                }
                None => {}
            }
            marks.insert(id, Mark::Visiting);
            path.push(id);
            if let Some(task) = by_id.get(&id) {
                for dep in &task.depends_on {
                    if !by_id.contains_key(dep) {
                        return Err(format!(
                            "task #{id} depends on #{dep}, which does not exist"
                        ));
                    }
                    visit(*dep, by_id, marks, path)?;
                }
            }
            path.pop();
            marks.insert(id, Mark::Done);
            Ok(())
        }
        for task in &self.state.tasks {
            visit(task.id, &by_id, &mut marks, &mut Vec::new())?;
        }
        for id in &self.rule_check {
            let Some(task) = by_id.get(id) else { continue };
            // A stale completion is a claim whose basis changed; it stands flagged, so the
            // rule does not reject the batch that reopened its dependency.
            if !matches!(task.status, TaskStatus::Active | TaskStatus::Completed) || task.stale {
                continue;
            }
            for dep in &task.depends_on {
                let dep_task = by_id[dep];
                if !dep_task.status.satisfies_dependents() {
                    return Err(format!(
                        "task #{} cannot be {} while it depends on #{} ({}); mark it blocked, \
                         or finish or skip #{} first",
                        task.id,
                        task.status.as_str(),
                        dep,
                        dep_task.status.as_str(),
                        dep
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A fresh, independent task batch replaces a finished plan in the current view. Explicit
/// references to prior tasks are continuations, not a request to archive those tasks.
fn starts_new_work(state: &PlanState, ops: &[PlanOp]) -> bool {
    let new_ref = |r: &TaskRef| matches!(r, TaskRef::New(_));
    state.tasks.iter().any(|t| !t.archived)
        && state
            .tasks
            .iter()
            .all(|t| t.archived || t.status.is_terminal())
        && ops.iter().any(|op| matches!(op, PlanOp::Create { .. }))
        && ops.iter().all(|op| match op {
            PlanOp::Create {
                depends_on, parent, ..
            } => depends_on.iter().chain(parent.iter()).all(new_ref),
            PlanOp::Update {
                task,
                depends_on,
                parent,
                ..
            } => std::iter::once(task)
                .chain(depends_on.iter().flatten())
                .chain(parent.iter())
                .all(new_ref),
            PlanOp::Reorder { task, after } => {
                std::iter::once(task).chain(after.iter()).all(new_ref)
            }
            PlanOp::Archive { .. } | PlanOp::ArchiveFinished => false,
        })
}

/// Apply `ops` to `state` atomically, including retirement of a previous finished plan.
pub fn apply(
    state: &PlanState,
    ops: &[PlanOp],
    expected_revision: Option<u64>,
    now: i64,
) -> Result<Applied, PlanRejected> {
    let reject = |code: ErrorCode, op: Option<usize>, message: String| PlanRejected {
        code,
        op,
        message,
        revision: state.revision,
    };
    if let Some(expected) = expected_revision
        && expected != state.revision
    {
        return Err(reject(
            ErrorCode::Conflict,
            None,
            format!(
                "the plan is at revision {}, not {expected}; read it again before changing it",
                state.revision
            ),
        ));
    }
    if ops.is_empty() {
        return Err(reject(
            ErrorCode::Invalid,
            None,
            "the batch has no operations".into(),
        ));
    }
    if ops.len() > MAX_PLAN_OPS {
        return Err(reject(
            ErrorCode::Invalid,
            None,
            format!("a batch can have at most {MAX_PLAN_OPS} operations"),
        ));
    }
    let mut batch = Batch {
        state: state.clone(),
        refs: HashMap::new(),
        created: BTreeMap::new(),
        changed: HashSet::new(),
        rule_check: HashSet::new(),
        events: Vec::new(),
        now,
        op: 0,
        ops,
    };
    if starts_new_work(state, ops) {
        batch
            .archive_finished()
            .map_err(|message| reject(ErrorCode::Invalid, None, message))?;
    }
    for (i, op) in ops.iter().enumerate() {
        batch.op = i;
        let step = match op {
            PlanOp::Create { .. } => batch.create(op),
            PlanOp::Update { .. } => batch.update(op),
            PlanOp::Reorder { .. } => batch.reorder(op),
            PlanOp::Archive { .. } => batch.archive(op),
            PlanOp::ArchiveFinished => batch.archive_finished(),
        };
        if let Err(message) = step {
            return Err(reject(ErrorCode::Invalid, Some(i), message));
        }
    }
    if let Err(message) = batch.check() {
        return Err(reject(ErrorCode::Invalid, None, message));
    }
    let _ = batch.ops;
    let mut state = batch.state;
    state.revision += 1;
    let mut changed: Vec<u64> = batch.changed.into_iter().collect();
    changed.sort_unstable();
    Ok(Applied {
        receipt: PlanReceipt {
            revision: state.revision,
            created: batch.created,
            changed,
        },
        state,
        events: batch.events,
    })
}

/// The client/model view of a state.
pub fn view(state: &PlanState, include_archived: bool) -> PlanView {
    let mut counts = PlanCounts::default();
    for task in &state.tasks {
        if task.archived {
            counts.archived += 1;
            continue;
        }
        match task.status {
            TaskStatus::Pending => counts.pending += 1,
            TaskStatus::Active => counts.active += 1,
            TaskStatus::Blocked => counts.blocked += 1,
            TaskStatus::Completed => counts.completed += 1,
            TaskStatus::Skipped => counts.skipped += 1,
            TaskStatus::Cancelled => counts.cancelled += 1,
        }
    }
    let mut tasks: Vec<PlanTaskView> = state
        .tasks
        .iter()
        .filter(|t| include_archived || !t.archived)
        .cloned()
        .collect();
    let truncated = tasks.len() > MAX_PLAN_VIEW_TASKS;
    if truncated {
        // Keep every live task; drop the oldest archived ones.
        let mut kept = Vec::with_capacity(MAX_PLAN_VIEW_TASKS);
        let live = tasks.iter().filter(|t| !t.archived).count();
        let mut archived_room = MAX_PLAN_VIEW_TASKS.saturating_sub(live);
        let archived_total = tasks.len() - live;
        let mut skip = archived_total.saturating_sub(archived_room);
        for task in tasks {
            if task.archived {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if archived_room == 0 {
                    continue;
                }
                archived_room -= 1;
            }
            kept.push(task);
        }
        tasks = kept;
    }
    PlanView {
        revision: state.revision,
        tasks,
        counts,
        truncated,
    }
}

// ---------------------------------------------------------------------------- rendering

/// One task on one line: `#3 [active] Title (after #1, #2) [stale]`.
pub fn line(task: &PlanTaskView) -> String {
    let mut s = format!("#{} [{}] {}", task.id, task.status.as_str(), task.title);
    if let Some(parent) = task.parent {
        s.push_str(&format!(" (under #{parent})"));
    }
    if !task.depends_on.is_empty() {
        let deps: Vec<String> = task.depends_on.iter().map(|d| format!("#{d}")).collect();
        s.push_str(&format!(" (after {})", deps.join(", ")));
    }
    if let Some(owner) = &task.owner {
        s.push_str(&format!(" [{owner}]"));
    }
    if task.stale {
        s.push_str(" [stale]");
    }
    if task.archived {
        s.push_str(" [archived]");
    }
    s
}

/// `3 open (1 active, 1 blocked, 1 pending) · 2 completed, 1 skipped, 0 cancelled · 4 archived`.
pub fn counts_line(counts: &PlanCounts) -> String {
    let mut s = format!(
        "{} open ({} active, {} blocked, {} pending) · {} completed, {} skipped, {} cancelled",
        counts.open(),
        counts.active,
        counts.blocked,
        counts.pending,
        counts.completed,
        counts.skipped,
        counts.cancelled
    );
    if counts.archived > 0 {
        s.push_str(&format!(" · {} archived", counts.archived));
    }
    s
}

/// A task in full, for `plan_read` of one task.
pub fn detail(task: &PlanTaskView) -> String {
    let mut out = vec![line(task)];
    if !task.detail.is_empty() {
        out.push(format!("Detail: {}", task.detail));
    }
    if !task.criteria.is_empty() {
        out.push(format!("Criteria: {}", task.criteria));
    }
    if let Some(reason) = &task.reason {
        out.push(format!("Reason: {reason}"));
    }
    if !task.evidence.is_empty() {
        out.push("Evidence:".into());
        for e in &task.evidence {
            match &e.note {
                Some(note) => out.push(format!("- {}: {} — {note}", e.kind, e.reference)),
                None => out.push(format!("- {}: {}", e.kind, e.reference)),
            }
        }
    }
    out.push(format!(
        "Revision {}; created {}; updated {}{}",
        task.revision,
        task.created_at_ms,
        task.updated_at_ms,
        task.completed_at_ms
            .map(|at| format!("; completed {at}"))
            .unwrap_or_default()
    ));
    out.join("\n")
}

/// Most task lines the prompt summary carries.
pub const SUMMARY_LINES: usize = 30;

/// The concise active-plan section injected into the model context, or `None` when the plan
/// has no live tasks. Lists open tasks only; finished work is counted, not listed.
pub fn summary(view: &PlanView) -> Option<String> {
    if view.tasks.is_empty() {
        return None;
    }
    let mut lines = vec![format!(
        "Current plan (revision {}; {}). Use plan_read for details and plan_update to change it.",
        view.revision,
        counts_line(&view.counts)
    )];
    let open: Vec<&PlanTaskView> = view
        .tasks
        .iter()
        .filter(|t| !t.archived && !t.status.is_terminal())
        .collect();
    for task in open.iter().take(SUMMARY_LINES) {
        lines.push(format!("- {}", line(task)));
    }
    if open.len() > SUMMARY_LINES {
        lines.push(format!(
            "- …and {} more open tasks",
            open.len() - SUMMARY_LINES
        ));
    }
    let stale = view.tasks.iter().filter(|t| !t.archived && t.stale).count();
    if stale > 0 {
        lines.push(format!(
            "{stale} completed task(s) are stale: their basis changed; re-check and complete again."
        ));
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests;

//! Background jobs as the desktop shows them: the engine's job list for the open
//! conversation, the person's view choices (history, the expanded job), a bounded log tail
//! fetched on request, and an attention count for jobs that finished while not in view.
//! The engine owns the jobs; nothing here starts or settles one.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum JobState {
    #[default]
    Starting,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Interrupted,
    Unknown,
}

impl JobState {
    pub fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Cancelling)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Cancelling => "Stopping",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::TimedOut => "Timed out",
            Self::Cancelled => "Stopped",
            Self::Interrupted => "Interrupted",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobOrigin {
    Model,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Job {
    pub id: u64,
    pub name: Option<String>,
    pub command: String,
    pub cwd: String,
    pub origin: JobOrigin,
    pub state: JobState,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    /// The tool call that started it (model origin), for the transcript link.
    pub call_id: Option<String>,
    /// A plan task the job is linked to.
    pub task: Option<u64>,
    pub wake: bool,
    /// `pending`, `notified`, `woken`, `suppressed` or `none`.
    pub delivery: String,
    pub log_bytes: u64,
    pub log_truncated: bool,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

impl Job {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.command)
    }

    /// Elapsed milliseconds: to the end, or to `now` while running.
    pub fn elapsed_ms(&self, now: i64) -> Option<i64> {
        let start = self.started_at_ms?;
        Some(self.ended_at_ms.unwrap_or(now).saturating_sub(start).max(0))
    }
}

/// A bounded slice of a job's log, as fetched for the expanded job.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobLogTail {
    pub id: u64,
    pub offset: u64,
    pub text: String,
    pub next_offset: u64,
    pub total_bytes: u64,
    pub truncated: bool,
    pub done: bool,
}

/// What the engine sends on every change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobsSync {
    pub jobs: Vec<Job>,
    pub active: usize,
    pub truncated: bool,
}

/// Bytes of log shown for the expanded job.
pub const JOB_LOG_TAIL_BYTES: u64 = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobsState {
    /// The engine reports jobs (the demo and engines without jobs leave this unset).
    pub available: bool,
    /// Active first, then finished newest first (the engine's order).
    pub jobs: Vec<Job>,
    pub active: usize,
    /// Older finished jobs exist beyond the listed ones.
    pub truncated: bool,
    pub history_open: bool,
    /// The one job shown in full with its log tail.
    pub expanded: Option<u64>,
    pub log: Option<JobLogTail>,
    /// A log request is in flight for this job (coalesces refreshes).
    pub pending_log: Option<u64>,
    /// Jobs that finished since the person last looked at the list.
    pub unseen: usize,
    pub error: Option<String>,
}

impl JobsState {
    pub fn job(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn finished(&self) -> usize {
        self.jobs.iter().filter(|j| !j.state.active()).count()
    }

    /// Apply the engine's list. Returns whether the expanded job's log should be fetched
    /// again (it is active and its log grew, or it just finished).
    pub fn sync(&mut self, sync: JobsSync) -> bool {
        self.available = true;
        let was_active: std::collections::HashSet<u64> = self
            .jobs
            .iter()
            .filter(|j| j.state.active())
            .map(|j| j.id)
            .collect();
        let newly_finished = sync
            .jobs
            .iter()
            .filter(|j| !j.state.active() && was_active.contains(&j.id))
            .count();
        self.unseen += newly_finished;
        let refetch = match (self.expanded, &self.log) {
            (Some(id), Some(log)) => sync.jobs.iter().find(|j| j.id == id).is_some_and(|j| {
                j.log_bytes != log.total_bytes || (!j.state.active() && !log.done)
            }),
            (Some(id), None) => sync.jobs.iter().any(|j| j.id == id),
            _ => false,
        };
        self.jobs = sync.jobs;
        self.active = sync.active;
        self.truncated = sync.truncated;
        if let Some(id) = self.expanded
            && self.job(id).is_none()
        {
            self.expanded = None;
            self.log = None;
        }
        refetch && self.pending_log.is_none()
    }

    pub fn log_received(&mut self, tail: JobLogTail) {
        if self.pending_log == Some(tail.id) {
            self.pending_log = None;
        }
        if self.expanded == Some(tail.id) {
            self.log = Some(tail);
        }
    }

    /// The byte range to ask for: the newest `JOB_LOG_TAIL_BYTES` of the job's log.
    pub fn tail_request(&self, id: u64) -> Option<(u64, u64)> {
        let job = self.job(id)?;
        Some((
            job.log_bytes.saturating_sub(JOB_LOG_TAIL_BYTES),
            JOB_LOG_TAIL_BYTES,
        ))
    }
}

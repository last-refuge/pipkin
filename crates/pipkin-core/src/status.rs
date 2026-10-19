//! The compact status model (plan §11, STATUS-1): what the person should see at a glance
//! about the open conversation, derived from state on demand. Segments are omitted when
//! they would be empty, so the strip never shows zeros. Context occupancy comes from the
//! engine's estimate (STATUS-2), never from cumulative usage.

use crate::model::{RunState, SessionUsage};
use crate::state::{AppState, ConversationState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextSource {
    #[default]
    Unavailable,
    Reported,
    ReportedInput,
    Estimated,
}

impl ContextSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "unknown",
            Self::Reported => "reported",
            Self::ReportedInput => "last request input",
            Self::Estimated => "estimated",
        }
    }
}

/// Context occupancy as the engine reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub source: ContextSource,
    pub capacity: Option<u64>,
    pub reserve: Option<u64>,
    pub remaining: Option<u64>,
    pub model: Option<String>,
    pub stale: bool,
    /// `idle`, `background`, `blocking` or `off`.
    pub compaction: String,
    pub revision: u64,
}

impl ContextUsage {
    /// Whole percent of the window in use, from occupancy and capacity only.
    pub fn percent(&self) -> Option<u8> {
        let (tokens, capacity) = (self.tokens?, self.capacity?);
        if capacity == 0 {
            return None;
        }
        Some(((tokens.saturating_mul(100)) / capacity).min(999) as u8)
    }

    pub fn known(&self) -> bool {
        self.tokens.is_some() && self.source != ContextSource::Unavailable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusPhase {
    Idle,
    Submitting,
    Working,
    Stopping,
    Unknown,
    Failed,
    /// A question from the model or an extension is waiting for the person.
    Waiting,
}

impl StatusPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Submitting => "Sending",
            Self::Working => "Working",
            Self::Stopping => "Stopping",
            Self::Unknown => "Outcome unknown",
            Self::Failed => "Failed",
            Self::Waiting => "Waiting for you",
        }
    }
}

/// What the strip shows. Counts are engine state, not mounted rows.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusLine {
    pub model: Option<String>,
    /// The effort level when it is not `off`.
    pub effort: Option<String>,
    pub phase: StatusPhase,
    pub queued: usize,
    pub active_jobs: usize,
    pub active_children: usize,
    pub open_questions: usize,
    pub context: Option<ContextUsage>,
    /// Committed session totals, when the engine reported any.
    pub total_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

impl StatusLine {
    /// One sentence for assistive technology, in reading order.
    pub fn accessible(&self) -> String {
        let mut parts = vec![self.phase.label().to_owned()];
        if let Some(model) = &self.model {
            parts.push(match &self.effort {
                Some(effort) => format!("model {model}, effort {effort}"),
                None => format!("model {model}"),
            });
        }
        if self.queued > 0 {
            parts.push(format!("{} queued", self.queued));
        }
        if self.active_jobs > 0 {
            parts.push(format!("{} jobs running", self.active_jobs));
        }
        if self.active_children > 0 {
            parts.push(format!("{} subagents running", self.active_children));
        }
        if self.open_questions > 0 {
            parts.push(format!("{} questions waiting", self.open_questions));
        }
        if let Some(context) = &self.context
            && context.known()
        {
            match (context.percent(), context.tokens) {
                (Some(p), _) => parts.push(format!(
                    "context {p} percent used ({})",
                    context.source.label()
                )),
                (None, Some(t)) => {
                    parts.push(format!("context {t} tokens ({})", context.source.label()))
                }
                _ => {}
            }
        }
        if let Some(total) = self.total_tokens {
            parts.push(format!("{total} session tokens"));
        }
        parts.join(", ")
    }
}

fn phase_of(c: &ConversationState) -> StatusPhase {
    if !c.ui_requests.is_empty() {
        return StatusPhase::Waiting;
    }
    match &c.run {
        RunState::Idle => StatusPhase::Idle,
        RunState::Submitting { .. } => StatusPhase::Submitting,
        RunState::Running { .. } => StatusPhase::Working,
        RunState::Stopping { .. } => StatusPhase::Stopping,
        RunState::OutcomeUnknown { .. } => StatusPhase::Unknown,
        RunState::Failed { .. } => StatusPhase::Failed,
    }
}

fn is_active_child(status: &str) -> bool {
    matches!(status, "pending" | "running" | "waiting" | "completing")
}

impl AppState {
    /// The status of the open conversation, or `None` without one.
    pub fn status_line(&self) -> Option<StatusLine> {
        let c = self.current()?;
        let model = self
            .prefs
            .model
            .as_ref()
            .and_then(|id| self.models.iter().find(|m| &m.id == id))
            .map(|m| m.name.clone());
        let effort = c
            .thinking_level
            .as_deref()
            .filter(|l| *l != "off")
            .map(str::to_owned);
        let totals = c.usage.as_ref().map(SessionUsage::total);
        Some(StatusLine {
            model,
            effort,
            phase: phase_of(c),
            queued: c.queue.len() + c.pending_queue.len(),
            active_jobs: c.jobs.active,
            active_children: c
                .subagents
                .children
                .iter()
                .filter(|child| is_active_child(&child.status))
                .count(),
            open_questions: c.ui_requests.len(),
            context: c.context.clone(),
            total_tokens: totals.as_ref().map(|t| t.total_tokens),
            cost_usd: totals.and_then(|t| t.cost_usd),
        })
    }
}

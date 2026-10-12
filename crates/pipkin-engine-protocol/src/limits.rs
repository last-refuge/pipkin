//! Hard limits, by bytes as well as counts. Both ends enforce them; exceeding one is a
//! definite refusal (`ErrorCode::TooLarge`), never a silent truncation of user input.

/// Largest encoded frame in either direction.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
/// Largest prompt/steer/follow-up text.
pub const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
/// Most images attached to one input.
pub const MAX_IMAGES: usize = 8;
/// Largest decoded image.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Most entries returned by one history page.
pub const MAX_PAGE_ENTRIES: u32 = 500;
/// Default entries in the window returned by `OpenSession`.
pub const DEFAULT_WINDOW_ENTRIES: u32 = 200;
/// Most steers and follow-ups an engine holds per session.
pub const MAX_QUEUE: usize = 64;
/// Longest request key.
pub const MAX_KEY_BYTES: usize = 256;
/// Most extension questions open at once per session (the desktop's existing limit).
pub const MAX_OPEN_QUESTIONS: usize = 20;
/// Bounded tool-output preview carried in views; the retained output is fetched separately.
pub const TOOL_PREVIEW_BYTES: usize = 8 * 1024;
/// Largest retained output per tool call. Bytes beyond it are dropped and the drop is reported.
pub const MAX_RETAINED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Most subagent rows returned by one directory page.
pub const MAX_SUBAGENT_ROWS: u32 = 4096;
/// Longest task excerpt carried in a subagent row.
pub const SUBAGENT_TASK_EXCERPT_CHARS: usize = 160;
/// Most live (non-archived) tasks in one plan.
pub const MAX_PLAN_TASKS: usize = 500;
/// Most tasks (live plus archived) returned in one plan view.
pub const MAX_PLAN_VIEW_TASKS: usize = 2000;
/// Most operations in one plan batch.
pub const MAX_PLAN_OPS: usize = 100;
/// Longest task title (UTF-16 units).
pub const MAX_TASK_TITLE_CHARS: usize = 200;
/// Longest task detail.
pub const MAX_TASK_DETAIL_CHARS: usize = 4000;
/// Longest task completion criteria.
pub const MAX_TASK_CRITERIA_CHARS: usize = 2000;
/// Longest skip/cancel/reopen reason.
pub const MAX_TASK_REASON_CHARS: usize = 1000;
/// Longest owner reference.
pub const MAX_TASK_OWNER_CHARS: usize = 200;
/// Most dependencies of one task.
pub const MAX_TASK_DEPENDENCIES: usize = 50;
/// Most evidence items on one task.
pub const MAX_TASK_EVIDENCE: usize = 20;
/// Longest evidence kind, reference or note.
pub const MAX_EVIDENCE_CHARS: usize = 500;

//! Seeded projects, conversations and deterministic generated history.
//!
//! History is a pure function of `(seed, conversation, index)`, so any page can be produced
//! on demand and two runs with the same seed are identical. Nothing here reads a clock or
//! the environment, and every command-looking string is plain, inert text.

use pipkin_core::*;

use super::rng::Rng;
use super::story;

/// Items per history page.
pub const PAGE: usize = 200;
/// Size of the special large conversation.
pub const LARGE_LEN: usize = 10_000;
/// Fixed default "now" for fixtures: 2026-01-15 09:00:00 UTC.
pub const DEFAULT_BASE_TIME: i64 = 1_768_467_600;

const ID_STRIDE: u64 = 100_000;

pub fn item_id(conv: ConversationId, idx: usize) -> ItemId {
    ItemId(conv.0 * ID_STRIDE + idx as u64 + 1)
}

pub fn item_index(conv: ConversationId, id: ItemId) -> Option<usize> {
    let base = conv.0 * ID_STRIDE;
    (id.0 > base && id.0 <= base + ID_STRIDE).then(|| (id.0 - base - 1) as usize)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Seeded(usize),
    Large,
    LargeDiff,
    BigOutput,
    Stress,
    Empty,
}

pub struct Spec {
    pub id: u64,
    pub project: u64,
    pub title: &'static str,
    /// Seconds before the base time of the last activity.
    pub age: i64,
    pub kind: Kind,
}

#[cfg(test)]
pub const LARGE_CONVERSATION: ConversationId = ConversationId(4);
#[cfg(test)]
pub const LARGE_DIFF_CONVERSATION: ConversationId = ConversationId(5);
#[cfg(test)]
pub const BIG_OUTPUT_CONVERSATION: ConversationId = ConversationId(6);
#[cfg(test)]
pub const STRESS_CONVERSATION: ConversationId = ConversationId(7);
#[cfg(test)]
pub const EMPTY_CONVERSATION: ConversationId = ConversationId(8);
pub const STORY_CONVERSATION: ConversationId = ConversationId(1);

pub const SPECS: &[Spec] = &[
    Spec {
        id: 1,
        project: 1,
        title: "Fix failing search test in pipkin-core",
        age: 600,
        kind: Kind::Seeded(14),
    },
    Spec {
        id: 2,
        project: 1,
        title: "Wire composer IME composition into the input handler",
        age: 3_600,
        kind: Kind::Seeded(22),
    },
    Spec {
        id: 3,
        project: 1,
        title: "Why does the transcript viewport jump when older pages prepend? Investigate scroll anchoring across virtualized rows with variable heights and expanded tool output",
        age: 7_200,
        kind: Kind::Seeded(30),
    },
    Spec {
        id: 4,
        project: 1,
        title: "Large history \u{2014} 10,000 messages",
        age: 90_000,
        kind: Kind::Large,
    },
    Spec {
        id: 5,
        project: 1,
        title: "Large diff \u{2014} regenerate protocol bindings",
        age: 100_000,
        kind: Kind::LargeDiff,
    },
    Spec {
        id: 6,
        project: 1,
        title: "Huge tool output \u{2014} 1.5 MB build log",
        age: 110_000,
        kind: Kind::BigOutput,
    },
    Spec {
        id: 7,
        project: 1,
        title: "Stress content \u{2014} Unicode, RTL & malformed Markdown",
        age: 120_000,
        kind: Kind::Stress,
    },
    Spec {
        id: 8,
        project: 2,
        title: "Empty conversation",
        age: 300,
        kind: Kind::Empty,
    },
    Spec {
        id: 9,
        project: 2,
        title: "Rounding error in invoice totals",
        age: 1_800,
        kind: Kind::Seeded(18),
    },
    Spec {
        id: 10,
        project: 2,
        title: "Migrate invoices.status to an enum \u{2014} \u{8acb}\u{6c42}\u{66f8}\u{306e}\u{30b9}\u{30c6}\u{30fc}\u{30bf}\u{30b9} \u{2728} \u{0645}\u{0631}\u{062d}\u{0628}\u{0627}",
        age: 20_000,
        kind: Kind::Seeded(12),
    },
    Spec {
        id: 11,
        project: 2,
        title: "Add idempotency keys to POST /charges",
        age: 50_000,
        kind: Kind::Seeded(26),
    },
    Spec {
        id: 12,
        project: 2,
        title: "Retry policy for webhook delivery",
        age: 200_000,
        kind: Kind::Seeded(8),
    },
];

pub struct Fixtures {
    pub seed: u64,
    pub base: i64,
}

impl Fixtures {
    pub fn new(seed: u64, base: i64) -> Self {
        Fixtures { seed, base }
    }

    pub fn projects(&self) -> Vec<Project> {
        vec![
            Project {
                id: ProjectId(1),
                name: "pipkin".into(),
                path: "/home/jasona/coding/pipkin".into(),
            },
            Project {
                id: ProjectId(2),
                name: "billing-service".into(),
                path: "/home/jasona/work/billing-service".into(),
            },
        ]
    }

    pub fn models(&self) -> Vec<ModelInfo> {
        let m = |id: &str, name: &str, note: &str| ModelInfo {
            id: id.into(),
            name: name.into(),
            note: note.into(),
        };
        vec![
            m(
                "pi-sonnet",
                "Pi Sonnet",
                "Balanced default for everyday coding (fixture)",
            ),
            m(
                "pi-opus",
                "Pi Opus",
                "Deeper reasoning, slower and costlier (fixture)",
            ),
            m(
                "pi-haiku",
                "Pi Haiku",
                "Fast and light for quick edits (fixture)",
            ),
            m(
                "local-llama",
                "Local Llama 3 70B",
                "Runs offline; no tool use (fixture)",
            ),
        ]
    }

    pub fn bootstrap(&self) -> Bootstrap {
        Bootstrap {
            projects: self.projects(),
            models: self.models(),
            conversations: SPECS
                .iter()
                .map(|s| {
                    (
                        ConversationId(s.id),
                        ProjectId(s.project),
                        s.title.to_string(),
                        self.base - s.age,
                    )
                })
                .collect(),
            now: self.base,
        }
    }

    pub fn spec(&self, conv: ConversationId) -> Option<&'static Spec> {
        SPECS.iter().find(|s| s.id == conv.0)
    }

    pub fn len(&self, conv: ConversationId) -> usize {
        match self.spec(conv).map(|s| s.kind) {
            None | Some(Kind::Empty) => 0,
            Some(Kind::Seeded(n)) => n,
            Some(Kind::Large) => LARGE_LEN,
            Some(Kind::LargeDiff) => 8,
            Some(Kind::BigOutput) => 4,
            Some(Kind::Stress) => stress_kinds().len(),
        }
    }

    pub fn changes(&self, conv: ConversationId) -> Vec<FileChange> {
        match self.spec(conv).map(|s| s.kind) {
            _ if conv == STORY_CONVERSATION => story::fix_search_changes(),
            Some(Kind::LargeDiff) => story::large_diff(self.seed),
            _ => Vec::new(),
        }
    }

    /// The most recent page of history.
    pub fn open(&self, conv: ConversationId) -> (Vec<TranscriptItem>, bool, Vec<FileChange>) {
        let len = self.len(conv);
        let start = len.saturating_sub(PAGE);
        (self.range(conv, start, len), start > 0, self.changes(conv))
    }

    /// Up to one page of items strictly before `before` (or before the end when `None`).
    pub fn older(
        &self,
        conv: ConversationId,
        before: Option<ItemId>,
    ) -> (Vec<TranscriptItem>, bool) {
        let end = match before {
            Some(id) => item_index(conv, id).unwrap_or(0).min(self.len(conv)),
            None => self.len(conv),
        };
        let start = end.saturating_sub(PAGE);
        (self.range(conv, start, end), start > 0)
    }

    fn range(&self, conv: ConversationId, start: usize, end: usize) -> Vec<TranscriptItem> {
        let stress =
            matches!(self.spec(conv).map(|s| s.kind), Some(Kind::Stress)).then(stress_kinds);
        (start..end)
            .map(|idx| TranscriptItem {
                native_only: false,
                id: item_id(conv, idx),
                at: self.item_time(conv, idx),
                kind: match &stress {
                    Some(list) => list[idx].clone(),
                    None => self.kind_at(conv, idx),
                },
            })
            .collect()
    }

    fn item_time(&self, conv: ConversationId, idx: usize) -> i64 {
        let age = self.spec(conv).map_or(0, |s| s.age);
        self.base - age - (self.len(conv).saturating_sub(idx) as i64) * 75
    }

    /// Item contents for any non-stress conversation.
    pub fn kind_at(&self, conv: ConversationId, idx: usize) -> ItemKind {
        let Some(spec) = self.spec(conv) else {
            return notice("Unknown conversation");
        };
        let mut rng = Rng::from_parts(&[self.seed, conv.0, idx as u64]);
        match spec.kind {
            Kind::Seeded(len) => seeded_kind(spec.project, len, idx, &mut rng),
            Kind::LargeDiff => {
                if idx == 0 {
                    ItemKind::User {
                        text: "Regenerate the protocol bindings from the updated schema.".into(),
                        attachments: vec![],
                        delivery: Delivery::Sent,
                        steer: false,
                    }
                } else if idx == 7 {
                    assistant(
                        "Regenerated all five binding files. The diff is large because every field widened from `u32` to `u64`; review it in the inspector.".into(),
                    )
                } else {
                    seeded_kind(spec.project, 8, idx, &mut rng)
                }
            }
            Kind::BigOutput => big_output_kind(idx, &mut Rng::from_parts(&[self.seed, 0xB16])),
            Kind::Large => large_kind(idx, &mut rng),
            Kind::Stress | Kind::Empty => notice("No content"),
        }
    }
}

// ----------------------------------------------------------------------------- builders

fn notice(text: &str) -> ItemKind {
    ItemKind::Notice {
        text: text.into(),
        level: NoticeLevel::Info,
    }
}

fn user(text: impl Into<String>) -> ItemKind {
    ItemKind::User {
        text: text.into(),
        attachments: vec![],
        delivery: Delivery::Sent,
        steer: false,
    }
}

fn assistant(text: String) -> ItemKind {
    ItemKind::Assistant {
        text,
        streaming: false,
    }
}

/// Build a tool row with the output bounded the same way the core bounds streamed output.
pub fn tool(name: &str, input: &str, output: &str, status: ToolStatus) -> ItemKind {
    let full_len = output.len();
    let mut cut = full_len.min(TOOL_OUTPUT_PREVIEW_BYTES);
    while !output.is_char_boundary(cut) {
        cut -= 1;
    }
    ItemKind::Tool(ToolCall {
        call_ref: None,
        call_id: None,
        name: name.into(),
        input: input.into(),
        output: output[..cut].to_string(),
        truncated: cut < full_len,
        full_len,
        status,
    })
}

// ----------------------------------------------------------------------- seeded history

const RUST_PROMPTS: &[&str] = &[
    "Why does the transcript jump when I expand a tool row?",
    "Can you add a unit test for draft restore with Unicode text?",
    "The composer loses the caret after a paste. Can you look?",
    "Please rename the `QueueId` usages in the nav module.",
    "Explain how the generation guard drops stale events.",
    "Add a doc comment to `AppState::availability`.",
    "Can you check the clippy warnings in pipkin-core?",
    "Make cancel idempotent, please.",
];

const BILLING_PROMPTS: &[&str] = &[
    "Invoice totals are off by a cent for three-way splits.",
    "Add an index for invoices(status, due_date).",
    "Why is the webhook worker retrying immediately?",
    "Draft a migration that backfills `currency` with 'USD'.",
    "Can we make POST /charges idempotent?",
    "Summarize the failing integration test.",
    "Replace float math with Decimal in the tax calculation.",
    "Document the retry policy for the on-call runbook.",
];

const RUST_FILES: &[(&str, &str)] = &[
    (
        "crates/pipkin-core/src/state.rs",
        "pub fn availability(&self) -> Availability {\n    let Some(c) = self.current() else {\n        return Availability::default();\n    };\n    let has_text = !c.draft.text.trim().is_empty();\n    let idle = matches!(c.run, RunState::Idle | RunState::Failed { .. });\n    Availability { submit: idle && has_text, ..Default::default() }\n}\n",
    ),
    (
        "crates/pipkin-ui/src/model.rs",
        "pub fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {\n    let outcome = self.state.dispatch(command);\n    self.finish(outcome, cx);\n}\n\nfn finish(&mut self, outcome: Outcome, cx: &mut Context<Self>) {\n    for note in outcome.notes {\n        cx.emit(note);\n    }\n    cx.notify();\n}\n",
    ),
    (
        "crates/pipkin-core/src/attach.rs",
        "pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;\n\npub fn describe_attachment(path: &Path) -> Attachment {\n    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());\n    // ...\n}\n",
    ),
];

const BILLING_FILES: &[(&str, &str)] = &[
    (
        "app/billing/totals.py",
        "def split_total(total_cents: int, ways: int) -> list[int]:\n    base, extra = divmod(total_cents, ways)\n    return [base + (1 if i < extra else 0) for i in range(ways)]\n",
    ),
    (
        "db/migrations/0042_add_currency.sql",
        "ALTER TABLE invoices ADD COLUMN currency TEXT NOT NULL DEFAULT 'USD';\nCREATE INDEX idx_invoices_status_due ON invoices (status, due_date);\n",
    ),
    (
        "src/webhooks/retry.ts",
        "export function nextDelayMs(attempt: number): number {\n  const base = Math.min(2 ** attempt * 500, 60_000);\n  return base + Math.floor(Math.random() * 250);\n}\n",
    ),
];

const ASSISTANT_TEMPLATES: &[&str] = &[
    "The jump comes from the **anchor** being stored as a pixel offset instead of an item identity. Three things to change:\n\n- Store the anchor as *(item id, offset within item)*.\n- Re-resolve it after every height change.\n- Skip re-anchoring while the user is *following* the stream.\n\nSee the [Rust reference on slices](https://doc.rust-lang.org/std/primitive.slice.html) for the indexing helpers.\n\n```rust\nstruct Anchor {\n    item: ItemId,\n    offset: f32,\n}\n\nfn resolve(&self, anchor: &Anchor) -> Option<usize> {\n    self.items.iter().position(|i| i.id == anchor.item)\n}\n```\n",
    "Here is the minimal repro as a **pytest** case. It fails today because the remainder is dropped:\n\n1. Split `1000` cents three ways.\n2. Assert the parts sum to the total.\n3. Assert no part differs from another by more than one cent.\n\n```python\ndef test_split_keeps_every_cent():\n    parts = split_total(1000, 3)\n    assert sum(parts) == 1000\n    assert max(parts) - min(parts) <= 1\n```\n\nDocs: [divmod](https://docs.python.org/3/library/functions.html#divmod).\n",
    "I'd model the retry delay as capped exponential backoff with *jitter*:\n\n- attempt 1: ~0.5 s\n- attempt 2: ~1 s\n- attempt 5: ~8 s\n- capped at **60 s**\n\n```typescript\nexport function nextDelayMs(attempt: number): number {\n  const base = Math.min(2 ** attempt * 500, 60_000);\n  return base + Math.floor(Math.random() * 250);\n}\n```\n\nSee [MDN on Math.random](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Math/random).\n",
    "The slow query is a sequential scan on `invoices`. An index on the filter columns fixes it:\n\n- `status` is low-cardinality, so put `due_date` second.\n- *Do not* index `currency`; the planner ignores it.\n\n```sql\nCREATE INDEX CONCURRENTLY idx_invoices_status_due\n    ON invoices (status, due_date)\n    WHERE status <> 'void';\n\nEXPLAIN ANALYZE\nSELECT id FROM invoices WHERE status = 'open' AND due_date < now();\n```\n",
    "Short version: run the checks in this order.\n\n1. Format.\n2. Lint with warnings denied.\n3. Run the tests.\n\n```bash\ncargo fmt --all\ncargo clippy -p pipkin-core --all-targets -- -D warnings\ncargo test -p pipkin-core\n```\n\nThese commands are shown for reference only; nothing was executed here. Notes in [the book](https://doc.rust-lang.org/book/).\n",
    "The server needs a stable key per logical request. A reasonable shape:\n\n- **Key** from the client (`Idempotency-Key` header).\n- **Scope** per account, *not* global.\n- **TTL** of 24 hours.\n\n```go\ntype Key struct {\n\tAccount string\n\tValue   string\n\tExpires time.Time\n}\n\nfunc (s *Store) Seen(k Key) (bool, error) {\n\t_, err := s.db.Exec(`SELECT 1 FROM idem WHERE account=$1 AND value=$2`, k.Account, k.Value)\n\treturn err == nil, err\n}\n```\n",
    "The crate manifest needs the feature flag, and the lockfile will follow:\n\n- Enable `bundled` so no system library is required.\n- Keep the version *pinned* to match the lock.\n\n```toml\n[dependencies]\nrusqlite = { version = \"0.32\", features = [\"bundled\"] }\nserde_json = \"1\"\n```\n\nReference: [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html).\n",
    "The payload the webhook sends looks like this; the `attempt` field is the one to watch:\n\n- **id** is stable across retries.\n- **attempt** increments each delivery.\n- *signature* covers the raw body.\n\n```json\n{\n  \"id\": \"evt_0042\",\n  \"type\": \"invoice.paid\",\n  \"attempt\": 3,\n  \"data\": { \"invoice\": \"inv_1017\", \"amount_cents\": 4999 }\n}\n```\n",
];

const CLIPPY_FAIL: &str = "    Checking pipkin-core v0.0.1 (/home/jasona/coding/pipkin/crates/pipkin-core)\nerror: this `if` has identical blocks\n  --> crates/pipkin-core/src/nav.rs:14:23\n   |\n14 |       if q.is_empty() {\n   |  _______________________^\n15 | |         true\n16 | |     } else {\n   | |_____^\n   |\n   = note: `-D clippy::if-same-then-else` implied by `-D warnings`\n\nerror: could not compile `pipkin-core` (lib) due to 1 previous error\n";

const PYTEST_FAIL: &str = "============================= test session starts ==============================\ncollected 4 items\n\ntests/test_totals.py ...F                                                [100%]\n\n=================================== FAILURES ===================================\n__________________________ test_split_keeps_every_cent _________________________\n\n    def test_split_keeps_every_cent():\n        parts = split_total(1000, 3)\n>       assert sum(parts) == 1000\nE       assert 999 == 1000\n\ntests/test_totals.py:12: AssertionError\n=========================== short test summary info ============================\nFAILED tests/test_totals.py::test_split_keeps_every_cent - assert 999 == 1000\n========================= 1 failed, 3 passed in 0.31s ==========================\n";

fn seeded_kind(project: u64, len: usize, idx: usize, rng: &mut Rng) -> ItemKind {
    let rust = project == 1;
    // One failed tool row per conversation, near the middle.
    let mut fail_idx = len / 2 / 6 * 6 + 4;
    if fail_idx < len / 2 {
        fail_idx += 6;
    }
    if fail_idx >= len {
        fail_idx = 4;
    }
    match idx % 6 {
        0 => user(*rng.pick(if rust { RUST_PROMPTS } else { BILLING_PROMPTS })),
        1 | 5 => assistant(rng.pick(ASSISTANT_TEMPLATES).to_string()),
        2 => {
            let (path, body) = *rng.pick(if rust { RUST_FILES } else { BILLING_FILES });
            tool(
                "read_file",
                &format!("{{\"path\": \"{path}\"}}"),
                body,
                ToolStatus::Ok,
            )
        }
        3 => {
            let (path, _) = *rng.pick(if rust { RUST_FILES } else { BILLING_FILES });
            let (a, r) = (rng.range(1, 6), rng.range(0, 4));
            tool(
                "edit",
                &format!("{{\"path\": \"{path}\", \"old\": \"...\", \"new\": \"...\"}}"),
                &format!("Applied 1 edit to {path} (+{a} -{r})\n"),
                ToolStatus::Ok,
            )
        }
        _ if idx == fail_idx => {
            if rust {
                tool(
                    "bash",
                    "cargo clippy -p pipkin-core -- -D warnings",
                    CLIPPY_FAIL,
                    ToolStatus::Failed,
                )
            } else {
                tool(
                    "bash",
                    "pytest -q tests/test_totals.py",
                    PYTEST_FAIL,
                    ToolStatus::Failed,
                )
            }
        }
        _ => {
            if rust {
                tool(
                    "bash",
                    "cargo test -p pipkin-core",
                    "running 13 tests\n.............\ntest result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
                    ToolStatus::Ok,
                )
            } else {
                tool(
                    "bash",
                    "pytest -q tests/test_totals.py",
                    "....                                                                     [100%]\n4 passed in 0.28s\n",
                    ToolStatus::Ok,
                )
            }
        }
    }
}

// ---------------------------------------------------------------------- large history

const SENTENCES: &[&str] = &[
    "The scroll anchor is an item identity plus an offset, never a pixel position.",
    "Prepending a page must leave the visible rows exactly where they were.",
    "Heights are measured lazily, so estimates are corrected as rows mount.",
    "Completed Markdown blocks are cached and only the streaming tail is reparsed.",
    "A stale generation means the event belongs to an earlier attachment and is dropped.",
    "Cancellation stays pending until the engine confirms the run has settled.",
    "Queued prompts start in order, and only after the previous run completes.",
    "Drafts are acknowledged only after the database transaction has committed.",
    "Oversized tool output is bounded for display and keeps its original length.",
    "Selection is addressed by block identity and text offset, not by painted rows.",
    "Unicode grapheme clusters must survive editing, undo and redo unchanged.",
    "None of the commands mentioned here are executed; they are plain text.",
];

fn paragraph(rng: &mut Rng) -> String {
    let n = rng.range(2, 5);
    (0..n)
        .map(|_| *rng.pick(SENTENCES))
        .collect::<Vec<_>>()
        .join(" ")
}

fn large_kind(idx: usize, rng: &mut Rng) -> ItemKind {
    if idx == 0 {
        return user("Message #0: let's go through the whole backlog, one piece at a time.");
    }
    match rng.below(100) {
        0..=14 => user(format!("Message #{idx}: {}", rng.pick(SENTENCES))),
        15..=34 => assistant(format!("Reply #{idx}: {}", rng.pick(SENTENCES))),
        35..=54 => {
            let paragraphs = rng.range(3, 6);
            let body: Vec<String> = (0..paragraphs).map(|_| paragraph(rng)).collect();
            assistant(format!("**Reply #{idx}**\n\n{}", body.join("\n\n")))
        }
        55..=69 => {
            let template = rng.pick(ASSISTANT_TEMPLATES);
            assistant(format!("Reply #{idx}, with an example:\n\n{template}"))
        }
        70..=79 => tool(
            "read_file",
            &format!("{{\"path\": \"src/module_{}.rs\"}}", idx % 97),
            &format!(
                "// module {}\npub fn item_{idx}() -> usize {{ {idx} }}\n",
                idx % 97
            ),
            ToolStatus::Ok,
        ),
        80..=87 => {
            let bytes = rng.range(2_000, 20_000);
            let out = story::log_text(rng, bytes);
            tool(
                "bash",
                &format!("make build-unit-{idx}"),
                &out,
                ToolStatus::Ok,
            )
        }
        88..=92 => {
            let body: Vec<String> = (0..rng.range(2, 4)).map(|_| paragraph(rng)).collect();
            user(format!("Message #{idx}: {}", body.join("\n\n")))
        }
        _ => notice(&format!("Checkpoint #{idx} saved.")),
    }
}

fn big_output_kind(idx: usize, rng: &mut Rng) -> ItemKind {
    match idx {
        0 => user("Run the full build and show me everything it prints."),
        1 => assistant(
            "Starting the full build. The output is large, so the preview is bounded.".into(),
        ),
        2 => {
            // Original size 1.5 MB; only the bounded preview is materialised for history.
            let preview = story::log_text(rng, TOOL_OUTPUT_PREVIEW_BYTES + 512);
            let mut item = tool("bash", "make build-all VERBOSE=1", &preview, ToolStatus::Ok);
            if let ItemKind::Tool(t) = &mut item {
                t.truncated = true;
                t.full_len = 1_572_864;
            }
            item
        }
        _ => assistant(
            "The build finished. Only the first 8 KB is shown; the original output was 1.5 MB."
                .into(),
        ),
    }
}

// --------------------------------------------------------------------- stress content

/// Hand-written adversarial content. Includes malformed Markdown on purpose.
pub fn stress_kinds() -> Vec<ItemKind> {
    let long_path = "/home/jasona/coding/pipkin/crates/pipkin-ui/src/components/transcript/virtualized/anchoring/very/deeply/nested/directory/structure/that/keeps/going/and/going/with_a_remarkably_long_file_name_that_will_never_fit_on_one_line_even_on_a_wide_monitor_at_any_scale.rs";
    let long_token = "supercalifragilisticexpialidocious".repeat(14);
    vec![
        user(format!("Please look at {long_path} and tell me what you think.")),
        assistant(format!(
            "A very long unbroken token follows and must wrap or scroll without breaking layout:\n\n{long_token}\n\nA long URL: https://example.com/{}?q={}\n",
            "segment/".repeat(30),
            "x".repeat(120)
        )),
        user("Emoji: \u{1F680}\u{1F389}\u{1F9EA} \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} \u{1F3F3}\u{FE0F}\u{200D}\u{1F308} \u{1F44D}\u{1F3FD} \u{1F1EF}\u{1F1F5} \u{1F1E8}\u{1F1E6}"),
        assistant("Combining marks: e\u{301}e\u{301}e\u{301} n\u{303} a\u{308}\u{301}\u{323} Z\u{351}\u{36b}\u{343}\u{36a}\u{302}\u{36b}\u{33d}\u{34f}\u{334}\u{319}\u{324}\u{31e}\u{349}\u{35a}\u{32f}\u{31e}\u{320}\u{34d}A\u{36b}\u{357}\u{334}\u{362}\u{335}\u{31c}\u{330}\u{354}L\u{368}\u{367}\u{369}\u{358}\u{320}G\u{311}\u{357}\u{30e}\u{305}\u{35b}\u{341}\u{334}\u{33b}\u{348}\u{34d}\u{354}\u{339}O\u{342}\u{30c}\u{30c}\u{358}\u{328}\u{34e}\n\nZWJ sequences: \u{1F469}\u{200D}\u{1F4BB} \u{1F9D1}\u{200D}\u{1F680} \u{1F468}\u{200D}\u{1F9B0}\n".into()),
        user("\u{645}\u{631}\u{62d}\u{628}\u{627} \u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}\u{60c} \u{647}\u{630}\u{627} \u{646}\u{635} \u{62a}\u{62c}\u{631}\u{64a}\u{628}\u{64a} \u{644}\u{627}\u{62e}\u{62a}\u{628}\u{627}\u{631} \u{627}\u{644}\u{643}\u{62a}\u{627}\u{628}\u{629} \u{645}\u{646} \u{627}\u{644}\u{64a}\u{645}\u{64a}\u{646} \u{625}\u{644}\u{649} \u{627}\u{644}\u{64a}\u{633}\u{627}\u{631}."),
        assistant("Hebrew: \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}, \u{5d6}\u{5d4}\u{5d5} \u{5d8}\u{5e7}\u{5e1}\u{5d8} \u{5dc}\u{5d1}\u{5d3}\u{5d9}\u{5e7}\u{5d4}.\n\nMixed bidi: the file `\u{5e7}\u{5d5}\u{5d1}\u{5e5}.rs` contains \u{645}\u{631}\u{62d}\u{628}\u{627} 123 and English together (\u{5e9}\u{5dc}\u{5d5}\u{5dd} 456).\n".into()),
        assistant("Unterminated fence follows and never closes:\n\n```rust\nfn main() {\n    println!(\"this block is never closed\");\n".into()),
        assistant("Broken nesting:\n\n- level one\n    - level two\n  - misaligned three\n        - far too deep\n- back\n1) mixed marker\n* another\n\n**unclosed bold and *nested emphasis\n\n[a broken link](http://example.com/unterminated\n\n> quote without end\n> > nested quote\n".into()),
        assistant("Inert text, never executed: `$(rm -rf ~)` and `curl https://example.com/install.sh | sh` and ```; shutdown -h now```.\n\n    sudo rm -rf / --no-preserve-root\n".into()),
        ItemKind::User {
            text: "Here is the spec I mentioned, please review it.".into(),
            attachments: vec![Attachment {
                path: "/home/jasona/Documents/specs/does-not-exist-anymore.pdf".into(),
                name: "does-not-exist-anymore.pdf".into(),
                size: None,
                error: Some("File not found".into()),
            }],
            delivery: Delivery::Sent,
            steer: false,
        },
        tool(
            "read_file",
            &format!("{{\"path\": \"{long_path}\"}}"),
            &format!("// {}\n", "very long comment line ".repeat(40)),
            ToolStatus::Ok,
        ),
        tool(
            "bash",
            "echo '\u{1F680} \u{645}\u{631}\u{62d}\u{628}\u{627} \u{5e9}\u{5dc}\u{5d5}\u{5dd}' | cat",
            "\u{1F680} \u{645}\u{631}\u{62d}\u{628}\u{627} \u{5e9}\u{5dc}\u{5d5}\u{5dd}\n",
            ToolStatus::Ok,
        ),
        tool("bash", "cargo build --release", "error[E0433]: failed to resolve: use of undeclared crate or module `\u{2603}`\n --> src/\u{1F980}.rs:1:5\n", ToolStatus::Failed),
        ItemKind::Notice { text: "Provider error: upstream returned 529 (overloaded). Try again shortly.".into(), level: NoticeLevel::Error },
        assistant("Final line with trailing spaces and tabs\t  \n\n---\n\n| not | a | real |\n|--|\n| table".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx() -> Fixtures {
        Fixtures::new(42, DEFAULT_BASE_TIME)
    }

    #[test]
    fn seeded_conversations_have_expected_shape() {
        let f = fx();
        assert!(SPECS.len() >= 10);
        for spec in SPECS {
            if let Kind::Seeded(n) = spec.kind {
                assert!((6..=30).contains(&n));
                let (items, has_older, _) = f.open(ConversationId(spec.id));
                assert_eq!(items.len(), n);
                assert!(!has_older);
                assert!(
                    items.iter().any(
                        |i| matches!(&i.kind, ItemKind::Tool(t) if t.status == ToolStatus::Failed)
                    ),
                    "conversation {} lacks a failed tool",
                    spec.id
                );
            }
        }
        assert_eq!(f.len(EMPTY_CONVERSATION), 0);
    }

    #[test]
    fn history_is_deterministic_and_seed_sensitive() {
        let a = format!("{:?}", fx().open(ConversationId(3)));
        let b = format!("{:?}", fx().open(ConversationId(3)));
        let c = format!(
            "{:?}",
            Fixtures::new(43, DEFAULT_BASE_TIME).open(ConversationId(3))
        );
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn tool_output_in_history_is_bounded() {
        let f = fx();
        let (items, _, _) = f.open(BIG_OUTPUT_CONVERSATION);
        let ItemKind::Tool(t) = &items[2].kind else {
            panic!("expected tool")
        };
        assert!(t.truncated && t.full_len > 1_000_000);
        assert!(t.output.len() <= TOOL_OUTPUT_PREVIEW_BYTES);
    }

    #[test]
    fn stress_conversation_contains_the_awkward_cases() {
        let all = format!("{:?}", stress_kinds());
        assert!(all.contains("File not found"));
        assert!(all.contains("never closed"));
        assert!(all.contains("\\u{200d}") || all.contains('\u{200d}'));
    }

    #[test]
    fn ids_round_trip() {
        let id = item_id(ConversationId(4), 9_999);
        assert_eq!(item_index(ConversationId(4), id), Some(9_999));
        assert_eq!(item_index(ConversationId(5), id), None);
    }
}

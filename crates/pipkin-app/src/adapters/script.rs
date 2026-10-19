//! Versioned scenario scripts: a small JSON step DSL embedded at build time.
//!
//! A scenario file looks like:
//!
//! ```json
//! { "version": 1, "name": "normal", "description": "...",
//!   "variants": [ { "label": "...", "steps": [ { "op": "accepted" }, ... ] } ] }
//! ```
//!
//! Submissions in one conversation walk through `variants` in order (wrapping), which is how
//! `failure` rejects the first attempt and accepts the retry. Any step may carry
//! `"if_steered": true|false` to run only after (or only without) a steer.

use std::collections::VecDeque;

use pipkin_core::EventKind;
use serde::Deserialize;

use super::rng::Rng;
use super::story;

pub const SCRIPT_VERSION: u32 = 1;
const DEFAULT_PACE_MS: u64 = 45;
const LOG_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `description` and `label` document the fixture files.
pub struct ScenarioFile {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Variant {
    #[serde(default)]
    pub label: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub if_steered: Option<bool>,
    #[serde(flatten)]
    pub op: StepOp,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StepOp {
    /// Pause the script (scaled by the speed option).
    DelayMs {
        ms: u64,
    },
    Accepted,
    Rejected {
        reason: String,
    },
    /// The acknowledgment is lost; the submission becomes "outcome unknown".
    AckLost,
    /// Block until the client asks `CheckStatus`, then resolve it as accepted.
    AwaitStatus,
    /// One literal token event.
    Token {
        text: String,
    },
    /// Text delivered as several token events of 1-6 words each.
    Stream {
        text: String,
        pace_ms: Option<u64>,
    },
    ToolStart {
        call: u32,
        name: String,
        input: String,
        /// The engine-style call id, when a later `jobs` step links a job to this call.
        #[serde(default)]
        call_id: Option<String>,
    },
    ToolOutput {
        call: u32,
        text: String,
    },
    /// Generated output of `bytes` bytes, delivered in 64 KiB chunks.
    ToolOutputGen {
        call: u32,
        bytes: usize,
    },
    ToolFinish {
        call: u32,
        ok: bool,
    },
    /// A named generated change set: `fix_search` or `large_diff`.
    Changes {
        set: String,
    },
    Complete,
    Fail {
        message: String,
    },
    /// Pause for a steer. Waits the timeout (scaled), or forever when none is given.
    AwaitSteer {
        timeout_ms: Option<u64>,
    },
    /// Splice in another scenario's first variant.
    Include {
        scenario: String,
    },
    /// The engine's job list as of this step: a background job appears, runs and finishes.
    /// Its times are relative to the step: `age_ms` since it started, `ran_ms` for one that
    /// has ended. The demo stamps them against the clock when the step runs.
    Jobs {
        jobs: Vec<ScriptJob>,
    },
}

/// One job in a `jobs` step.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptJob {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    /// A `JobState` key: `starting`, `running`, `succeeded`, `failed`, ...
    pub state: String,
    /// The `call_id` of the `tool_start` that started it, so the job takes that row's place.
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub log_bytes: u64,
    #[serde(default)]
    pub age_ms: u64,
    #[serde(default)]
    pub ran_ms: Option<u64>,
}

impl ScriptJob {
    /// The job with relative times: `started_at_ms` is `-age_ms`, to be offset by the clock.
    fn to_job(&self) -> pipkin_core::Job {
        let started = -(self.age_ms as i64);
        pipkin_core::Job {
            id: self.id,
            name: self.name.clone(),
            command: self.command.clone(),
            cwd: self
                .cwd
                .clone()
                .unwrap_or_else(|| "/home/you/project".into()),
            origin: pipkin_core::JobOrigin::Model,
            state: pipkin_core::JobState::from_key(&self.state).unwrap_or_default(),
            exit_code: self.exit_code,
            reason: None,
            call_id: self.call_id.clone(),
            task: None,
            wake: true,
            delivery: if self.ran_ms.is_some() {
                "woken".into()
            } else {
                "pending".into()
            },
            log_bytes: self.log_bytes,
            log_truncated: false,
            created_at_ms: started,
            started_at_ms: Some(started),
            ended_at_ms: self.ran_ms.map(|ran| started + ran as i64),
        }
    }
}

/// Turn the relative job times of a scripted snapshot into clock times.
pub fn stamp_jobs(jobs: &mut pipkin_core::JobsSync, now_ms: i64) {
    for job in &mut jobs.jobs {
        job.created_at_ms += now_ms;
        if let Some(s) = &mut job.started_at_ms {
            *s += now_ms;
        }
        if let Some(e) = &mut job.ended_at_ms {
            *e += now_ms;
        }
    }
}

/// What the runner executes. `Step`s expand into these.
#[derive(Debug, Clone)]
pub enum Action {
    Sleep(u64),
    Emit(Box<EventKind>),
    AwaitSteer(Option<u64>),
    AwaitStatus,
}

/// Placeholders substituted into script text.
#[derive(Default, Clone)]
pub struct Subst {
    pub prompt: String,
    pub steer: String,
    pub model: String,
}

impl Subst {
    fn apply(&self, text: &str) -> String {
        text.replace("{prompt}", &self.prompt)
            .replace("{steer}", &self.steer)
            .replace("{model}", &self.model)
    }
}

pub const SCENARIOS: &[(&str, &str, &str)] = &[
    (
        "subagents",
        "Visual-only synthetic child tasks: Counter A running, Counter B completed, Counter C failed",
        include_str!("../../../../fixtures/scenarios/subagents.json"),
    ),
    (
        "normal",
        "Fix a failing test: streaming, tools, a failed then passing test run, three changed files",
        include_str!("../../../../fixtures/scenarios/normal.json"),
    ),
    (
        "jobs",
        "A background job started by the model: visible in the conversation while it runs, then its result",
        include_str!("../../../../fixtures/scenarios/jobs.json"),
    ),
    (
        "followup",
        "~20 s run with steer points; queue follow-ups and cancel",
        include_str!("../../../../fixtures/scenarios/followup.json"),
    ),
    (
        "failure",
        "Provider rejection (retry succeeds), then a tool failure that ends in Failed",
        include_str!("../../../../fixtures/scenarios/failure.json"),
    ),
    (
        "unknown",
        "Acknowledgment lost; Check status resolves it, then the run continues",
        include_str!("../../../../fixtures/scenarios/unknown.json"),
    ),
    (
        "stressed",
        "Unicode, emoji, RTL, long tokens and malformed Markdown in the reply",
        include_str!("../../../../fixtures/scenarios/stressed.json"),
    ),
    (
        "large",
        "A very large diff and a tool output over 1 MB",
        include_str!("../../../../fixtures/scenarios/large.json"),
    ),
    (
        "persist-fail",
        "Behaves as normal; use Developer > storage failure to inject write errors",
        include_str!("../../../../fixtures/scenarios/persist-fail.json"),
    ),
];

pub fn scenario_names() -> Vec<&'static str> {
    SCENARIOS.iter().map(|s| s.0).collect()
}

/// Parse and validate one scenario document. Unknown versions are rejected.
pub fn parse(source: &str) -> Result<ScenarioFile, String> {
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let header: Header =
        serde_json::from_str(source).map_err(|e| format!("invalid scenario: {e}"))?;
    if header.version != SCRIPT_VERSION {
        return Err(format!(
            "unsupported scenario version {} (this build understands {SCRIPT_VERSION})",
            header.version
        ));
    }
    let file: ScenarioFile =
        serde_json::from_str(source).map_err(|e| format!("invalid scenario: {e}"))?;
    if file.variants.is_empty() {
        return Err(format!("scenario {} has no variants", file.name));
    }
    Ok(file)
}

/// Parsed, include-resolved variants of a built-in scenario.
pub fn load(name: &str) -> Result<Vec<Vec<Step>>, String> {
    load_depth(name, 0)
}

fn load_depth(name: &str, depth: usize) -> Result<Vec<Vec<Step>>, String> {
    if depth > 4 {
        return Err(format!("include nesting too deep at {name}"));
    }
    let (_, _, source) = SCENARIOS
        .iter()
        .find(|s| s.0 == name)
        .ok_or_else(|| format!("unknown scenario {name:?}"))?;
    let file = parse(source)?;
    let mut out = Vec::new();
    for variant in file.variants {
        let mut steps = Vec::new();
        for step in variant.steps {
            match step.op {
                StepOp::Include { scenario } => {
                    let included = load_depth(&scenario, depth + 1)?;
                    steps.extend(included.into_iter().next().unwrap_or_default());
                }
                _ => steps.push(step),
            }
        }
        out.push(steps);
    }
    Ok(out)
}

/// Expand one step into runnable actions. `steered` selects `if_steered` branches.
pub fn expand(
    step: &Step,
    steered: bool,
    subst: &Subst,
    rng: &mut Rng,
    out: &mut VecDeque<Action>,
) {
    if step.if_steered.is_some_and(|want| want != steered) {
        return;
    }
    let emit = |k: EventKind| Action::Emit(Box::new(k));
    match &step.op {
        StepOp::DelayMs { ms } => out.push_back(Action::Sleep(*ms)),
        StepOp::Accepted => out.push_back(emit(EventKind::Accepted)),
        StepOp::Rejected { reason } => out.push_back(emit(EventKind::Rejected {
            reason: subst.apply(reason),
        })),
        StepOp::AckLost => out.push_back(emit(EventKind::AckLost)),
        StepOp::AwaitStatus => out.push_back(Action::AwaitStatus),
        StepOp::Token { text } => out.push_back(emit(EventKind::Token(subst.apply(text)))),
        StepOp::Stream { text, pace_ms } => {
            let pace = pace_ms.unwrap_or(DEFAULT_PACE_MS);
            for chunk in chunk_words(&subst.apply(text), rng) {
                out.push_back(emit(EventKind::Token(chunk)));
                out.push_back(Action::Sleep(pace));
            }
        }
        StepOp::ToolStart {
            call,
            name,
            input,
            call_id,
        } => out.push_back(emit(EventKind::ToolStarted {
            call: *call,
            name: name.clone(),
            input: subst.apply(input),
            call_id: call_id.clone(),
        })),
        StepOp::ToolOutput { call, text } => out.push_back(emit(EventKind::ToolOutput {
            call: *call,
            chunk: subst.apply(text),
        })),
        StepOp::ToolOutputGen { call, bytes } => {
            let mut left = *bytes;
            while left > 0 {
                let n = left.min(LOG_CHUNK_BYTES);
                let mut chunk = story::log_text(rng, n);
                // `log_text` overshoots by up to a line; trim to a char boundary at `n`.
                let mut cut = n.min(chunk.len());
                while !chunk.is_char_boundary(cut) {
                    cut -= 1;
                }
                chunk.truncate(cut);
                out.push_back(emit(EventKind::ToolOutput { call: *call, chunk }));
                left -= n;
            }
        }
        StepOp::ToolFinish { call, ok } => out.push_back(emit(EventKind::ToolFinished {
            call: *call,
            ok: *ok,
        })),
        StepOp::Changes { set } => {
            let changes = match set.as_str() {
                "large_diff" => story::large_diff(rng.next_u64()),
                _ => story::fix_search_changes(),
            };
            out.push_back(emit(EventKind::ChangesReported(changes)));
        }
        StepOp::Jobs { jobs } => {
            let jobs: Vec<pipkin_core::Job> = jobs.iter().map(ScriptJob::to_job).collect();
            let active = jobs.iter().filter(|j| j.state.active()).count();
            out.push_back(emit(EventKind::JobsSynced {
                jobs: pipkin_core::JobsSync {
                    jobs,
                    active,
                    truncated: false,
                },
            }));
        }
        StepOp::Complete => out.push_back(emit(EventKind::Completed)),
        StepOp::Fail { message } => out.push_back(emit(EventKind::Failed {
            message: subst.apply(message),
        })),
        StepOp::AwaitSteer { timeout_ms } => out.push_back(Action::AwaitSteer(*timeout_ms)),
        StepOp::Include { .. } => {}
    }
}

/// Split text into chunks of 1-6 whitespace-separated words, preserving every byte.
pub fn chunk_words(text: &str, rng: &mut Rng) -> Vec<String> {
    let pieces: Vec<&str> = text.split_inclusive(' ').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let n = rng.range(1, 6).min(pieces.len() - i);
        out.push(pieces[i..i + n].concat());
        i += n;
    }
    out
}

#[cfg(test)]
/// Virtual duration of a variant at speed 1.0, counting sleeps and timed steer waits.
pub fn nominal_duration_ms(steps: &[Step]) -> u64 {
    let mut rng = Rng::from_parts(&[1]);
    let mut total = 0;
    for step in steps {
        let mut actions = VecDeque::new();
        expand(step, false, &Subst::default(), &mut rng, &mut actions);
        // Count both branches of steered steps once, using the unsteered path.
        for a in actions {
            match a {
                Action::Sleep(ms) => total += ms,
                Action::AwaitSteer(Some(ms)) => total += ms,
                _ => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_scenario_parses() {
        for (name, _, _) in SCENARIOS {
            let v = load(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!v.is_empty() && v.iter().all(|s| !s.is_empty()), "{name}");
        }
    }

    #[test]
    fn unknown_versions_are_rejected() {
        let doc = r#"{"version": 2, "name": "x", "variants": [{"steps": [{"op": "complete"}]}]}"#;
        let err = parse(doc).unwrap_err();
        assert!(err.contains("unsupported scenario version 2"), "{err}");
        assert!(parse("{\"name\": \"x\"}").is_err());
    }

    #[test]
    fn chunking_preserves_text_and_respects_limits() {
        let text =
            "one two three four five six seven eight nine ten eleven twelve thirteen\nfourteen";
        let mut rng = Rng::from_parts(&[5]);
        let chunks = chunk_words(text, &mut rng);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|c| c.split_inclusive(' ').count() <= 6));
        assert!(chunks.len() >= 3);
    }

    #[test]
    fn followup_runs_about_twenty_seconds() {
        let steps = &load("followup").unwrap()[0];
        let ms = nominal_duration_ms(steps);
        assert!((15_000..=30_000).contains(&ms), "{ms} ms");
    }

    #[test]
    fn if_steered_selects_branches() {
        let step = Step {
            if_steered: Some(true),
            op: StepOp::Complete,
        };
        let mut rng = Rng::from_parts(&[1]);
        let mut q = VecDeque::new();
        expand(&step, false, &Subst::default(), &mut rng, &mut q);
        assert!(q.is_empty());
        expand(&step, true, &Subst::default(), &mut rng, &mut q);
        assert_eq!(q.len(), 1);
    }
}

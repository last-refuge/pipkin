//! The engine-wide activity directory behind the orchestration dashboard (protocol 42).
//!
//! Every read here is store-only: it never opens, recovers or attaches a session, never runs
//! extensions or CLI probes, and never builds a session runtime. Rows are keyed by the worker's
//! own session id, roots and children alike. A page reports `through`, the durable outbox cursor
//! its read transaction saw, so a client can tail `CatchUp` from there and refetch changed rows
//! with `ActivityRows` (see `docs/orchestration-dashboard-contracts.md` §4.3).

use serde::{Deserialize, Serialize};

use crate::execution::{ExecutionBackend, LocalCliMode};
use crate::view::{RunOutcome, RunPhase, SessionId};

/// `Activity`, `ActivityRows`.
pub const CAPABILITY_ACTIVITY_DIRECTORY: &str = "activityDirectory";
/// `ObserveHistory`, `ObserveChildren`.
pub const CAPABILITY_ACTIVITY_OBSERVE: &str = "activityObserve";
/// `StopRun`.
pub const CAPABILITY_GUARDED_STOP: &str = "guardedStop";
/// `FollowUpSubagent`; advertised only while subagents are enabled.
pub const CAPABILITY_SUBAGENT_FOLLOW_UP: &str = "subagentFollowUp";

/// The engine's normalised state of one worker. Clients never infer it from output quietness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum WorkState {
    /// A child exists; its first run has not started.
    Pending,
    /// A root holds an admitted queue entry but no active run.
    Queued,
    Running,
    Waiting {
        reason: WaitReason,
    },
    Stopping,
    /// A child finished its run; its result is not recorded yet.
    Completing,
    /// A root is idle after its last run completed, or has never run.
    Ready,
    /// A child ended successfully. Terminal.
    Completed,
    /// A root's last run failed (it still accepts a prompt), or a child failed (terminal).
    Failed {
        #[serde(default)]
        detail: Option<String>,
    },
    /// A root's last run was aborted (it still accepts a prompt), or a child was stopped.
    Stopped,
    /// The run was settled as interrupted, for example by an engine restart.
    Interrupted,
    /// The session cannot run here (provider or CLI readiness). Never shown as running.
    Blocked {
        reason: String,
    },
    /// A state this client does not know. It offers no controls for it.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitReason {
    /// An open question in the session's live runtime.
    Question,
    /// A pending Claude CLI approval.
    CliApproval,
    /// A child waiting for input.
    Input,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerKind {
    Root,
    Child,
}

/// One tab of the directory. Each worker is in exactly one of Attention, Running and Finished,
/// by that precedence; `All` lists every worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityGroup {
    Overview,
    Attention,
    Running,
    /// Terminal children and roots with a settled run, including older results.
    Finished,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFilter {
    /// Client-local acknowledged failure revisions; never stored by the engine.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub seen_failures: std::collections::BTreeMap<String, u64>,
    /// Exact match on the normalised working directory.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub kind: Option<WorkerKind>,
    #[serde(default)]
    pub backend: Option<ExecutionBackend>,
    /// Case-folded substring of title, task excerpt or cwd, at most
    /// [`crate::limits::MAX_ACTIVITY_TEXT_CHARS`].
    #[serde(default)]
    pub text: Option<String>,
    /// Only workers whose last run failed or was interrupted. The client uses this to find
    /// failures the person has not seen yet, which it counts as needing attention.
    #[serde(default)]
    pub failed: bool,
}

/// An opaque keyset position. Only the engine that issued it can interpret it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActivityCursor(pub String);

/// Engine-wide counts for the page's filter, read in the same transaction as the rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCounts {
    pub attention: u32,
    pub running: u32,
    pub finished_recent: u32,
    /// Failed or interrupted within the recent window (a subset of `finished_recent`).
    #[serde(default)]
    pub failed_recent: u32,
    pub total: u32,
    /// Background jobs; never counted as agents.
    pub jobs_active: u32,
}

/// The current run, or the last settled one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBrief {
    pub id: u64,
    /// Set while the run is open.
    #[serde(default)]
    pub phase: Option<RunPhase>,
    /// Set once the run settled.
    #[serde(default)]
    pub outcome: Option<RunOutcome>,
    pub started_at_ms: i64,
    #[serde(default)]
    pub ended_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    /// The worker's own session.
    pub session: SessionId,
    /// `sessions.revision` of this worker; a refetch replaces a row only when this grows.
    pub revision: u64,
    pub kind: WorkerKind,
    pub root: SessionId,
    /// The real parent of a child (`parent_session`), which may itself be a child.
    #[serde(default)]
    pub parent: Option<SessionId>,
    /// The child's number within its root; with `root` it is the control authority.
    #[serde(default)]
    pub child: Option<u64>,
    pub cwd: String,
    #[serde(default)]
    pub title: Option<String>,
    /// Bounded by [`crate::limits::SUBAGENT_TASK_EXCERPT_CHARS`].
    #[serde(default)]
    pub task_excerpt: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    /// A child's origin: `model` or `user`.
    #[serde(default)]
    pub origin: Option<String>,
    /// The child this one is a follow-up of.
    #[serde(default)]
    pub continues: Option<u64>,
    #[serde(default)]
    pub execution_backend: ExecutionBackend,
    #[serde(default)]
    pub local_cli_mode: Option<LocalCliMode>,
    #[serde(default)]
    pub model: Option<String>,
    pub state: WorkState,
    /// A child's stored status, verbatim, so an `Unknown` state can still be described.
    #[serde(default)]
    pub raw_status: Option<String>,
    #[serde(default)]
    pub run: Option<RunBrief>,
    #[serde(default)]
    pub children_active: u32,
    #[serde(default)]
    pub jobs_active: u32,
    #[serde(default)]
    pub has_result: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// The group-specific ordering anchor (contracts §4.2.1).
    pub sort_at_ms: i64,
}

/// Parts of the system the directory cannot see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoverageGap {
    /// Workers the Claude CLI runs internally.
    CliInternalWorkers,
    /// Agents launched outside Pipkin.
    ExternalAgents,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Every root session is enumerable.
    pub roots: bool,
    /// Every child task is enumerable (no row cap).
    pub children: bool,
    /// Open questions of live runtimes were overlaid.
    pub live_attention: bool,
    #[serde(default)]
    pub excluded: Vec<CoverageGap>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    pub rows: Vec<ActivityRow>,
    #[serde(default)]
    pub next: Option<ActivityCursor>,
    pub counts: ActivityCounts,
    /// The durable outbox cursor visible to the read.
    pub through: u64,
    pub coverage: Coverage,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::api::{Call, Lane, Reply};

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: &T,
    ) -> Value {
        let json = serde_json::to_value(value).unwrap();
        let back: T = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(&back, value);
        json
    }

    fn row() -> ActivityRow {
        ActivityRow {
            session: "c1".into(),
            revision: 7,
            kind: WorkerKind::Child,
            root: "r1".into(),
            parent: Some("r1".into()),
            child: Some(3),
            cwd: "/p".into(),
            title: None,
            task_excerpt: Some("check the tests".into()),
            profile: Some("worker".into()),
            origin: Some("user".into()),
            continues: Some(2),
            execution_backend: ExecutionBackend::NativeAgent,
            local_cli_mode: None,
            model: Some("p/m".into()),
            state: WorkState::Waiting {
                reason: WaitReason::Input,
            },
            raw_status: Some("waiting".into()),
            run: Some(RunBrief {
                id: 11,
                phase: Some(RunPhase::Running),
                outcome: None,
                started_at_ms: 100,
                ended_at_ms: None,
            }),
            children_active: 1,
            jobs_active: 0,
            has_result: false,
            created_at_ms: 90,
            updated_at_ms: 120,
            sort_at_ms: 100,
        }
    }

    #[test]
    fn protocol_is_42() {
        assert_eq!(crate::PROTOCOL_VERSION, 42);
    }

    #[test]
    fn calls_have_their_wire_names() {
        let json = round_trip(&Call::Activity {
            filter: ActivityFilter {
                seen_failures: Default::default(),
                cwd: Some("/p".into()),
                kind: Some(WorkerKind::Root),
                backend: Some(ExecutionBackend::ClaudeCliAgent),
                text: Some("fix".into()),
                failed: true,
            },
            group: ActivityGroup::Attention,
            after: Some(ActivityCursor("opaque".into())),
            limit: 50,
        });
        assert_eq!(
            json,
            json!({
                "call": "activity",
                "filter": {"cwd": "/p", "kind": "root", "backend": "claudeCliAgent",
                           "text": "fix", "failed": true},
                "group": "attention",
                "after": "opaque",
                "limit": 50,
            })
        );
        let parsed: Call =
            serde_json::from_value(json!({"call": "activity", "group": "all", "limit": 1}))
                .unwrap();
        assert_eq!(
            parsed,
            Call::Activity {
                filter: ActivityFilter::default(),
                group: ActivityGroup::All,
                after: None,
                limit: 1,
            }
        );
        for (group, wire) in [
            (ActivityGroup::Attention, "attention"),
            (ActivityGroup::Running, "running"),
            (ActivityGroup::Finished, "finished"),
            (ActivityGroup::All, "all"),
        ] {
            assert_eq!(round_trip(&group), json!(wire));
        }

        let json = round_trip(&Call::ActivityRows {
            sessions: vec!["a".into(), "b".into()],
        });
        assert_eq!(
            json,
            json!({"call": "activityRows", "sessions": ["a", "b"]})
        );

        let json = round_trip(&Call::ObserveHistory {
            session: "c1".into(),
            before: Some(9),
            limit: 100,
        });
        assert_eq!(
            json,
            json!({"call": "observeHistory", "session": "c1", "before": 9, "limit": 100})
        );

        let json = round_trip(&Call::ObserveChildren {
            session: "r1".into(),
            after: None,
            limit: 500,
        });
        assert_eq!(
            json,
            json!({"call": "observeChildren", "session": "r1", "after": null, "limit": 500})
        );

        let json = round_trip(&Call::FollowUpSubagent {
            session: "r1".into(),
            child: 3,
            request: "k".into(),
            text: "and the docs".into(),
        });
        assert_eq!(
            json,
            json!({"call": "followUpSubagent", "session": "r1", "child": 3, "request": "k",
                   "text": "and the docs"})
        );
    }

    #[test]
    fn guarded_stop_is_a_control_call_and_plain_stop_is_unchanged() {
        let guarded = Call::StopRun {
            session: "r1".into(),
            expected_run: 4,
        };
        assert_eq!(
            round_trip(&guarded),
            json!({"call": "stopRun", "session": "r1", "expected_run": 4})
        );
        assert_eq!(guarded.lane(), Lane::Control);
        let plain: Call = serde_json::from_value(json!({"call": "stop", "session": "r1"})).unwrap();
        assert_eq!(
            plain,
            Call::Stop {
                session: "r1".into()
            }
        );
        assert!(
            serde_json::from_value::<Call>(json!({"call": "stopRun", "session": "r1"})).is_err(),
            "a guarded stop without its run is refused, never treated as unguarded"
        );
    }

    #[test]
    fn replies_round_trip() {
        let page = ActivityPage {
            rows: vec![row()],
            next: Some(ActivityCursor("n".into())),
            counts: ActivityCounts {
                attention: 1,
                running: 2,
                finished_recent: 3,
                failed_recent: 1,
                total: 9,
                jobs_active: 4,
            },
            through: 77,
            coverage: Coverage {
                roots: true,
                children: true,
                live_attention: true,
                excluded: vec![CoverageGap::CliInternalWorkers, CoverageGap::ExternalAgents],
            },
        };
        let json = round_trip(&Reply::Activity(page));
        assert_eq!(json["reply"], "activity");
        assert_eq!(json["next"], "n");
        assert_eq!(json["counts"]["finishedRecent"], 3);
        assert_eq!(json["counts"]["jobsActive"], 4);
        assert_eq!(
            json["coverage"]["excluded"],
            json!(["cliInternalWorkers", "externalAgents"])
        );
        let r = &json["rows"][0];
        assert_eq!(r["state"], json!({"state": "waiting", "reason": "input"}));
        assert_eq!(r["kind"], "child");
        assert_eq!(r["executionBackend"], "nativeAgent");
        assert_eq!(r["run"]["phase"], "running");
        assert_eq!(r["sortAtMs"], 100);

        let json = round_trip(&Reply::ActivityRows {
            rows: vec![row()],
            missing: vec!["gone".into()],
            through: 5,
        });
        assert_eq!(json["reply"], "activityRows");
        assert_eq!(json["missing"], json!(["gone"]));

        let json = round_trip(&Reply::ObservedChildren {
            rows: vec![row()],
            next: Some(3),
            through: 5,
        });
        assert_eq!(json["reply"], "observedChildren");
    }

    #[test]
    fn every_state_has_a_stable_wire_name() {
        let cases = [
            (WorkState::Pending, json!({"state": "pending"})),
            (WorkState::Queued, json!({"state": "queued"})),
            (WorkState::Running, json!({"state": "running"})),
            (
                WorkState::Waiting {
                    reason: WaitReason::Question,
                },
                json!({"state": "waiting", "reason": "question"}),
            ),
            (
                WorkState::Waiting {
                    reason: WaitReason::CliApproval,
                },
                json!({"state": "waiting", "reason": "cliApproval"}),
            ),
            (WorkState::Stopping, json!({"state": "stopping"})),
            (WorkState::Completing, json!({"state": "completing"})),
            (WorkState::Ready, json!({"state": "ready"})),
            (WorkState::Completed, json!({"state": "completed"})),
            (
                WorkState::Failed {
                    detail: Some("boom".into()),
                },
                json!({"state": "failed", "detail": "boom"}),
            ),
            (WorkState::Stopped, json!({"state": "stopped"})),
            (WorkState::Interrupted, json!({"state": "interrupted"})),
            (
                WorkState::Blocked {
                    reason: "sign in".into(),
                },
                json!({"state": "blocked", "reason": "sign in"}),
            ),
            (WorkState::Unknown, json!({"state": "unknown"})),
        ];
        for (state, wire) in cases {
            assert_eq!(round_trip(&state), wire);
        }
        assert_eq!(round_trip(&CoverageGap::Other), json!("other"));
    }

    #[test]
    fn unknown_values_decode_fail_closed() {
        let state: WorkState = serde_json::from_value(json!({"state": "hibernating"})).unwrap();
        assert_eq!(state, WorkState::Unknown);
        let state: WorkState =
            serde_json::from_value(json!({"state": "waiting", "reason": "telepathy"})).unwrap();
        assert_eq!(
            state,
            WorkState::Waiting {
                reason: WaitReason::Other
            }
        );
        let state: WorkState = serde_json::from_value(json!({"state": "failed"})).unwrap();
        assert_eq!(state, WorkState::Failed { detail: None });
        let gap: CoverageGap = serde_json::from_value(json!("satellites")).unwrap();
        assert_eq!(gap, CoverageGap::Other);

        let mut json = serde_json::to_value(row()).unwrap();
        json["executionBackend"] = json!("someFutureLoop");
        json.as_object_mut().unwrap().remove("continues");
        json.as_object_mut().unwrap().remove("rawStatus");
        let parsed: ActivityRow = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.execution_backend, ExecutionBackend::Unsupported);
        assert_eq!(parsed.continues, None);
        assert_eq!(parsed.raw_status, None);
    }

    #[test]
    fn cursors_are_opaque_strings() {
        assert_eq!(
            round_trip(&ActivityCursor("g1:00ab:s".into())),
            json!("g1:00ab:s")
        );
        assert!(serde_json::from_value::<ActivityCursor>(json!({"group": 1})).is_err());
    }
}

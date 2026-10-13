//! Goals (plan §9, GOAL-1): the pure parts of the engine-owned goal runner. The turn prompt
//! and the status-marker contract are the desktop runner's, kept byte-identical so a model
//! that worked with the old runner works with this one; GOAL-2 replaces the marker with
//! structured updates.
//!
//! A verdict is only ever what the executor's final line says. No marker is "ambiguous" and
//! pauses the goal; the runner never guesses.

/// The marker lines the executor ends a goal turn with.
pub const MARK_CONTINUE: &str = "[PIPKIN_GOAL_CONTINUE]";
pub const MARK_MET: &str = "[PIPKIN_GOAL_MET]";
pub const MARK_BLOCKED: &str = "[PIPKIN_GOAL_BLOCKED]";
pub const MARK_UNREACHABLE: &str = "[PIPKIN_GOAL_UNREACHABLE]";

/// The prefix of every goal turn's prompt (the desktop hides the rest in its transcript).
pub const PROMPT_PREFIX: &str = "Pursue this ongoing goal: ";

/// Pause and state reasons, as shown to the person.
pub const REASON_STOP: &str = "Stop requested.";
pub const REASON_MET: &str = "Reported met by the executor; not independently verified.";
pub const REASON_BLOCKED: &str = "Blocked; input needed.";
pub const REASON_UNREACHABLE: &str = "Reported unreachable by the executor.";
pub const REASON_AMBIGUOUS: &str = "Paused: the engine did not give a clear goal status.";
pub const REASON_RESTART: &str = "Paused after an engine restart; resume to continue.";
pub const REASON_DETACHED: &str = "Paused: no window is attached to this conversation.";
pub const REASON_USER_INPUT: &str = "Paused: you sent a message; resume to continue the goal.";
pub const REASON_PAUSED_BY_PERSON: &str = "Paused by you.";
pub const REASON_REPLACED: &str = "Replaced by a new goal.";
pub const REASON_ARCHIVED: &str = "Archived by you.";
pub const REASON_MIGRATED: &str = "Paused after migration from the desktop; resume to continue.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Continue,
    Met,
    Blocked,
    Unreachable,
    /// No recognised marker on the final line.
    Ambiguous,
}

/// The executor's verdict: exactly the last non-blank line of its final reply.
pub fn verdict(final_text: &str) -> Verdict {
    match final_text.trim_end().lines().last().map(str::trim) {
        Some(MARK_CONTINUE) => Verdict::Continue,
        Some(MARK_MET) => Verdict::Met,
        Some(MARK_BLOCKED) => Verdict::Blocked,
        Some(MARK_UNREACHABLE) => Verdict::Unreachable,
        _ => Verdict::Ambiguous,
    }
}

/// The prompt of one goal turn. Byte-identical to the desktop runner's.
pub fn turn_prompt(objective: &str) -> String {
    format!(
        "{PROMPT_PREFIX}{objective}\n\nKeep working until it is met, blocked, or unreachable. At the end of your response, put exactly one status marker on its own final line: {MARK_CONTINUE} if there is more work you can do now; {MARK_MET} if done; {MARK_BLOCKED} if you need the user's input; {MARK_UNREACHABLE} if it cannot be done. Explain the result or blocker before that marker. Never claim work was done unless it was."
    )
}

/// The request key of a goal turn: one per `(goal, epoch)`, so a replayed schedule finds its
/// admission and an old goal's turn can never be mistaken for a replacement's.
pub fn request_key(goal: u64, epoch: u64) -> String {
    format!("goal:{goal}:{epoch}")
}

/// Whether `key` is the request key of one of `goal`'s turns.
pub fn is_turn_of(key: &str, goal: u64) -> bool {
    key.strip_prefix(&format!("goal:{goal}:"))
        .is_some_and(|epoch| epoch.parse::<u64>().is_ok())
}

/// The objective shown for a goal-turn prompt, when the prompt is one.
pub fn objective_of_prompt(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(PROMPT_PREFIX)?;
    Some(rest.split("\n\n").next().unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_is_only_the_final_line() {
        assert_eq!(verdict("done\n[PIPKIN_GOAL_MET]\n"), Verdict::Met);
        assert_eq!(
            verdict("more\n  [PIPKIN_GOAL_CONTINUE]  "),
            Verdict::Continue
        );
        assert_eq!(verdict("[PIPKIN_GOAL_BLOCKED]"), Verdict::Blocked);
        assert_eq!(
            verdict("x\n[PIPKIN_GOAL_UNREACHABLE]"),
            Verdict::Unreachable
        );
        assert_eq!(
            verdict("[PIPKIN_GOAL_MET] but then more text"),
            Verdict::Ambiguous
        );
        assert_eq!(
            verdict("[PIPKIN_GOAL_MET]\nafterthought"),
            Verdict::Ambiguous
        );
        assert_eq!(verdict(""), Verdict::Ambiguous);
    }

    #[test]
    fn prompts_keys_and_objectives_round_trip() {
        let prompt = turn_prompt("ship v1");
        assert!(prompt.starts_with("Pursue this ongoing goal: ship v1\n\nKeep working"));
        assert_eq!(objective_of_prompt(&prompt), Some("ship v1"));
        assert_eq!(objective_of_prompt("hello"), None);
        assert_eq!(request_key(3, 7), "goal:3:7");
    }

    #[test]
    fn turn_keys_are_recognised_per_goal() {
        assert!(is_turn_of(&request_key(3, 7), 3));
        assert!(!is_turn_of(&request_key(3, 7), 4));
        assert!(!is_turn_of("goal:3:", 3));
        assert!(!is_turn_of("goal:3:x", 3));
        assert!(!is_turn_of("r1", 3));
    }
}

// ------------------------------------------------------------- GOAL-2: structured progress

use pipkin_engine_protocol::limits::{
    MAX_CRITERION_EVIDENCE, MAX_GOAL_CRITERIA, MAX_GOAL_TEXT_CHARS,
};
use pipkin_engine_protocol::{CriterionStatus, GoalCriterion, GoalEvidence};

pub const REASON_PROPOSED: &str =
    "Completion proposed by the executor with accepted evidence for every criterion; not audited.";
pub const REASON_TURNS_USED: &str = "Turn allowance used; raise the limit to continue.";
pub const REASON_TOKENS_USED: &str = "Token allowance used (checked before each turn, so the last turn may have overshot); raise the limit to continue.";
pub const REASON_COST_USED: &str = "Cost allowance used (checked before each turn from the reported cost, so the last turn may have overshot); raise the limit to continue.";
pub const REASON_NO_STATUS: &str = "Paused: the executor reported no goal status this turn (use update_goal, or a final status marker).";
/// Default consecutive no-progress turns before pausing (`goals.stallTurns`).
pub const DEFAULT_STALL_TURNS: u32 = 3;

/// The prompt of a goal turn in this version: structured reporting first, the marker as a
/// still-accepted fallback. The prefix is unchanged so transcripts keep showing the objective.
pub fn turn_prompt_structured(objective: &str, criteria: &[GoalCriterion]) -> String {
    let mut prompt = format!(
        "{PROMPT_PREFIX}{objective}\n\nKeep working until it is met, blocked, or unreachable. Report with the goal tools: call update_goal with status \"continue\" (and a short note) when there is more you can do now, \"blocked\" when you need the user's input, or \"unreachable\" when it cannot be done; call propose_goal_completion with evidence for every criterion when it is met. Attach evidence as tool call ids, commands you ran, files (with their hash) or clearly labelled observations. Never claim work was done unless it was. If you cannot call the tools, put exactly one status marker on your final line instead: {MARK_CONTINUE}, {MARK_MET}, {MARK_BLOCKED} or {MARK_UNREACHABLE}."
    );
    if !criteria.is_empty() {
        prompt.push_str("\n\nCompletion criteria:");
        for c in criteria {
            prompt.push_str(&format!("\n- #{} [{}] {}", c.id, c.status.as_str(), c.text));
        }
    }
    prompt
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Validate and bound a text field.
pub fn bounded(field: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if utf16_len(value) > MAX_GOAL_TEXT_CHARS {
        return Err(format!(
            "{field} can be at most {MAX_GOAL_TEXT_CHARS} characters"
        ));
    }
    Ok(value.to_owned())
}

/// Criteria from the person's list: open, numbered from 1.
pub fn criteria_from_texts(texts: &[String]) -> Result<Vec<GoalCriterion>, String> {
    if texts.len() > MAX_GOAL_CRITERIA {
        return Err(format!(
            "a goal can have at most {MAX_GOAL_CRITERIA} criteria"
        ));
    }
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            Ok(GoalCriterion {
                id: i as u64 + 1,
                text: bounded("criterion", t)?,
                status: CriterionStatus::Open,
                evidence: Vec::new(),
                judged_at_revision: 0,
            })
        })
        .collect()
}

/// What the engine can establish about a tool call named as evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallCheck {
    /// No such call in this session.
    Missing,
    /// The call ran and its result was not an error.
    Succeeded,
    /// The call's result was an error, or it was interrupted.
    Failed,
    /// The call has no result yet.
    Open,
}

/// Check one evidence item deterministically where possible; the result is written into
/// `accepted`/`verdict`/`judgment`. `call` answers for tool call ids; `cwd` resolves file
/// paths.
pub fn check_evidence(
    evidence: &mut GoalEvidence,
    call: &dyn Fn(&str) -> CallCheck,
    cwd: &std::path::Path,
) {
    evidence.judgment = false;
    let (accepted, verdict) = match evidence.kind.as_str() {
        "toolCall" | "command" | "test" => match call(evidence.reference.trim()) {
            CallCheck::Succeeded => (
                true,
                "the tool call exists in this session and did not fail".to_owned(),
            ),
            CallCheck::Failed => (false, "the tool call failed or was interrupted".to_owned()),
            CallCheck::Open => (false, "the tool call has no result yet".to_owned()),
            CallCheck::Missing => (false, "no such tool call in this session".to_owned()),
        },
        "file" => {
            let path = std::path::Path::new(evidence.reference.trim());
            let full = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            if !full.starts_with(cwd) {
                (
                    false,
                    "the path is outside the session's directory".to_owned(),
                )
            } else {
                match std::fs::read(&full) {
                    Err(_) => (
                        false,
                        "the file does not exist or cannot be read".to_owned(),
                    ),
                    Ok(bytes) => match &evidence.sha256 {
                        None => (
                            true,
                            "the file exists (no hash was given to pin its content)".to_owned(),
                        ),
                        Some(expected) => {
                            let actual = crate::web::sha256_hex(&bytes);
                            if actual.eq_ignore_ascii_case(expected.trim()) {
                                (
                                    true,
                                    "the file exists and its content matches the hash".to_owned(),
                                )
                            } else {
                                (
                                    false,
                                    "stale: the file's content no longer matches the hash"
                                        .to_owned(),
                                )
                            }
                        }
                    },
                }
            }
        }
        "observation" => {
            evidence.judgment = true;
            (
                true,
                "an observation the engine cannot check; counted as judgment, not proof".to_owned(),
            )
        }
        other => (
            false,
            format!("unknown evidence kind {other:?} (toolCall, command, test, file, observation)"),
        ),
    };
    evidence.accepted = Some(accepted);
    evidence.verdict = Some(verdict);
}

/// Apply a claim to a criterion: `met` holds only with at least one accepted mechanical
/// item or an observation (which leaves it `unverified`, labelled judgment); anything else
/// is `unverified`.
pub fn judge_criterion(criterion: &mut GoalCriterion, claimed: CriterionStatus, revision: u64) {
    criterion.judged_at_revision = revision;
    criterion.status = match claimed {
        CriterionStatus::Met => {
            let mechanical = criterion
                .evidence
                .iter()
                .any(|e| e.accepted == Some(true) && !e.judgment);
            if mechanical {
                CriterionStatus::Met
            } else {
                CriterionStatus::Unverified
            }
        }
        other => other,
    };
}

/// Every criterion must be `met` for a completion to be accepted; reports what is not.
pub fn unmet_criteria(criteria: &[GoalCriterion]) -> Vec<String> {
    criteria
        .iter()
        .filter(|c| c.status != CriterionStatus::Met)
        .map(|c| format!("#{} [{}] {}", c.id, c.status.as_str(), c.text))
        .collect()
}

/// A revision of the objective or criteria invalidates every met claim.
pub fn mark_stale(criteria: &mut [GoalCriterion]) {
    for c in criteria {
        if c.status == CriterionStatus::Met {
            c.status = CriterionStatus::Unverified;
        }
    }
}

/// A durable progress signature: criteria statuses and evidence counts, the note, linked
/// tasks and the plan revision. Equal signatures across continued turns mean no progress.
pub fn progress_signature(
    criteria: &[GoalCriterion],
    note: Option<&str>,
    linked_tasks: &[u64],
    plan_revision: u64,
) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in criteria {
        (c.id, c.status.as_str(), c.evidence.len()).hash(&mut h);
    }
    note.hash(&mut h);
    linked_tasks.hash(&mut h);
    plan_revision.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Bound a batch of evidence items.
pub fn check_evidence_count(existing: usize, added: usize) -> Result<(), String> {
    if existing + added > MAX_CRITERION_EVIDENCE {
        return Err(format!(
            "a criterion can carry at most {MAX_CRITERION_EVIDENCE} evidence items"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod structured_tests {
    use super::*;

    fn ev(kind: &str, reference: &str, sha: Option<&str>) -> GoalEvidence {
        GoalEvidence {
            kind: kind.into(),
            reference: reference.into(),
            note: None,
            sha256: sha.map(str::to_owned),
            accepted: None,
            verdict: None,
            judgment: false,
        }
    }

    #[test]
    fn evidence_is_checked_mechanically_and_observations_are_judgment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), b"hello").unwrap();
        let hash = crate::web::sha256_hex(b"hello");
        let call = |id: &str| match id {
            "ok" => CallCheck::Succeeded,
            "bad" => CallCheck::Failed,
            "open" => CallCheck::Open,
            _ => CallCheck::Missing,
        };
        let mut e = ev("toolCall", "ok", None);
        check_evidence(&mut e, &call, dir.path());
        assert_eq!(e.accepted, Some(true));
        for (id, needle) in [
            ("bad", "failed"),
            ("open", "no result"),
            ("nope", "no such"),
        ] {
            let mut e = ev("command", id, None);
            check_evidence(&mut e, &call, dir.path());
            assert_eq!(e.accepted, Some(false), "{id}");
            assert!(e.verdict.as_deref().unwrap().contains(needle), "{id}");
        }
        let mut e = ev("file", "out.txt", Some(&hash));
        check_evidence(&mut e, &call, dir.path());
        assert_eq!(e.accepted, Some(true));
        let mut e = ev("file", "out.txt", Some("deadbeef"));
        check_evidence(&mut e, &call, dir.path());
        assert_eq!(e.accepted, Some(false));
        assert!(e.verdict.as_deref().unwrap().starts_with("stale"));
        let mut e = ev("file", "../etc/passwd", None);
        check_evidence(&mut e, &call, dir.path());
        assert_eq!(e.accepted, Some(false));
        let mut e = ev("file", "missing.txt", None);
        check_evidence(&mut e, &call, dir.path());
        assert_eq!(e.accepted, Some(false));
        let mut e = ev("observation", "it looks right", None);
        check_evidence(&mut e, &call, dir.path());
        assert_eq!((e.accepted, e.judgment), (Some(true), true));
        let mut e = ev("vibes", "x", None);
        check_evidence(&mut e, &call, dir.path());
        assert_eq!(e.accepted, Some(false));
    }

    #[test]
    fn met_needs_mechanical_evidence_and_revisions_make_claims_stale() {
        let mut c = criteria_from_texts(&["tests pass".into(), "docs updated".into()]).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id, 1);
        judge_criterion(&mut c[0], CriterionStatus::Met, 1);
        assert_eq!(
            c[0].status,
            CriterionStatus::Unverified,
            "no evidence at all"
        );
        let mut obs = ev("observation", "looked fine", None);
        obs.accepted = Some(true);
        obs.judgment = true;
        c[0].evidence.push(obs);
        judge_criterion(&mut c[0], CriterionStatus::Met, 1);
        assert_eq!(
            c[0].status,
            CriterionStatus::Unverified,
            "judgment alone is not proof"
        );
        let mut mech = ev("toolCall", "ok", None);
        mech.accepted = Some(true);
        c[0].evidence.push(mech);
        judge_criterion(&mut c[0], CriterionStatus::Met, 1);
        assert_eq!(c[0].status, CriterionStatus::Met);
        assert_eq!(unmet_criteria(&c), ["#2 [open] docs updated"]);
        mark_stale(&mut c);
        assert_eq!(c[0].status, CriterionStatus::Unverified);
        assert!(criteria_from_texts(&vec!["x".into(); MAX_GOAL_CRITERIA + 1]).is_err());
        assert!(criteria_from_texts(&["  ".into()]).is_err());
        let a = progress_signature(&c, Some("n"), &[1], 1);
        assert_eq!(a, progress_signature(&c, Some("n"), &[1], 1));
        assert_ne!(a, progress_signature(&c, Some("n2"), &[1], 1));
        assert_ne!(a, progress_signature(&c, Some("n"), &[1], 2));
    }
}

// ---------------------------------------------------------------- GOAL-3: completion audit

use pipkin_engine_protocol::{AuditFinding, GoalView};

pub const DEFAULT_AUDIT_ROUNDS: u32 = 2;

/// The parent call id under which an audit round's child session is created.
pub fn audit_call_id(goal: u64, round: u32) -> String {
    format!("audit:{goal}:{round}")
}
pub const REASON_AUDIT_RUNNING: &str =
    "Verifying: an independent auditor is checking the completion claim.";
pub const REASON_AUDIT_STOPPED: &str =
    "Paused: the audit was stopped; resume and propose completion again.";
pub const REASON_AUDIT_RESTART: &str =
    "Paused after an engine restart during an audit; resume and propose completion again.";
pub const REASON_AUDIT_STALE: &str = "Paused: files the completion claim stood on changed during the audit; propose completion again.";
pub const REASON_AUDIT_SKIPPED: &str =
    "Completed without audit: skipped by the person (unreviewed).";

/// The auditor's structured verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditVerdict {
    /// `approved`, `changes_requested` or `inconclusive`.
    pub verdict: String,
    pub findings: Vec<AuditFinding>,
}

/// Parse the auditor's reply: the last JSON object in the text with a `verdict` field.
/// Anything else is `None`, which the runner treats as inconclusive, never as approval.
pub fn parse_audit_verdict(text: &str) -> Option<AuditVerdict> {
    let mut end = text.len();
    while let Some(close) = text[..end].rfind('}') {
        // Walk back to the matching open brace.
        let mut depth = 0i32;
        let mut start = None;
        for (i, ch) in text[..=close].char_indices().rev() {
            match ch {
                '}' => depth += 1,
                '{' => {
                    depth -= 1;
                    if depth == 0 {
                        start = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(start) = start else { break };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text[start..=close])
            && let Some(verdict) = value.get("verdict").and_then(serde_json::Value::as_str)
        {
            let verdict = match verdict.trim() {
                "approved" => "approved",
                "changes_requested" | "changes-requested" | "changes requested" => {
                    "changes_requested"
                }
                "inconclusive" => "inconclusive",
                _ => return None,
            };
            let findings = value
                .get("findings")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|f| {
                            Some(AuditFinding {
                                criterion: f.get("criterion")?.as_u64()?,
                                ok: f
                                    .get("ok")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false),
                                note: f
                                    .get("note")
                                    .and_then(serde_json::Value::as_str)
                                    .map(|n| n.chars().take(MAX_GOAL_TEXT_CHARS).collect()),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            return Some(AuditVerdict {
                verdict: verdict.to_owned(),
                findings,
            });
        }
        end = start;
    }
    None
}

/// The auditor's task: the claim and everything it must check, as text.
pub fn audit_prompt(goal: &GoalView, round: u32, summary: &str) -> String {
    let mut out = format!(
        "Audit round {round}: another agent claims this goal is complete. Check the claim yourself; do not take its word.\n\nObjective: {}\n\nExecutor's summary of what was done: {summary}\n\nCriteria and the evidence the executor cited:",
        goal.objective
    );
    for c in &goal.criteria {
        out.push_str(&format!("\n- #{} [{}] {}", c.id, c.status.as_str(), c.text));
        for e in &c.evidence {
            let mark = match (e.accepted, e.judgment) {
                (Some(true), true) => "judgment",
                (Some(true), false) => "engine-accepted",
                (Some(false), _) => "engine-rejected",
                (None, _) => "unchecked",
            };
            out.push_str(&format!(
                "\n    · {mark} {} {}{}",
                e.kind,
                e.reference,
                e.note
                    .as_deref()
                    .map(|n| format!(" — {n}"))
                    .unwrap_or_default()
            ));
        }
    }
    if let Some(note) = &goal.note {
        out.push_str(&format!("\n\nExecutor's last note: {note}"));
    }
    out.push_str("\n\nUse read, list_files and search_files to verify each criterion in the workspace. Report one finding per criterion.");
    out
}

/// A fingerprint of what the claim stands on: every accepted file evidence's current
/// content and the plan revision. If it changes while the auditor works, the verdict is stale.
pub fn claim_fingerprint(goal: &GoalView, cwd: &std::path::Path, plan_revision: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    plan_revision.hash(&mut h);
    goal.revision.hash(&mut h);
    for c in &goal.criteria {
        for e in &c.evidence {
            if e.kind == "file" {
                let path = std::path::Path::new(e.reference.trim());
                let full = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                };
                match std::fs::read(&full) {
                    Ok(bytes) => crate::web::sha256_hex(&bytes).hash(&mut h),
                    Err(_) => "missing".hash(&mut h),
                }
            }
        }
    }
    format!("{:016x}", h.finish())
}

/// The findings of the last audit, for the next turn's prompt.
pub fn findings_section(goal: &GoalView) -> Option<String> {
    let result = goal.audit.result.as_ref()?;
    if result.verdict != "changes_requested" {
        return None;
    }
    let mut out = format!(
        "\n\nAn independent audit (round {}) did not approve your last completion proposal. Address these findings before proposing again:",
        result.round
    );
    for f in &result.findings {
        out.push_str(&format!(
            "\n- criterion #{}: {}{}",
            f.criterion,
            if f.ok { "ok" } else { "not met" },
            f.note
                .as_deref()
                .map(|n| format!(" — {n}"))
                .unwrap_or_default()
        ));
    }
    if let Some(summary) = &result.summary {
        out.push_str(&format!("\nAuditor's summary: {summary}"));
    }
    Some(out)
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn verdicts_are_parsed_from_the_last_json_object_and_never_invented() {
        let text = "I checked things.\n{\"verdict\": \"approved\", \"findings\": [{\"criterion\": 1, \"ok\": true, \"note\": \"tests pass\"}]}";
        let v = parse_audit_verdict(text).unwrap();
        assert_eq!(v.verdict, "approved");
        assert_eq!(v.findings.len(), 1);
        assert!(v.findings[0].ok);
        let v = parse_audit_verdict("{\"verdict\": \"approved\"} then {\"verdict\": \"changes requested\", \"findings\": [{\"criterion\": 2, \"ok\": false}]}").unwrap();
        assert_eq!(v.verdict, "changes_requested");
        assert!(!v.findings[0].ok);
        assert_eq!(parse_audit_verdict("looks good to me"), None);
        assert_eq!(parse_audit_verdict("{\"verdict\": \"yes\"}"), None);
        assert_eq!(parse_audit_verdict("{\"verdict\": \"approved\""), None);
        assert_eq!(parse_audit_verdict("{\"findings\": []}"), None);
        let v =
            parse_audit_verdict("{\"verdict\": \"inconclusive\", \"findings\": \"n/a\"}").unwrap();
        assert!(v.findings.is_empty());
    }

    #[test]
    fn fingerprints_follow_file_evidence_and_the_plan_revision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one").unwrap();
        let mut goal = GoalView {
            id: 1,
            objective: "o".into(),
            revision: 1,
            state: pipkin_engine_protocol::GoalState::Active,
            reason: None,
            turns: 0,
            epoch: 0,
            provenance: "user".into(),
            last_request: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            usage_cost_usd: 0.0,
            criteria: vec![GoalCriterion {
                id: 1,
                text: "c".into(),
                status: CriterionStatus::Met,
                evidence: vec![GoalEvidence {
                    kind: "file".into(),
                    reference: "a.txt".into(),
                    note: None,
                    sha256: None,
                    accepted: Some(true),
                    verdict: None,
                    judgment: false,
                }],
                judged_at_revision: 1,
            }],
            limits: Default::default(),
            usage_tokens: 0,
            stall_turns: 0,
            note: None,
            linked_tasks: vec![],
            decision: None,
            audit: Default::default(),
        };
        let a = claim_fingerprint(&goal, dir.path(), 1);
        assert_eq!(a, claim_fingerprint(&goal, dir.path(), 1));
        assert_ne!(a, claim_fingerprint(&goal, dir.path(), 2));
        std::fs::write(dir.path().join("a.txt"), "two").unwrap();
        assert_ne!(a, claim_fingerprint(&goal, dir.path(), 1));
        goal.audit.result = Some(pipkin_engine_protocol::GoalAuditResult {
            round: 1,
            verdict: "changes_requested".into(),
            findings: vec![AuditFinding {
                criterion: 1,
                ok: false,
                note: Some("no".into()),
            }],
            summary: Some("not yet".into()),
            fingerprint: None,
            child: None,
        });
        let section = findings_section(&goal).unwrap();
        assert!(section.contains("criterion #1: not met — no"));
        assert!(section.contains("Auditor's summary: not yet"));
        assert!(audit_prompt(&goal, 1, "did it").contains("engine-accepted file a.txt"));
    }
}

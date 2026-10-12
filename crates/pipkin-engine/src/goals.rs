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
}

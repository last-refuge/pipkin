//! Explicit reapproval of an old, evidence-backed clean Stop. No prompt/evidence rewrite.
use super::*;
use prompts::{ExternalPromptReceipt, ExternalTerminalOutcome};

impl Store {
    fn legacy_clean_stop_in(c: &Connection, receipt: &ExternalPromptReceipt) -> Result<bool> {
        Ok(receipt.state == ExternalRunState::Interrupted
            && receipt.outcome == Some(RunOutcome::Aborted)
            && matches!(&receipt.result, Some(PromptResult::Unanswered { reason, .. }) if reason == "aborted")
            && receipt.evidence.as_ref().is_some_and(|e| e.clean_cancelled_session(receipt.cursor))
            // Absent in the older build. Explicit false in a newer receipt is NOT old evidence.
            && c.query_row("SELECT json_type(evidence,'$.continuation_authorized') IS NULL FROM external_prompts WHERE id=?1", [receipt.id], |r| r.get::<_, bool>(0))?)
    }

    /// Read-only eligibility, not permission to execute or to restore a connection grant.
    pub fn cli_clean_stop_recovery(&self, session: &str) -> Result<Option<ExternalPromptReceipt>> {
        self.read(|c| Self::cli_clean_stop_recovery_in(c, session))
    }

    fn cli_clean_stop_recovery_in(
        c: &Connection,
        session: &str,
    ) -> Result<Option<ExternalPromptReceipt>> {
        let Some(binding) = Self::cli_binding_in(c, session)? else {
            return Ok(None);
        };
        if binding.context_valid
            || binding.identity.launch_policy != crate::execution::claude_cli::coding_policy::ID
        {
            return Ok(None);
        }
        let ids = c.prepare("SELECT p.id FROM external_runs r LEFT JOIN external_prompts p USING(session,generation,op) WHERE r.session=?1 ORDER BY p.id")?
            .query_map([session], |r| r.get::<_, Option<u64>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(Some(last)) = ids.last().copied() else {
            return Ok(None);
        };
        let stopped = Self::external_prompt_id_in(c, last)?;
        if !Self::legacy_clean_stop_in(c, &stopped)?
            || binding.generation != stopped.key.generation.saturating_add(1)
        {
            return Ok(None);
        }
        // A clean last Stop cannot hide an older unknown run, missing receipt, or recovery.
        for id in ids {
            let Some(id) = id else {
                return Ok(None);
            };
            let receipt = Self::external_prompt_id_in(c, id)?;
            if receipt.key.generation != stopped.key.generation {
                return Ok(None);
            }
            if id == last {
                continue;
            }
            let Some(e) = receipt.evidence.as_ref() else {
                return Ok(None);
            };
            let clean = e.projection_finished
                && e.input_delivered
                && e.root_reaped
                && e.pipes_drained
                && !e.cleanup_timed_out
                && !e.stop_requested
                && !e.cancelled_clean_confirmed
                && e.supervisor_stop.is_none()
                && e.projection_error.is_none()
                && e.rejected_record.is_none()
                && e.parse_failure.is_none();
            let success = receipt.state == ExternalRunState::Completed
                && receipt.outcome == Some(RunOutcome::Completed)
                && matches!(receipt.result, Some(PromptResult::Done { .. }))
                && receipt
                    .terminal
                    .as_ref()
                    .is_some_and(|t| t.outcome == ExternalTerminalOutcome::Success)
                && e.exit_code == Some(0);
            let provider_error = receipt.state == ExternalRunState::Interrupted
                && matches!(receipt.outcome, Some(RunOutcome::ModelError { .. }))
                && matches!(receipt.result, Some(PromptResult::Unanswered { .. }))
                && receipt
                    .terminal
                    .as_ref()
                    .is_some_and(|t| t.outcome == ExternalTerminalOutcome::Error)
                && matches!(e.exit_code, Some(0 | 1));
            if !clean || !(success || provider_error) {
                return Ok(None);
            }
        }
        let unresolved: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM external_permissions WHERE session=?1 AND state IN ('pending','granted')) OR EXISTS(SELECT 1 FROM external_actions WHERE session=?1 AND status IN ('observed','running')) OR EXISTS(SELECT 1 FROM cli_stop_recoveries WHERE session=?1) OR EXISTS(SELECT 1 FROM session_removals WHERE root=?1)",
            [session], |r| r.get(0))?;
        if unresolved {
            return Ok(None);
        }
        Ok(Some(stopped))
    }

    /// Caller must hold the current verified setup lease and recheck the entire captured
    /// identity against fresh CLI metadata. Recovery grants no execution by itself.
    pub fn recover_cli_clean_stop_in(
        tx: &Transaction<'_>,
        session: &str,
        expected: &CliBinding,
        setup: &str,
        now: i64,
    ) -> Result<CliBinding> {
        let current = Self::cli_binding_in(tx, session)?.ok_or(StoreError::ExecutionConflict)?;
        if &current != expected {
            return Err(StoreError::ExecutionConflict);
        }
        let receipt =
            Self::cli_clean_stop_recovery_in(tx, session)?.ok_or(StoreError::ExecutionConflict)?;
        let generation = current
            .generation
            .checked_add(1)
            .ok_or(StoreError::ExecutionConflict)?;
        tx.execute("INSERT INTO cli_stop_recoveries(prompt,session,cli_session_id,previous_generation,generation,setup,recovered_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![receipt.id,session,current.identity.cli_session_id,current.generation,generation,setup,now])?;
        let changed = tx.execute("UPDATE cli_bindings SET context_valid=1,generation=?3 WHERE session=?1 AND generation=?2 AND context_valid=0", params![session,current.generation,generation])?;
        if changed != 1 {
            return Err(StoreError::ExecutionConflict);
        }
        Self::bump(tx, session, now)?;
        Self::cli_binding_in(tx, session)?.ok_or(StoreError::ExecutionConflict)
    }

    pub(super) fn recovered_cli_stop_in(
        c: &Connection,
        receipt: &ExternalPromptReceipt,
        generation: u64,
    ) -> Result<bool> {
        if !Self::legacy_clean_stop_in(c, receipt)? {
            return Ok(false);
        }
        Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM cli_stop_recoveries a JOIN cli_bindings b USING(session,cli_session_id) WHERE a.prompt=?1 AND a.session=?2 AND a.previous_generation=?3 AND a.generation<=?4 AND b.context_valid=1 AND b.generation=?4)",
            params![receipt.id,receipt.key.session,receipt.key.generation+1,generation], |r| r.get(0))?)
    }
}

//! Goal accounting reads actual CLI receipts, never the API usage/pricing ledger.
use super::*;
const LINEAGE: &str = include_str!("goal_lineage.sql");
pub(crate) struct GoalUsage {
    pub known_tokens: u64,
    pub complete: bool,
    pub active: u64,
    pub uncertain: bool,
    pub execution_failed: bool,
    pub turns: u64,
    pub waiting: bool,
}
impl Store {
    pub(crate) fn cli_goal_usage_in(c: &Connection, session: &str, id: u64) -> Result<GoalUsage> {
        // A person may stop the current auditor by explicitly skipping its verdict.
        // Preserve its actual tokens/unknown usage, but do not turn that intentional
        // cancellation into a failure of the goal. Unrelated worker failures still pause.
        let skipped_auditor = Self::goal_current(c, session)?
            .filter(|g| g.id == id)
            .and_then(|g| g.audit.result)
            .filter(|r| r.verdict == "skipped")
            .and_then(|r| r.child)
            .map(|child| {
                c.query_row(
                    "SELECT child_session FROM subagent_tasks WHERE root=?1 AND child=?2",
                    params![session, child],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .transpose()?
            .flatten();
        let mut stmt=c.prepare(&format!("{LINEAGE} SELECT t.state,t.fenced,t.evidence,t.request_key,t.session FROM lineage l JOIN cli_agent_turns t ON t.session=l.session AND t.run=l.run WHERE l.goal_session=?1 AND l.goal_id=?2"))?;
        let mut rows = stmt.query(params![session, id])?;
        let mut usage = GoalUsage {
            known_tokens: 0,
            complete: true,
            active: 0,
            uncertain: false,
            execution_failed: false,
            turns: 0,
            waiting: false,
        };
        while let Some(row) = rows.next()? {
            let state: String = row.get(0)?;
            let fenced: bool = row.get(1)?;
            let evidence: Option<String> = row.get(2)?;
            let request: String = row.get(3)?;
            if request.starts_with("goal:")
                || request.starts_with("cli-job-wake:")
                || request.starts_with("cli-child-wake:")
            {
                usage.turns += 1;
            }
            if matches!(state.as_str(), "admitted" | "launchIntent" | "running") {
                usage.active += 1;
                usage.complete = false;
                continue;
            }
            // A rejected pre-launch reservation spent no inference tokens.
            if state == "rejected" && !fenced {
                continue;
            }
            let receipt_session: String = row.get(4)?;
            usage.execution_failed |= (state != "completed" || fenced)
                && skipped_auditor.as_deref() != Some(receipt_session.as_str());
            let terminal = evidence
                .as_deref()
                .map(|e| parse_json::<Completion>(e, "CLI goal receipt"))
                .transpose()?;
            let Some(tokens) = terminal.and_then(|e| e.terminal.usage) else {
                usage.complete = false;
                usage.uncertain = true;
                continue;
            };
            usage.complete &= state == "completed" && !fenced;
            usage.uncertain |= state != "completed" || fenced;
            for count in [
                tokens.input_tokens,
                tokens.output_tokens,
                tokens.cache_read_input_tokens,
                tokens.cache_creation_input_tokens,
            ] {
                match count {
                    Some(n) => {
                        let total = usage.known_tokens.saturating_add(n);
                        usage.complete &= total <= i64::MAX as u64;
                        usage.uncertain |= total > i64::MAX as u64;
                        usage.known_tokens = total.min(i64::MAX as u64);
                    }
                    None => {
                        usage.complete = false;
                        usage.uncertain = true;
                    }
                }
            }
        }
        usage.waiting = c.query_row(&format!("{LINEAGE} SELECT EXISTS(SELECT 1 FROM lineage l JOIN jobs j ON j.session=l.session AND j.run=l.run WHERE l.goal_session=?1 AND l.goal_id=?2 AND j.wake=1 AND j.delivery='pending') OR EXISTS(SELECT 1 FROM lineage l JOIN cli_tool_bridges b ON b.session=l.session AND b.run=l.run JOIN cli_bridge_calls bc ON bc.bridge=b.id JOIN subagent_tasks t ON t.parent_session=l.session WHERE l.goal_session=?1 AND l.goal_id=?2 AND (t.parent_call=bc.call_id OR EXISTS(SELECT 1 FROM cli_agent_turns child WHERE child.session=t.child_session AND child.request_key='child:'||bc.call_id AND child.run=(SELECT MAX(run) FROM cli_agent_turns WHERE session=t.child_session))) AND (t.status IN ('pending','running') OR t.delivery='wake'))"),params![session,id],|r|r.get(0))?;
        Ok(usage)
    }
    pub(crate) fn cli_run_goals_in(
        c: &Connection,
        session: &str,
        run: u64,
    ) -> Result<Vec<(String, u64)>> {
        let mut stmt=c.prepare(&format!("{LINEAGE} SELECT DISTINCT goal_session,goal_id FROM lineage WHERE session=?1 AND run=?2"))?;
        Ok(stmt
            .query_map(params![session, run], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    /// Admission-time allowance check. Active ancestor usage is not yet reported; it is
    /// distinct from a settled receipt with missing usage, which cannot fund another turn.
    pub(crate) fn cli_goal_allow_in(
        c: &Connection,
        session: &str,
        id: u64,
        autonomous: bool,
    ) -> Result<()> {
        let goal = Self::goal_current(c, session)?
            .filter(|g| g.id == id)
            .ok_or(StoreError::ExecutionConflict)?;
        let usage = Self::cli_goal_usage_in(c, session, id)?;
        if !(goal.state == GoalState::Active || !autonomous && goal.state == GoalState::Verifying)
            || goal.limits.max_cost_usd.is_some()
            || goal
                .limits
                .max_tokens
                .is_some_and(|n| usage.known_tokens.max(goal.usage_tokens) >= n || usage.uncertain)
            || autonomous
                && (usage.active > 0
                    || goal
                        .limits
                        .max_turns
                        .is_some_and(|n| usage.turns.max(goal.turns) >= n))
        {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
    pub(crate) fn cli_goal_allow_origin_in(
        c: &Connection,
        session: &str,
        run: u64,
        autonomous: bool,
    ) -> Result<()> {
        for (root, id) in Self::cli_run_goals_in(c, session, run)? {
            Self::cli_goal_allow_in(c, &root, id, autonomous)?;
        }
        Ok(())
    }
    /// Absolute reconciliation is idempotent, includes late receipts for archived goals,
    /// and cannot charge a replacement goal. Caller owns the publication gate.
    pub(crate) fn cli_goal_reconcile_run_in(
        c: &Connection,
        session: &str,
        run: u64,
        now: i64,
    ) -> Result<Vec<(String, GoalView, u64)>> {
        let mut updates = Vec::new();
        for (root, id) in Self::cli_run_goals_in(c, session, run)? {
            if let Some(update) = Self::cli_goal_reconcile_in(c, &root, id, now)? {
                updates.push(update);
            }
        }
        Ok(updates)
    }
    pub(crate) fn cli_goal_reconcile_in(
        c: &Connection,
        root: &str,
        id: u64,
        now: i64,
    ) -> Result<Option<(String, GoalView, u64)>> {
        let usage = Self::cli_goal_usage_in(c, root, id)?;
        let mut changed=c.execute("UPDATE goals SET usage_tokens=MAX(usage_tokens,?3),turns=MAX(turns,?4),updated_at=?5 WHERE session=?1 AND id=?2 AND (usage_tokens<?3 OR turns<?4)",params![root,id,usage.known_tokens,usage.turns,now])?;
        if usage.uncertain {
            const NOTE: &str = "CLI token usage is incomplete; the token count is a known minimum.";
            changed+=c.execute("UPDATE goals SET note=substr(COALESCE(note,''),1,2000)||char(10)||?3,updated_at=?4 WHERE session=?1 AND id=?2 AND instr(COALESCE(note,''),?3)=0",params![root,id,NOTE,now])?;
        }
        if usage.execution_failed
            && Self::goal_current(c, root)?
                .is_some_and(|g| g.id == id && g.state == GoalState::Completed)
        {
            Self::goal_set_state(
                c,
                root,
                id,
                GoalState::Paused,
                Some(
                    "Paused: background CLI execution did not finish with a confirmed receipt; inspect retained effects, do not replay.",
                ),
                now,
            )?;
            changed += 1;
        }
        if changed > 0
            && let Some(goal) = Self::goal_current(c, root)?.filter(|g| g.id == id)
        {
            return Ok(Some((root.into(), goal, Self::bump(c, root, now)?)));
        }
        Ok(None)
    }
    pub(crate) fn cli_goal_recover_usage_in(c: &Connection, now: i64) -> Result<()> {
        let goals=c.prepare("SELECT g.session,g.id FROM goals g JOIN sessions s ON s.id=g.session WHERE s.execution_backend='claudeCliAgent'")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (root, id) in goals {
            Self::cli_goal_reconcile_in(c, &root, id, now)?;
        }
        Ok(())
    }
    pub(crate) fn cli_goal_answer(&self, session: &str, request: &str) -> Result<String> {
        self.read(|c| {
            let turn=stored(c,session,request)?.ok_or(StoreError::ExecutionConflict)?;
            if turn.state!=State::Completed || turn.fenced {return Err(StoreError::ExecutionConflict);}
            let mut stmt=c.prepare("SELECT * FROM entries WHERE session=?1 AND run=?2 AND retired=0 AND partial=0 AND length(CAST(body AS BLOB))<=8388608 ORDER BY id DESC LIMIT 128")?;
            for row in stmt.query_map(params![session,turn.scope.run],Self::entry_from_row)? {
                let (entry,_)=row?;
                if let EntryKind::Message {message:m @ Message::Assistant {..},partial:false,..}=entry.kind {return Ok(m.text());}
            }
            Ok(String::new())
        })
    }
}

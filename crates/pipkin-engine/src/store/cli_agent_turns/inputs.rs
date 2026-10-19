//! Durable input data. Only an original connected InputTicket can consume it.
use super::*;
#[allow(dead_code)] // Private route; public activation remains gated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    FollowUp,
    Steer,
    Compact,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::FollowUp => "followUp",
            Self::Steer => "steer",
            Self::Compact => "compact",
        }
    }
    pub fn mode(self) -> InputMode {
        if self == Self::Steer {
            InputMode::Steer
        } else {
            InputMode::FollowUp
        }
    }
}
#[derive(Clone)]
pub(crate) struct Queued {
    pub session: String,
    pub request: String,
    pub text: String,
    pub images: Vec<ImageInput>,
    pub kind: Kind,
    pub generation: u64,
    pub operation: String,
    pub ticket: String,
}
impl Queued {
    pub fn validate(&self, c: &Connection, input: &Input<'_>) -> Result<()> {
        let q = strict_request(c, &self.session, &self.request)?
            .ok_or(StoreError::ExecutionConflict)?;
        let current:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_inputs WHERE session=?1 AND request_key=?2 AND ticket=?3 AND generation=?4 AND operation=?5 AND kind=?6)",params![self.session,self.request,self.ticket,self.generation,self.operation,self.kind.name()],|r|r.get(0))?;
        if !current
            || input.session != self.session
            || input.request != self.request
            || input.text != self.text
            || input.images != self.images
            || input.generation != self.generation
            || input.operation != self.operation
            || input.mode != self.kind.mode()
            || q.state != RequestState::Queued
            || q.text != self.text
            || q.mode != self.kind.mode()
            || q.images != self.images
            || q.fingerprint != fingerprint(self.kind.mode(), &self.text, &self.images)
            || q.run.is_some()
            || q.outcome.is_some()
        {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
}
pub(super) fn next(c: &Connection, session: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT key FROM requests WHERE session=?1 AND state='queued' ORDER BY CASE mode WHEN 'steer' THEN 0 ELSE 1 END, queue_entry LIMIT 1",[session],|r|r.get(0)).optional()?)
}
pub(super) fn assert_compacted(c: &Connection, r: &Receipt) -> Result<()> {
    let missing:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_inputs i WHERE i.session=?1 AND i.request_key=?2 AND i.kind='compact') AND NOT EXISTS(SELECT 1 FROM cli_agent_context WHERE run=?3 AND compact_count>0)",params![r.scope.session,r.request,r.scope.run],|r|r.get(0))?;
    if missing {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
impl Store {
    /// Exact repeats are lookup only: the caller must not create another ticket/actor.
    pub(crate) fn queue_cli_input(
        &self,
        b: &cli_agent::Binding,
        q: &Queued,
        now: i64,
    ) -> Result<(RequestRow, bool)> {
        if q.session != b.create.session
            || !external::token(&q.request)
            || !external::token(&q.operation)
            || q.generation == 0
            || q.generation > i64::MAX as u64
            || input_data::validate(&q.text, &q.images).is_err()
            || (q.kind == Kind::Compact && !q.images.is_empty())
        {
            return Err(StoreError::ExecutionConflict);
        }
        self.tx(|c| {
            live(c,&q.session,b.digest)?;
            if let Some(old)=strict_request(c,&q.session,&q.request)? {
                let same:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_inputs WHERE session=?1 AND request_key=?2 AND generation=?3 AND operation=?4 AND kind=?5)",params![q.session,q.request,q.generation,q.operation,q.kind.name()],|r|r.get(0))?;
                if !same || old.text!=q.text || old.images!=q.images || old.mode!=q.kind.mode() {return Err(StoreError::ExecutionConflict);}
                return Ok((old,false));
            }
            let unsafe_history:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND (fenced=1 OR state='unknown'))",[&q.session],|r|r.get(0))?;
            let prior:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND state='completed')",[&q.session],|r|r.get(0))?;
            if unsafe_history || (q.kind==Kind::Compact && !prior) || Self::queue_in(c,&q.session)?.len()>=MAX_QUEUE {return Err(StoreError::ExecutionConflict);}
            let entry:u64=c.query_row("SELECT COALESCE(MAX(queue_entry),0)+1 FROM requests WHERE session=?1",[&q.session],|r|r.get(0))?;
            c.execute("INSERT INTO requests(session,key,fingerprint,mode,state,queue_entry,text,images,model,created_at) VALUES(?1,?2,?3,?4,'queued',?5,?6,?9,?7,?8)",params![q.session,q.request,fingerprint(q.kind.mode(),&q.text,&q.images),mode_str(q.kind.mode()),entry,q.text,b.create.model,now,json(&q.images)])?;
            c.execute("INSERT INTO cli_agent_inputs VALUES(?1,?2,?3,?4,?5,?6)",params![q.session,q.request,q.ticket,q.generation,q.operation,q.kind.name()])?;
            Self::bump(c,&q.session,now)?;
            Ok((strict_request(c,&q.session,&q.request)?.ok_or(StoreError::ExecutionConflict)?,true))
        })
    }
    pub(crate) fn cli_input_is_next(&self, q: &Queued) -> Result<bool> {
        self.read(|c| Ok(next(c, &q.session)?.as_deref() == Some(&q.request)))
    }
    pub(crate) fn withdraw_cli_input(&self, q: &Queued) -> Result<()> {
        self.tx(|c| {
            let changed=c.execute("UPDATE requests SET state='withdrawn',outcome=?4 WHERE session=?1 AND key=?2 AND state='queued' AND EXISTS(SELECT 1 FROM cli_agent_inputs i WHERE i.session=?1 AND i.request_key=?2 AND i.ticket=?3)",params![q.session,q.request,q.ticket,json(&RunOutcome::Aborted)])?;
            if changed>0 {Self::bump(c,&q.session,now_ms())?;} Ok(())
        })
    }
    pub(crate) fn cli_context_usage(&self, session: &str) -> Result<ContextUsageView> {
        self.read(|c| {
            let row=c.query_row("SELECT * FROM sessions WHERE id=?1",[session],Self::session_from_row).optional()?.ok_or(StoreError::ExecutionConflict)?;
            let latest:Option<(String,Option<u64>,u64)>=c.query_row("SELECT CASE WHEN EXISTS(SELECT 1 FROM cli_agent_turns bad WHERE bad.session=t.session AND (bad.fenced=1 OR bad.state='unknown')) THEN 'unknown' ELSE t.state END,x.tokens,COALESCE(x.compact_count,0) FROM cli_agent_turns t LEFT JOIN cli_agent_context x ON x.run=t.run WHERE t.session=?1 ORDER BY t.run DESC LIMIT 1",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let tokens=latest.as_ref().filter(|(s,_,_)|s=="completed" && row.blocked.is_none()).and_then(|(_,n,_)|*n);
            Ok(ContextUsageView {tokens,source:if tokens.is_some(){"cli-reported-input"}else{"unavailable"}.into(),capacity:None,reserve:None,remaining:None,model:row.model,measured_entry:None,measured_model:None,stale:row.blocked.is_some() || latest.as_ref().is_some_and(|(s,_,_)|s!="completed"),compaction:if latest.as_ref().is_some_and(|(s,_,n)|s=="completed" && *n>0){"cli-compacted"}else{"cli-managed"}.into(),revision:row.revision})
        })
    }
}

/// The caller holds original ticket authority and commits this with either root admission
/// or the actual native tool receipt. A wire write is never itself an acknowledgment.
pub(crate) fn consume(
    c: &Connection,
    q: &Queued,
    scope: &Scope,
    call: Option<&str>,
    now: i64,
) -> Result<SessionChange> {
    if q.session != scope.session || q.kind == Kind::Compact {
        return Err(StoreError::ExecutionConflict);
    }
    q.validate(
        c,
        &Input {
            session: &q.session,
            request: &q.request,
            generation: q.generation,
            operation: &q.operation,
            mode: q.kind.mode(),
            text: &q.text,
            images: &q.images,
            binding: [0; 32],
        },
    )?;
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND run=?2 AND generation=?3 AND operation=?4 AND fenced=0 AND state IN ('admitted','launchIntent','running'))",params![scope.session,scope.run,scope.generation,scope.operation],|r|r.get(0))?;
    if !valid {
        return Err(StoreError::ExecutionConflict);
    }
    Store::consume_queued(c, &q.session, std::slice::from_ref(&q.request), scope.run)?;
    let entry = Store::append_entry(
        c,
        &q.session,
        now,
        Some(scope.run),
        &EntryKind::Message {
            message: Message::User {
                content: input_data::content(&q.text, &q.images),
                timestamp: now,
                extra: Default::default(),
            },
            mode: q.kind.mode(),
            request: Some(q.request.clone()),
            partial: false,
        },
    )?;
    c.execute(
        "INSERT INTO cli_agent_input_deliveries VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            q.session,
            q.request,
            scope.run,
            entry.id,
            if call.is_some() { "tool" } else { "prompt" },
            call,
            q.text
        ],
    )?;
    Ok(SessionChange::EntryAppended { entry })
}
/// Preserve message boundaries and escape arbitrary user text; never interpret it as metadata.
pub(crate) fn envelope(messages: &[(String, String)]) -> String {
    format!(
        "queued messages from the user (JSON array of request ID and exact text; apply each in order):\n{}",
        serde_json::to_string(messages).expect("strings serialize")
    )
}
pub(super) fn prompt(c: &Connection, run: u64, primary: &str) -> Result<String> {
    let rows=c.prepare("SELECT request_key,text FROM cli_agent_input_deliveries WHERE run=?1 AND channel='prompt' ORDER BY entry")?
        .query_map([run],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(primary.into());
    }
    let key: String = c.query_row("SELECT request FROM runs WHERE id=?1", [run], |r| r.get(0))?;
    let mut messages = vec![(key, primary.into())];
    messages.extend(rows);
    let text = envelope(&messages);
    if text.len() > MAX_PROMPT_BYTES {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(text)
}
pub(super) fn settle(c: &Connection, r: &Receipt, outcome: &RunOutcome, now: i64) -> Result<()> {
    let rows = c
        .prepare("SELECT request_key,entry FROM cli_agent_input_deliveries WHERE run=?1")?
        .query_map([r.scope.run], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (key, entry) in rows {
        Store::settle_request(c, &r.scope.session, &key, outcome, now)?;
        if *outcome != RunOutcome::Completed {
            Store::retire_entries(c, &r.scope.session, &[entry])?;
        }
    }
    Ok(())
}
impl Store {
    pub(crate) fn cli_input_delivery(&self, q: &Queued) -> Result<Option<(Receipt, String)>> {
        self.read(|c| {
            let key:Option<(String,String)>=c.query_row("SELECT t.request_key,t.owner FROM cli_agent_input_deliveries d JOIN cli_agent_inputs i ON i.session=d.session AND i.request_key=d.request_key JOIN cli_agent_turns t ON t.run=d.run WHERE d.session=?1 AND d.request_key=?2 AND i.ticket=?3",params![q.session,q.request,q.ticket],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            key.map(|(key,owner)|Ok((stored(c,&q.session,&key)?.ok_or(StoreError::ExecutionConflict)?,owner))).transpose()
        })
    }
    pub(crate) fn cli_batch_entries(&self, run: u64) -> Result<Vec<pipkin_engine_protocol::Entry>> {
        self.read(|c| {
            let rows=c.prepare("SELECT session,entry FROM cli_agent_input_deliveries WHERE run=?1 AND channel='prompt' ORDER BY entry")?
                .query_map([run],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(|(s,id)|Self::entry_in(c,&s,id)?.ok_or(StoreError::ExecutionConflict)).collect()
        })
    }
}

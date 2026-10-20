//! Storage-only CLI binding and external-effect ledger. No process launch, credential probe,
//! native run, tool intent, retry, or executable transcript projection occurs here.
//!
//! `admit_external_run` returns `(record, newly_admitted)`: a repeated op NEVER grants a new
//! launch. Commit Admitted -> LaunchIntent BEFORE spawning. Completion is accepted only from
//! Running with all action metadata Completed/Failed, and the caller MUST have verified both
//! a valid final result and root process exit. `context_valid` alone is NOT admission or resume
//! authorization; Stopping is not eligible for new admission.
//! Call `recover_external_runs` at startup before allowing any execution; it never replays work.
use super::*;
pub mod permissions;
pub mod prompts;
mod stop_recovery;

/// Immutable, nonsecret identity. Config/account fields are opaque markers, NEVER credentials
/// or paths to credentials. The caller must qualify both paths against the actual filesystem;
/// storage validates canonical shape only and does no filesystem or secrets I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliBindingSpec {
    pub cli_session_id: String,
    pub project_path: String,
    pub executable_path: String,
    pub cli_version: String,
    pub config_ref: String,
    pub account_scope: String,
    pub launch_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliBinding {
    pub identity: CliBindingSpec,
    pub generation: u64,
    /// Necessary but not sufficient for admission/resume; consult the run ledger as well.
    pub context_valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRunKey {
    pub session: String,
    pub generation: u64,
    pub op: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalRunState {
    Admitted,
    LaunchIntent,
    Running,
    Stopping,
    Completed,
    Interrupted,
    Unknown,
}

impl ExternalRunState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::LaunchIntent => "launchIntent",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
        }
    }

    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "admitted" => Ok(Self::Admitted),
            "launchIntent" => Ok(Self::LaunchIntent),
            "running" => Ok(Self::Running),
            "stopping" => Ok(Self::Stopping),
            "completed" => Ok(Self::Completed),
            "interrupted" => Ok(Self::Interrupted),
            "unknown" => Ok(Self::Unknown),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }

    fn active(self) -> bool {
        matches!(
            self,
            Self::Admitted | Self::LaunchIntent | Self::Running | Self::Stopping
        )
    }

    fn permits(self, next: Self) -> bool {
        use ExternalRunState::*;
        matches!(
            (self, next),
            (Admitted, LaunchIntent) | (LaunchIntent, Running) | (Running, Stopping | Completed)
        ) || (self.active() && matches!(next, Interrupted | Unknown))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRun {
    pub key: ExternalRunKey,
    pub state: ExternalRunState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalActionStatus {
    Observed,
    Running,
    Completed,
    Failed,
    Unknown,
}

impl ExternalActionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "observed" => Ok(Self::Observed),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

/// Bounded metadata only. No arguments, results, approval payloads or executable blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAction {
    pub key: ExternalRunKey,
    pub call_id: String,
    pub tool_name: String,
    pub status: ExternalActionStatus,
    pub provenance: ExecutionBackend,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub(super) fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

fn canonical_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 4096
        && !s.chars().any(char::is_control)
        && Path::new(s).is_absolute()
        && (s == "/" || (!s.ends_with('/') && !s.contains("//")))
        && !s.split('/').any(|c| c == "." || c == "..")
        && !s.contains('\\')
}

pub(super) fn validate_spec(s: &CliBindingSpec) -> Result<()> {
    let uuid = s.cli_session_id.as_bytes();
    let canonical_uuid = uuid.len() == 36
        && uuid.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(b)
            }
        });
    if !canonical_uuid
        || !canonical_path(&s.project_path)
        || !canonical_path(&s.executable_path)
        || ![
            &s.cli_version,
            &s.config_ref,
            &s.account_scope,
            &s.launch_policy,
        ]
        .into_iter()
        .all(|s| token(s))
    {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}

fn validate_key(key: &ExternalRunKey) -> Result<()> {
    if !token(&key.session)
        || !token(&key.op)
        || key.generation == 0
        || key.generation >= i64::MAX as u64
    {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}

impl Store {
    /// Fail closed for CLI/unknown identities (including missing sessions), never fall back.
    pub fn assert_native_execution(c: &Connection, session: &str) -> Result<()> {
        Self::assert_session_live(c, session)?;
        let backend: Option<String> = c
            .query_row(
                "SELECT execution_backend FROM sessions WHERE id=?1",
                [session],
                |r| r.get(0),
            )
            .optional()?;
        if backend.as_deref() != Some("nativeAgent") {
            return Err(StoreError::UnsupportedExecution);
        }
        Ok(())
    }

    /// Legacy root-only CLI creation without a model. No rebinding or model upgrade.
    pub fn create_cli_session(
        &self,
        id: &str,
        request_key: &str,
        binding: &CliBindingSpec,
        now: i64,
    ) -> Result<(SessionRow, bool)> {
        self.create_cli_session_impl(id, request_key, binding, None, now)
    }

    /// Atomically freeze the approved binding and model alias. Exact request key, id,
    /// binding AND model repeats are idempotent; any changed payload conflicts.
    /// Creation permanently consumes the CLI UUID, even without launch or after removal.
    pub fn create_cli_session_with_model(
        &self,
        id: &str,
        request_key: &str,
        binding: &CliBindingSpec,
        model_alias: &str,
        now: i64,
    ) -> Result<(SessionRow, bool)> {
        if !matches!(model_alias, "haiku" | "sonnet" | "opus") {
            return Err(StoreError::ExecutionConflict);
        }
        self.create_cli_session_impl(id, request_key, binding, Some(model_alias), now)
    }

    /// Resolve a durable creation before checking an ephemeral approval's expiry.
    /// Only the exact live CLI id/key pair succeeds. This read never consumes an approval,
    /// changes a revision, or authorizes creation; None means neither id nor key exists.
    pub fn cli_creation(&self, id: &str, request_key: &str) -> Result<Option<SessionRow>> {
        if !token(id) || !token(request_key) {
            return Err(StoreError::ExecutionConflict);
        }
        self.read(|c| Self::cli_creation_in(c, id, request_key))
    }

    fn cli_creation_in(c: &Connection, id: &str, request_key: &str) -> Result<Option<SessionRow>> {
        Self::assert_session_live(c, id)?;
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_removals WHERE create_key=?1)",
            [request_key],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(StoreError::Removed);
        }
        let existing = c
            .query_row(
                "SELECT * FROM sessions WHERE create_key=?1",
                [request_key],
                Self::session_from_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.id != id
                || existing.execution_backend != ExecutionBackend::LocalClaudeCli
                || Self::cli_binding_in(c, id)?.is_none()
            {
                return Err(StoreError::ExecutionConflict);
            }
            return Ok(Some(existing));
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(None)
    }

    fn create_cli_session_impl(
        &self,
        id: &str,
        request_key: &str,
        binding: &CliBindingSpec,
        model_alias: Option<&str>,
        now: i64,
    ) -> Result<(SessionRow, bool)> {
        validate_spec(binding)?;
        if !token(id) || !token(request_key) {
            return Err(StoreError::ExecutionConflict);
        }
        self.tx(|tx| {
            if let Some(existing) = Self::cli_creation_in(tx, id, request_key)? {
                if existing.model.as_deref() != model_alias
                    || Self::cli_binding_in(tx, id)?.map(|b| b.identity).as_ref() != Some(binding) {
                    return Err(StoreError::ExecutionConflict);
                }
                return Ok((existing, false));
            }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1) OR EXISTS(SELECT 1 FROM cli_binding_receipts WHERE cli_session_id=?2 OR session=?1)", params![id,binding.cli_session_id], |r| r.get::<_, bool>(0))? {
                return Err(StoreError::ExecutionConflict);
            }
            tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,subagents_enabled,create_key,execution_backend,model)
                VALUES(?1,?2,?3,?3,?4,0,?5,'localClaudeCli',?6)", params![id,binding.project_path,now,json(&Origin::default()),request_key,model_alias])?;
            tx.execute("INSERT INTO cli_bindings(session,cli_session_id,project_path,executable_path,cli_version,config_ref,account_scope,launch_policy)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id,binding.cli_session_id,binding.project_path,binding.executable_path,binding.cli_version,binding.config_ref,binding.account_scope,binding.launch_policy])?;
            Ok((tx.query_row("SELECT * FROM sessions WHERE id=?1", [id], Self::session_from_row)?, true))
        })
    }

    fn cli_binding_in(c: &Connection, session: &str) -> Result<Option<CliBinding>> {
        Ok(c.query_row("SELECT b.* FROM cli_bindings b JOIN sessions s ON s.id=b.session WHERE b.session=?1 AND s.execution_backend='localClaudeCli'", [session], |r| {
            Ok(CliBinding {
                identity: CliBindingSpec {
                    cli_session_id: r.get("cli_session_id")?, project_path: r.get("project_path")?,
                    executable_path: r.get("executable_path")?, cli_version: r.get("cli_version")?,
                    config_ref: r.get("config_ref")?, account_scope: r.get("account_scope")?, launch_policy: r.get("launch_policy")?,
                },
                generation: r.get("generation")?, context_valid: r.get("context_valid")?,
            })
        }).optional()?)
    }

    /// Native (or missing) sessions have no CLI binding. Reads never probe credentials or disk.
    pub fn cli_binding(&self, session: &str) -> Result<Option<CliBinding>> {
        self.read(|c| Self::cli_binding_in(c, session))
    }

    fn assert_external_binding(c: &Connection, key: &ExternalRunKey) -> Result<()> {
        validate_key(key)?;
        Self::assert_session_live(c, &key.session)?;
        let binding =
            Self::cli_binding_in(c, &key.session)?.ok_or(StoreError::UnsupportedExecution)?;
        if !binding.context_valid || binding.generation != key.generation {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }

    fn external_run_in(c: &Connection, key: &ExternalRunKey) -> Result<Option<ExternalRun>> {
        Ok(c.query_row("SELECT state,created_at,updated_at FROM external_runs WHERE session=?1 AND generation=?2 AND op=?3",
            params![key.session,key.generation,key.op], |r| Ok(ExternalRun {
                key: key.clone(), state: ExternalRunState::parse(&r.get::<_, String>(0)?)?,
                created_at_ms: r.get(1)?, updated_at_ms: r.get(2)?,
            })).optional()?)
    }

    pub fn external_run(&self, key: &ExternalRunKey) -> Result<Option<ExternalRun>> {
        validate_key(key)?;
        self.read(|c| Self::external_run_in(c, key))
    }

    /// One active op per binding. Only a fresh binding or a completed previous run can admit
    /// new work. An existing key is returned with `false`, never a fresh launch authorization.
    pub fn admit_external_run(
        &self,
        key: &ExternalRunKey,
        now: i64,
    ) -> Result<(ExternalRun, bool)> {
        self.tx(|tx| {
            Self::assert_raw_external_run(tx, key)?;
            Self::assert_external_binding(tx, key)?;
            if let Some(run) = Self::external_run_in(tx, key)? { return Ok((run, false)); }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM external_runs WHERE session=?1 AND (state<>'completed' OR op=?2))", params![key.session,key.op], |r| r.get::<_, bool>(0))? {
                return Err(StoreError::ExecutionConflict);
            }
            tx.execute("INSERT INTO external_runs(session,generation,op,state,created_at,updated_at) VALUES(?1,?2,?3,'admitted',?4,?4)", params![key.session,key.generation,key.op,now])?;
            Self::bump(tx, &key.session, now)?;
            Ok((Self::external_run_in(tx, key)?.expect("inserted run"), true))
        })
    }

    /// Strict compare-and-transition; terminal states cannot transition or be replayed.
    /// Running -> Completed atomically requires all action metadata Completed/Failed, PLUS
    /// caller-verified final result AND root exit. Settled metadata is not proof of either.
    /// Stopping cannot complete or admit new work, even while context_valid remains true.
    /// Interrupted/Unknown permanently invalidate context and fence generation.
    pub fn transition_external_run(
        &self,
        key: &ExternalRunKey,
        expected: ExternalRunState,
        next: ExternalRunState,
        now: i64,
    ) -> Result<ExternalRun> {
        if !expected.permits(next) {
            return Err(StoreError::ExecutionConflict);
        }
        self.tx(|tx| {
            Self::assert_raw_external_run(tx, key)?;
            Self::assert_external_binding(tx, key)?;
            if next == ExternalRunState::Completed && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM external_actions WHERE session=?1 AND generation=?2 AND op=?3 AND status NOT IN ('completed','failed'))",
                params![key.session,key.generation,key.op], |r| r.get::<_, bool>(0))? {
                return Err(StoreError::ExecutionConflict);
            }
            let changed = tx.execute("UPDATE external_runs SET state=?4,updated_at=MAX(updated_at,?5) WHERE session=?1 AND generation=?2 AND op=?3 AND state=?6", params![key.session,key.generation,key.op,next.as_str(),now,expected.as_str()])?;
            if changed != 1 { return Err(StoreError::ExecutionConflict); }
            if matches!(next, ExternalRunState::Interrupted | ExternalRunState::Unknown) {
                Self::invalidate_external_binding(tx, key, now)?;
            }
            Self::bump(tx, &key.session, now)?;
            Ok(Self::external_run_in(tx, key)?.expect("transitioned run"))
        })
    }

    fn invalidate_external_binding(c: &Connection, key: &ExternalRunKey, now: i64) -> Result<()> {
        Self::revoke_external_permissions_for_run_in(c, key, now)?;
        c.execute("UPDATE cli_bindings SET context_valid=0,generation=generation+1 WHERE session=?1 AND generation=?2 AND context_valid=1", params![key.session,key.generation])?;
        Self::mark_external_actions_unknown(c, key, now)
    }

    fn mark_external_actions_unknown(c: &Connection, key: &ExternalRunKey, now: i64) -> Result<()> {
        c.execute("UPDATE external_actions SET status='unknown',updated_at=MAX(updated_at,?4) WHERE session=?1 AND generation=?2 AND op=?3 AND status IN ('observed','running')", params![key.session,key.generation,key.op,now])?;
        Ok(())
    }

    /// Startup fence, in ONE transaction, before anything executes. All nonterminal runs
    /// become Unknown, including admitted-but-not-launched runs. Completed history is preserved.
    pub fn recover_external_runs(&self, now: i64) -> Result<usize> {
        self.tx(|tx| {
            // Revoke even anomalous/orphaned authority; restart never restores an allow.
            tx.execute("UPDATE external_permissions SET state='revoked',updated_at=MAX(updated_at,?1) WHERE state IN ('pending','granted')", [now])?;
            let keys = tx.prepare("SELECT session,generation,op FROM external_runs WHERE state IN ('admitted','launchIntent','running','stopping')")?
                .query_map([], |r| Ok(ExternalRunKey { session: r.get(0)?, generation: r.get(1)?, op: r.get(2)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for key in &keys {
                if Self::external_prompt_for_run_in(tx, key)?.is_some() {
                    Self::recover_external_prompt_in(tx, key, now)?;
                    Self::bump(tx, &key.session, now)?;
                    continue;
                }
                tx.execute("UPDATE external_runs SET state='unknown',updated_at=MAX(updated_at,?4) WHERE session=?1 AND generation=?2 AND op=?3", params![key.session,key.generation,key.op,now])?;
                Self::invalidate_external_binding(tx, key, now)?;
                Self::bump(tx, &key.session, now)?;
            }
            Ok(keys.len())
        })
    }

    /// Metadata only, at most 4096 distinct calls per run. Writes require the exact live
    /// (session,generation,op), Running/Stopping state, and immutable call-id/tool identity.
    pub fn record_external_action(
        &self,
        key: &ExternalRunKey,
        call_id: &str,
        tool_name: &str,
        status: ExternalActionStatus,
        now: i64,
    ) -> Result<()> {
        if !token(call_id) || !token(tool_name) {
            return Err(StoreError::ExecutionConflict);
        }
        self.tx(|tx| {
            Self::assert_raw_external_run(tx, key)?;
            Self::assert_external_binding(tx, key)?;
            let run = Self::external_run_in(tx, key)?.ok_or(StoreError::ExecutionConflict)?;
            if !matches!(run.state, ExternalRunState::Running | ExternalRunState::Stopping) { return Err(StoreError::ExecutionConflict); }
            let existing: Option<(String,String)> = tx.query_row("SELECT tool_name,status FROM external_actions WHERE session=?1 AND generation=?2 AND op=?3 AND call_id=?4", params![key.session,key.generation,key.op,call_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((name, old)) = existing {
                if name != tool_name { return Err(StoreError::ExecutionConflict); }
                if old == status.as_str() { return Ok(()); }
                if !matches!(old.as_str(), "observed" | "running") || status == ExternalActionStatus::Observed {
                    return Err(StoreError::ExecutionConflict);
                }
            } else {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM external_actions WHERE session=?1 AND generation=?2 AND op=?3", params![key.session,key.generation,key.op], |r| r.get(0))?;
                if count >= 4096 { return Err(StoreError::ExecutionConflict); }
            }
            tx.execute("INSERT INTO external_actions(session,generation,op,call_id,tool_name,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)
                ON CONFLICT(session,generation,op,call_id) DO UPDATE SET status=excluded.status,updated_at=MAX(external_actions.updated_at,excluded.updated_at)", params![key.session,key.generation,key.op,call_id,tool_name,status.as_str(),now])?;
            Self::bump(tx, &key.session, now)?;
            Ok(())
        })
    }

    pub fn external_actions(&self, key: &ExternalRunKey) -> Result<Vec<ExternalAction>> {
        validate_key(key)?;
        self.read(|c| {
            Ok(c.prepare("SELECT call_id,tool_name,status,created_at,updated_at FROM external_actions WHERE session=?1 AND generation=?2 AND op=?3 ORDER BY created_at,call_id")?
                .query_map(params![key.session,key.generation,key.op], |r| Ok(ExternalAction {
                    key: key.clone(), call_id: r.get(0)?, tool_name: r.get(1)?,
                    status: ExternalActionStatus::parse(&r.get::<_, String>(2)?)?, provenance: ExecutionBackend::LocalClaudeCli,
                    created_at_ms: r.get(3)?, updated_at_ms: r.get(4)?,
                }))?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests;

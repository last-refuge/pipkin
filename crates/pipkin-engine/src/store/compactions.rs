//! Durable compaction operations. The journal stores model input and non-secret stream
//! options; credentials, headers, environment and resolved endpoints are never checkpointed.
//! Callers update progress, usage and summary entries together in a short transaction.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompactionScope {
    Manual,
    Background,
    Blocking,
}
impl CompactionScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Background => "background",
            Self::Blocking => "blocking",
        }
    }
}

/// The exact non-secret request chosen by selection. The model key is resolved against
/// the catalog again on recovery; credentials and network configuration remain current.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryRequest {
    pub model: String,
    pub thinking: String,
    pub provider_session: String,
    pub messages: Vec<Message>,
    pub max_tokens: u64,
    pub timeout_ms: u64,
    pub stream_options: serde_json::Map<String, Value>,
    pub tail: u64,
    pub first_kept: u64,
    pub tokens_before: u64,
}

impl SummaryRequest {
    pub fn capture(
        req: &crate::provider::ProviderRequest,
        tail: u64,
        first_kept: u64,
        tokens_before: u64,
    ) -> Self {
        let mut stream_options = serde_json::Map::new();
        // Explicit allowlist prevents future provider/auth options accidentally becoming
        // credential persistence. Request construction re-applies the current environment.
        for key in [
            "maxRetries",
            "maxRetryDelayMs",
            "transport",
            "thinkingBudgets",
            "websocketConnectTimeoutMs",
        ] {
            if let Some(value) = req.options.get(key) {
                stream_options.insert(key.into(), value.clone());
            }
        }
        Self {
            model: req.model.key(),
            thinking: req.thinking.as_str().into(),
            provider_session: req.session_id.clone(),
            messages: req.messages.clone(),
            max_tokens: req.max_tokens.unwrap_or(0),
            timeout_ms: req.timeout.as_millis().min(u64::MAX as u128) as u64,
            stream_options,
            tail,
            first_kept,
            tokens_before,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryResponse {
    pub message: Message,
    pub received_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase", deny_unknown_fields)]
pub enum CompactionOutcome {
    Done { entry: u64 },
    Nothing { reason: String },
    Failed { error: String },
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase", deny_unknown_fields)]
pub enum CompactionProgress {
    Select,
    Summarize {
        request: Box<SummaryRequest>,
        attempt: u32,
        response: Option<Box<SummaryResponse>>,
    },
    Retry {
        request: Box<SummaryRequest>,
        attempt: u32,
        until: i64,
    },
    Terminal {
        result: CompactionOutcome,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompactionJob {
    pub id: u64,
    pub session: String,
    pub key: String,
    pub run: Option<u64>,
    pub scope: CompactionScope,
    pub instructions: Option<String>,
    pub stopping: bool,
    pub revision: u64,
    pub progress: CompactionProgress,
}

impl Store {
    /// Bounded startup enumeration: saved prompts and responses are loaded only by drivers.
    pub fn pending_compactions_page(&self, after: u64, limit: u32) -> Result<Vec<(u64, String)>> {
        self.read(|c| {
            let mut q = c.prepare("SELECT id, session FROM compactions WHERE state != 'settled' AND id > ?1 ORDER BY id LIMIT ?2")?;
            Ok(q.query_map(params![after, limit.min(64)], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn pending_run_compaction(c: &Connection, run: u64) -> Result<Option<u64>> {
        Ok(c.query_row(
            "SELECT id FROM compactions WHERE run = ?1 AND state != 'settled' ORDER BY id LIMIT 1",
            [run],
            |r| r.get(0),
        )
        .optional()?)
    }

    /// Admit once per stable operation key. A repeat must describe the identical operation.
    pub fn admit_compaction(
        c: &Connection,
        session: &str,
        key: &str,
        run: Option<u64>,
        scope: CompactionScope,
        instructions: Option<&str>,
    ) -> Result<CompactionJob> {
        if let Some(job) = Self::compaction_by_key(c, session, key)? {
            if job.run != run || job.scope != scope || job.instructions.as_deref() != instructions {
                return Err(StoreError::Corrupt(
                    "compaction key was reused for a different operation".into(),
                ));
            }
            return Ok(job);
        }
        if scope == CompactionScope::Blocking && run.is_none() {
            return Err(StoreError::Corrupt(
                "blocking compaction requires a run".into(),
            ));
        }
        if let Some(run) = run {
            let owner: Option<String> = c
                .query_row("SELECT session FROM runs WHERE id = ?1", [run], |r| {
                    r.get(0)
                })
                .optional()?;
            if owner.as_deref() != Some(session) {
                return Err(StoreError::Corrupt(
                    "compaction run belongs to another session".into(),
                ));
            }
        }
        c.execute("INSERT INTO compactions(session, operation_key, run, scope, instructions, state, revision, progress)
            VALUES (?1, ?2, ?3, ?4, ?5, 'running', 0, ?6)",
            params![session, key, run, scope.as_str(), instructions, json(&CompactionProgress::Select)])?;
        Self::compaction_in(c, c.last_insert_rowid() as u64)?
            .ok_or_else(|| StoreError::Corrupt("missing admitted compaction".into()))
    }

    pub fn compaction_by_key(
        c: &Connection,
        session: &str,
        key: &str,
    ) -> Result<Option<CompactionJob>> {
        let id = c
            .query_row(
                "SELECT id FROM compactions WHERE session = ?1 AND operation_key = ?2",
                params![session, key],
                |r| r.get::<_, u64>(0),
            )
            .optional()?;
        id.map(|id| Self::compaction_in(c, id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn compaction_in(c: &Connection, id: u64) -> Result<Option<CompactionJob>> {
        let row = c.query_row("SELECT session, operation_key, run, scope, instructions, state, revision, progress FROM compactions WHERE id = ?1", [id], |r| {
            Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<u64>>(2)?, r.get::<_,String>(3)?, r.get::<_,Option<String>>(4)?, r.get::<_,String>(5)?, r.get::<_,u64>(6)?, r.get::<_,String>(7)?))
        }).optional()?;
        row.map(
            |(session, key, run, scope, instructions, state, revision, raw)| {
                let scope = match scope.as_str() {
                    "manual" => CompactionScope::Manual,
                    "background" => CompactionScope::Background,
                    "blocking" => CompactionScope::Blocking,
                    _ => return Err(StoreError::Corrupt("unknown compaction scope".into())),
                };
                let progress: CompactionProgress = parse_json(&raw, "compaction checkpoint")?;
                if (state == "settled") != matches!(progress, CompactionProgress::Terminal { .. }) {
                    return Err(StoreError::Corrupt(
                        "compaction checkpoint disagrees with terminal state".into(),
                    ));
                }
                Ok(CompactionJob {
                    id,
                    session,
                    key,
                    run,
                    scope,
                    instructions,
                    stopping: state == "stopping",
                    revision,
                    progress,
                })
            },
        )
        .transpose()
    }

    /// Compare-and-swap prevents a stale response/retry from overwriting durable Stop or
    /// settlement. Perform usage accounting/entry placement in the SAME caller transaction.
    pub fn save_compaction(
        c: &Connection,
        job: &CompactionJob,
        progress: &CompactionProgress,
    ) -> Result<bool> {
        if matches!(job.progress, CompactionProgress::Terminal { .. }) {
            return Ok(false);
        }
        if job.stopping
            && !matches!(
                progress,
                CompactionProgress::Terminal {
                    result: CompactionOutcome::Aborted
                }
            )
        {
            return Ok(false);
        }
        let state = if matches!(progress, CompactionProgress::Terminal { .. }) {
            "settled"
        } else {
            "running"
        };
        Ok(c.execute(
            "UPDATE compactions SET progress = ?3, state = ?4, revision = revision + 1
            WHERE id = ?1 AND revision = ?2 AND state != 'settled'",
            params![job.id, job.revision, json(progress), state],
        )? == 1)
    }

    /// Called in the same transaction as a conversation's Stop intent. Background scope is
    /// deliberately excluded; a stopping row remains discoverable until it settles Aborted.
    pub fn stop_compactions(c: &Connection, session: &str) -> Result<()> {
        c.execute(
            "UPDATE compactions SET state = 'stopping', revision = revision + 1
            WHERE session = ?1 AND scope != 'background' AND state = 'running'",
            [session],
        )?;
        Ok(())
    }

    /// Enumerate only identities on startup; do not materialize every saved prompt/response.
    pub fn pending_compaction_ids(&self) -> Result<Vec<u64>> {
        self.read(|c| {
            let mut query =
                c.prepare("SELECT id FROM compactions WHERE state != 'settled' ORDER BY id")?;
            Ok(query
                .query_map([], |r| r.get::<_, u64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests;

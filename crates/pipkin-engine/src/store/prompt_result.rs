//! Read a settled input's answer from durable state, including preserved Pi submissions.
use super::*;

pub(crate) enum PromptState {
    Unknown,
    Pending,
    Settled(PromptResult),
}

impl Store {
    pub(crate) fn pending_sessions(&self) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT session FROM runs WHERE state != 'settled'
                UNION SELECT session FROM requests WHERE state = 'queued'",
            )?;
            Ok(stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub(crate) fn prompt_result(&self, session: &str, request: &str) -> Result<PromptState> {
        self.read(|c| {
            // Pending waits wake on streaming commits too. Read only status columns, not
            // the request's potentially large input text/images, on each notification.
            let row = c
                .query_row(
                    "SELECT state, run, outcome FROM requests WHERE session = ?1 AND key = ?2",
                    params![session, request],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<i64>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((state, run, outcome)) = row else {
                return Ok(PromptState::Unknown);
            };
            if matches!(state.as_str(), "queued" | "running") {
                return Ok(PromptState::Pending);
            }
            // Imported inputs have no native run. Use their preserved submission record so
            // original answer identities and reasons (including reset/stale) are not guessed
            // from nearby messages or the display-oriented RunOutcome mapping.
            if run.is_none() {
                let raw: Option<String> = c
                    .query_row(
                        "SELECT value FROM kv WHERE session = ?1 AND key = 'pi.submissions'",
                        [session],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(raw) = raw {
                    let submissions: Vec<Value> = serde_json::from_str(&raw)
                        .map_err(|e| StoreError::Corrupt(format!("Imported submissions: {e}")))?;
                    if let Some(submission) = submissions
                        .iter()
                        .find(|s| s["requestId"].as_str() == Some(request) && s["type"] == "input")
                    {
                        if submission["status"] == "unanswered" {
                            let reason = submission["reason"].as_str().ok_or_else(|| {
                                StoreError::Corrupt(
                                    "Unanswered imported input has no reason".into(),
                                )
                            })?;
                            return Ok(PromptState::Settled(unanswered(reason)));
                        }
                        if submission["status"] == "done" {
                            let entry = submission["answer"]
                                .as_u64()
                                .map(|id| Self::entry_in(c, session, id))
                                .transpose()?
                                .flatten();
                            return Ok(PromptState::Settled(done(entry)));
                        }
                    }
                }
            }
            let Some(outcome) = outcome else {
                return Err(StoreError::Corrupt("Settled input has no outcome".into()));
            };
            let outcome: RunOutcome = serde_json::from_str(&outcome)
                .map_err(|e| StoreError::Corrupt(format!("Input outcome: {e}")))?;
            let result = match outcome {
                RunOutcome::Completed => {
                    let entry = if let Some(run) = run {
                        // Use the indexed run range; never materialize the conversation or
                        // accidentally return a later prompt's answer. Intermediate thinking
                        // and tool-round text are not the final answer.
                        let id: Option<i64> = c
                            .query_row(
                                "SELECT id FROM entries WHERE session = ?1 AND run = ?2
                             AND partial = 0 AND kind = 'message'
                             AND json_extract(body, '$.message.role') = 'assistant'
                             ORDER BY id DESC LIMIT 1",
                                params![session, run],
                                |r| r.get(0),
                            )
                            .optional()?;
                        id.map(|id| Self::entry_in(c, session, id as u64))
                            .transpose()?
                            .flatten()
                    } else {
                        None
                    };
                    done(entry)
                }
                RunOutcome::Aborted => unanswered("aborted"),
                RunOutcome::ModelError { .. } => unanswered("model_error"),
                RunOutcome::NoModel { .. } => unanswered("no_model"),
                RunOutcome::Interrupted { .. } => unanswered("interrupted"),
                RunOutcome::Failed { .. } => unanswered("failed"),
            };
            Ok(PromptState::Settled(result))
        })
    }
}

fn unanswered(reason: &str) -> PromptResult {
    PromptResult::Unanswered {
        text: None,
        reason: reason.into(),
    }
}

fn done(entry: Option<Entry>) -> PromptResult {
    let text = match entry.map(|e| e.kind) {
        Some(EntryKind::Message {
            message: Message::Assistant { content, .. },
            ..
        }) => content.iter().filter_map(Block::as_text).collect(),
        // Pi reads only model[0], including a custom entry that preserved its model envelope.
        Some(EntryKind::Custom { data, .. }) => data["model"]
            .as_array()
            .and_then(|m| m.first())
            .filter(|m| m["role"] == "assistant")
            .and_then(|m| m["content"].as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect()
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    PromptResult::Done { text, reason: None }
}

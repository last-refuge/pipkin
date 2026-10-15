//! Context occupancy (plan §11, STATUS-2): what the next request would carry, from the same
//! estimation the compaction path uses. The newest assistant usage in the live context is
//! the reported base (its request included everything before it); entries after it and the
//! cached system prompt are estimated. Published after a run settles, after a compaction and
//! after a model switch; the snapshot carries it too. Nothing here reads cumulative usage.

use pipkin_engine_protocol::*;

use super::Shared;
use crate::context::compaction::{
    calculate_context_tokens, estimate_context, estimate_text_tokens,
};

/// The pure computation, for tests.
pub struct ContextInputs<'a> {
    pub entries: &'a [Entry],
    pub system_tokens: u64,
    pub model: Option<String>,
    pub capacity: Option<u64>,
    pub reserve: Option<u64>,
    pub compaction: String,
    pub revision: u64,
}

pub fn compute(inputs: ContextInputs<'_>) -> ContextUsageView {
    // The newest reported assistant after the head marker (ids above it), as the estimator
    // sees it; and whether anything with context weight was appended after it.
    let head = inputs
        .entries
        .first()
        .filter(|e| crate::context::derive::is_head_marker(e))
        .map(|e| e.id);
    let mut measured: Option<(u64, String)> = None;
    let mut after_measured = false;
    for entry in inputs.entries.iter().rev() {
        if head.is_some_and(|h| entry.id <= h) {
            break;
        }
        if entry.retired {
            continue;
        }
        if let EntryKind::Message {
            message:
                Message::Assistant {
                    usage,
                    model,
                    provider,
                    ..
                },
            partial: false,
            ..
        } = &entry.kind
            && calculate_context_tokens(usage) > 0
        {
            // The message carries provider and model id apart; the selection is `provider/id`.
            let key = if provider.is_empty() {
                model.clone()
            } else {
                format!("{provider}/{model}")
            };
            measured = Some((entry.id, key));
            break;
        }
        if matches!(entry.kind, EntryKind::Message { .. }) {
            after_measured = true;
        }
    }
    let has_content = inputs
        .entries
        .iter()
        .any(|e| matches!(e.kind, EntryKind::Message { .. }));
    // A reported usage already counted the system prompt; the estimate adds it only when
    // nothing was measured (as the compaction estimator's `extra`).
    let (tokens, source) = if !has_content && inputs.system_tokens == 0 {
        (None, "unavailable")
    } else {
        let mut tokens = estimate_context(inputs.entries, &[]);
        if measured.is_none() {
            tokens += inputs.system_tokens;
        }
        let source = match (&measured, after_measured) {
            (Some(_), false) => "reported",
            _ => "estimated",
        };
        (Some(tokens), source)
    };
    let measured_model = measured.as_ref().map(|(_, m)| m.clone());
    let stale = matches!((&measured_model, &inputs.model), (Some(a), Some(b)) if a != b);
    let remaining = match (inputs.capacity, tokens) {
        (Some(capacity), Some(tokens)) => Some(
            capacity
                .saturating_sub(inputs.reserve.unwrap_or(0))
                .saturating_sub(tokens),
        ),
        _ => None,
    };
    ContextUsageView {
        tokens,
        source: source.into(),
        capacity: inputs.capacity,
        reserve: inputs.reserve,
        remaining,
        model: inputs.model,
        measured_entry: measured.as_ref().map(|(id, _)| *id),
        measured_model,
        stale,
        compaction: inputs.compaction,
        revision: inputs.revision,
    }
}

impl Shared {
    pub fn context_usage(&self, session: &str) -> Result<ContextUsageView, ProtoError> {
        let system_tokens = self
            .session(session)
            .ok()
            .map(|s| Self::cached_system_tokens(&s))
            .unwrap_or(0);
        self.context_usage_with_system_tokens(session, system_tokens)
    }

    pub(super) fn context_usage_with_system_tokens(
        &self,
        session: &str,
        system_tokens: u64,
    ) -> Result<ContextUsageView, ProtoError> {
        let row = self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        let entries = self.store.context_entries(session)?;
        let settings = self.settings_for(session);
        let model = row.model.clone();
        let capacity = model
            .as_deref()
            .and_then(|key| self.auth.catalog().find(key).map(|m| m.context_window))
            .filter(|w| *w > 0);
        let reserve = settings
            .compaction
            .enabled
            .then_some(settings.compaction.reserve_tokens);
        let compaction = if !settings.compaction.enabled {
            "off".to_owned()
        } else {
            self.store
                .read(|c| {
                    use rusqlite::OptionalExtension;
                    Ok(c.query_row(
                        "SELECT scope FROM compactions WHERE session = ?1 AND state != 'settled' ORDER BY id DESC LIMIT 1",
                        [session],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?)
                })?
                .unwrap_or_else(|| "idle".to_owned())
        };
        Ok(compute(ContextInputs {
            entries: &entries,
            system_tokens,
            model,
            capacity,
            reserve,
            compaction,
            revision: row.revision,
        }))
    }

    /// Capture volatile cache state before taking publication; session() takes the registry
    /// lock, which removal takes BEFORE publication. Never invert that ordering in snapshots.
    pub(super) fn cached_system_tokens(sess: &super::SessionRt) -> u64 {
        sess.prompt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(_, sections)| {
                sections
                    .values()
                    .map(|v| estimate_text_tokens(v.as_str().unwrap_or("")))
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Volatile publication (like UI state): the next snapshot recomputes it anyway.
    pub(super) fn publish_context(&self, session: &str) {
        if let Ok(context) = self.context_usage(session) {
            self.publish(Event::Session {
                session: session.to_owned(),
                revision: 0,
                change: SessionChange::ContextChanged { context },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, message: Message) -> Entry {
        Entry {
            id,
            at_ms: 0,
            run: None,
            retired: false,
            kind: EntryKind::Message {
                message,
                mode: InputMode::Prompt,
                request: None,
                partial: false,
            },
        }
    }

    fn assistant(model: &str, total: u64) -> Message {
        Message::Assistant {
            content: vec![Block::text("ok")],
            api: "scripted".into(),
            provider: "p".into(),
            model: model.into(),
            usage: Usage {
                input: total - 1,
                output: 1,
                ..Default::default()
            },
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            extra: Default::default(),
        }
    }

    fn user(text: &str) -> Message {
        Message::User {
            content: vec![Block::text(text)],
            timestamp: 0,
            extra: Default::default(),
        }
    }

    #[test]
    fn reported_estimated_unavailable_stale_and_remaining() {
        let empty = compute(ContextInputs {
            entries: &[],
            system_tokens: 0,
            model: Some("p/m".into()),
            capacity: Some(1000),
            reserve: Some(100),
            compaction: "idle".into(),
            revision: 3,
        });
        assert_eq!(empty.tokens, None);
        assert_eq!(empty.source, "unavailable");
        assert_eq!(empty.remaining, None);
        assert_eq!(empty.revision, 3);
        // The last response's usage, nothing after it: reported.
        let entries = vec![entry(1, user("hi")), entry(2, assistant("m", 40))];
        let reported = compute(ContextInputs {
            entries: &entries,
            system_tokens: 0,
            model: Some("p/m".into()),
            capacity: Some(1000),
            reserve: Some(100),
            compaction: "idle".into(),
            revision: 4,
        });
        assert_eq!(reported.tokens, Some(40));
        assert_eq!(reported.source, "reported");
        assert_eq!(reported.measured_entry, Some(2));
        assert_eq!(reported.remaining, Some(860));
        assert!(!reported.stale);
        // A user message after it, or a system prompt, makes it an estimate on that base.
        let mut more = entries.clone();
        more.push(entry(3, user("a longer follow-up question here")));
        let estimated = compute(ContextInputs {
            entries: &more,
            system_tokens: 25,
            model: Some("p/m".into()),
            capacity: Some(1000),
            reserve: None,
            compaction: "background".into(),
            revision: 5,
        });
        assert_eq!(estimated.source, "estimated");
        assert!(
            estimated.tokens.unwrap() > 40 && estimated.tokens.unwrap() < 65,
            "the system prompt was already in the reported figure: {:?}",
            estimated.tokens
        );
        assert_eq!(estimated.remaining, Some(1000 - estimated.tokens.unwrap()));
        assert_eq!(estimated.compaction, "background");
        // A different selected model makes the reported figure stale; no capacity, no remaining.
        let stale = compute(ContextInputs {
            entries: &entries,
            system_tokens: 0,
            model: Some("p/other".into()),
            capacity: None,
            reserve: None,
            compaction: "off".into(),
            revision: 6,
        });
        assert!(stale.stale);
        assert_eq!(stale.measured_model.as_deref(), Some("p/m"));
        assert_eq!(stale.remaining, None);
        // Tokens over the window saturate to zero remaining; retired entries are skipped.
        let mut retired = entries.clone();
        retired[1].retired = true;
        let est = compute(ContextInputs {
            entries: &retired,
            system_tokens: 30,
            model: None,
            capacity: Some(10),
            reserve: Some(5),
            compaction: "idle".into(),
            revision: 7,
        });
        assert_eq!(est.measured_entry, None);
        assert!(
            est.tokens.unwrap() >= 30,
            "unmeasured: the system prompt counts"
        );
        assert_eq!(est.remaining, Some(0));
    }
}

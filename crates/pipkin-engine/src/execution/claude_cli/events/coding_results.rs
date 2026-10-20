//! Coding owns a process with multiple internal turns. The CLI may flush their held
//! results together. Retain one bounded aggregate until EOF; never settle per turn.
use super::*;

#[derive(Default)]
pub(super) struct CodingResults {
    seen: HashMap<String, [u8; 32]>,
    anonymous: bool,
    pub outcome: Option<Outcome>,
    pub usage: Option<Usage>,
    pub last_result: Option<serde_json::Value>,
}

impl CodingResults {
    /// False means an identical delivery of an already counted result. It cannot
    /// close a later unfinished turn or add usage a second time.
    pub fn observe(&mut self, record: TerminalRecord, outcome: Outcome) -> Result<bool, Error> {
        if let Some(id) = &record.uuid {
            if !valid_id(id) || self.anonymous {
                return Err(Error::Identity);
            }
            let semantic = serde_json::to_vec(&(
                &record.subtype,
                record.is_error,
                record.usage,
                &record.result,
            ))
            .map_err(|_| Error::MalformedRecord)?;
            let digest: [u8; 32] = Sha256::digest(semantic).into();
            if let Some(prior) = self.seen.get(id) {
                return if prior == &digest {
                    Ok(false)
                } else {
                    Err(Error::Sequence)
                };
            }
            if self.seen.len() >= MAX_ENTRIES {
                return Err(Error::Limit);
            }
            self.seen.insert(id.clone(), digest);
        } else {
            // Older fixtures/producers may omit identity on a single result. Never
            // guess whether a second unidentified record is a retry or another turn.
            if self.outcome.is_some() {
                return Err(Error::Identity);
            }
            self.anonymous = true;
        }
        self.usage = if self.outcome.is_none() {
            record.usage
        } else {
            match (self.usage, record.usage) {
                (Some(a), Some(b)) => Some(sum_usage(a, b)?),
                _ => None,
            }
        };
        self.outcome = Some(match (self.outcome, outcome) {
            (Some(Outcome::Error), _) | (_, Outcome::Error) => Outcome::Error,
            (Some(Outcome::Unknown), _) | (_, Outcome::Unknown) => Outcome::Unknown,
            _ => Outcome::Success,
        });
        self.last_result = record.result;
        Ok(true)
    }
}

fn sum_usage(a: Usage, b: Usage) -> Result<Usage, Error> {
    fn sum(a: Option<u64>, b: Option<u64>) -> Result<Option<u64>, Error> {
        match (a, b) {
            (Some(a), Some(b)) => a
                .checked_add(b)
                .filter(|n| *n <= i64::MAX as u64)
                .map(Some)
                .ok_or(Error::Limit),
            _ => Ok(None),
        }
    }
    Ok(Usage {
        input_tokens: sum(a.input_tokens, b.input_tokens)?,
        output_tokens: sum(a.output_tokens, b.output_tokens)?,
        cache_read_input_tokens: sum(a.cache_read_input_tokens, b.cache_read_input_tokens)?,
        cache_creation_input_tokens: sum(
            a.cache_creation_input_tokens,
            b.cache_creation_input_tokens,
        )?,
        reasoning_tokens: sum(a.reasoning_tokens, b.reasoning_tokens)?,
        cache_write_1h: sum(a.cache_write_1h, b.cache_write_1h)?,
    })
}

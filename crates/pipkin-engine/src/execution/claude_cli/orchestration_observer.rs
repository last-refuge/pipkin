//! Inactive native-root stream contract. No effects, store writes or success authority.
//! Exactly the bound Pipkin MCP catalog is allowed, not Coding's CLI-owned tool inventory.
//! SDK tool IDs/results are correlation observations only; native cards/receipts come from
//! the authenticated bridge, never this stdout stream. No installed-CLI qualification.
use super::{
    events,
    orchestration_protocol::{Scope, Session},
};
use crate::{provider::ToolSpec, store::cli_agent_turns::LaunchContext};
pub use events::{Error, Outcome, TextKind, Usage};

/// Model display observations only. There is intentionally no ToolCall/ToolResult variant,
/// native protocol conversion or serialization. Debug never exposes model text.
#[derive(PartialEq, Eq)]
pub enum Observation {
    Context {
        tokens: Option<u64>,
        compact_pre_tokens: Option<u64>,
    },
    Initialized,
    Delta {
        message_id: String,
        block_index: usize,
        kind: TextKind,
        offset: usize,
        text: String,
    },
    Snapshot {
        message_id: String,
        block_index: usize,
        kind: TextKind,
        text: String,
    },
    Terminal {
        outcome: Outcome,
        usage: Option<Usage>,
        metadata: Option<super::usage_metadata::Metadata>,
    },
}
impl std::fmt::Debug for Observation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliAgentObservation([redacted])")
    }
}
fn display(event: events::Event) -> Option<Observation> {
    Some(match event {
        events::Event::Initialized => Observation::Initialized,
        events::Event::Context {
            tokens,
            compact_pre_tokens,
        } => Observation::Context {
            tokens,
            compact_pre_tokens,
        },
        events::Event::BlockDelta {
            message_id,
            block_index,
            kind,
            offset,
            text,
        } => Observation::Delta {
            message_id,
            block_index,
            kind,
            offset,
            text,
        },
        events::Event::BlockSnapshot {
            message_id,
            block_index,
            kind,
            text,
        } => Observation::Snapshot {
            message_id,
            block_index,
            kind,
            text,
        },
        events::Event::Terminal {
            outcome,
            usage,
            metadata,
        } => Observation::Terminal {
            outcome,
            usage,
            metadata,
        },
        events::Event::ExternalToolObserved { .. } | events::Event::ExternalToolResult { .. } => {
            return None;
        }
    })
}
/// Owns framing, UUID checking and SDK correlation. Bounds and display-loss diagnostics
/// are the shared decoder's; a terminal or successful finish is NOT root completion.
/// No API can turn this instance into a permissive CLI-owned-tools decoder.
pub struct Observer {
    scope: Scope,
    decoder: events::Decoder,
    catalog_hash: [u8; 32],
}
impl Observer {
    pub fn new(
        context: &LaunchContext,
        tools: &[ToolSpec],
        catalog_hash: [u8; 32],
    ) -> Result<Self, Error> {
        let catalog = Session::new(context.scope().clone(), tools)
            .map_err(|_| Error::InvalidConfiguration)?;
        if catalog.catalog_hash() != catalog_hash {
            return Err(Error::InvalidConfiguration);
        }
        let names = tools
            .iter()
            .map(|t| format!("mcp__pipkin__{}", t.name))
            .collect::<Vec<_>>();
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(Self {
            scope: context.scope().clone(),
            catalog_hash,
            decoder: events::Decoder::new(
                &context.binding().create.identity.cli_session_id,
                &names,
            )?
            .with_exact_inventory(),
        })
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn catalog_hash(&self) -> [u8; 32] {
        self.catalog_hash
    }
    pub fn summary(&self) -> events::Summary {
        self.decoder.summary()
    }
    pub fn diagnostic(&self) -> Option<events::StreamDiagnostic> {
        self.decoder.diagnostic()
    }
    pub fn parse_failure(&self) -> Option<events::ParseFailure> {
        self.decoder.parse_failure()
    }
    pub fn failed_record(&self) -> Option<events::RecordClass> {
        self.decoder.failed_record()
    }
    /// A mismatched callback cannot consume bytes or alter this owner's decoder.
    pub fn feed<E>(
        &mut self,
        scope: &Scope,
        bytes: &[u8],
        mut sink: impl FnMut(&Scope, Observation) -> Result<(), E>,
    ) -> Result<(), Error> {
        if scope != &self.scope {
            return Err(Error::Identity);
        }
        self.decoder.feed(bytes, |event| match display(event) {
            Some(event) => sink(&self.scope, event),
            None => Ok(()),
        })
    }
    pub fn finish<E>(
        self,
        mut sink: impl FnMut(&Scope, Observation) -> Result<(), E>,
    ) -> Result<events::Summary, Error> {
        self.decoder.finish(|event| match display(event) {
            Some(event) => sink(&self.scope, event),
            None => Ok(()),
        })
    }
}
#[cfg(test)]
mod tests;

//! Bounded CLI provider framing, lifecycle observations and inert display normalization.
//!
//! CLI assistant envelopes contain completed *blocks*, not whole-message finals. Match
//! stream blocks by API message/index, deduplicate SDK envelope IDs, and reconcile complete
//! blocks with previews. Without previews, distinct envelopes append distinct blocks.
//! Unknown telemetry is discarded. A malformed/unsupported presentation record marks display
//! incomplete, not execution failed; complete blocks/final result text can repair the display.
//!
//! Identity, init/inventory, tool correlation, resource bounds, terminal contradictions and
//! sink errors remain hard failures. Only an explicit result plus independent supervised
//! exit/drain/permission evidence can establish execution completion. EOF never does.
//! Trailing system telemetry is drained after a result. No prompt/delta is replayed.
//!
//! The runtime is the sole production framing consumer and supplies bounded asynchronous
//! backpressure; the phase-0 smoke decoder is not a second production validator. This module
//! does no I/O, permission decisions, native tool execution or credential access. Tool input,
//! signatures and account metadata are discarded; Debug never exposes display content.
//! Offline fixtures follow the official SDK message-flow contract; they are not live
//! qualification. See docs/claude-cli-provider-review.md and docs/scorecard.md.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de::IgnoredAny};
use sha2::{Digest, Sha256};

pub const MAX_LINE_BYTES: usize = 256 * 1024;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ENTRIES: usize = 4096;
/// Cumulative emitted display bytes (including replacements and tool results).
pub const MAX_PROJECTED_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    InvalidConfiguration,
    MalformedRecord,
    Identity,
    MissingInit,
    Sequence,
    UnsupportedContent,
    UnsupportedTool,
    OrphanToolResult,
    Limit,
    SinkRejected,
    Incomplete,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "invalid projection configuration",
            Self::MalformedRecord => "malformed CLI record",
            Self::Identity => "CLI identity mismatch",
            Self::MissingInit => "CLI initialization required",
            Self::Sequence => "invalid CLI event sequence",
            Self::UnsupportedContent => "unsupported CLI content",
            Self::UnsupportedTool => "unsupported CLI tool inventory or observation",
            Self::OrphanToolResult => "uncorrelated CLI tool result",
            Self::Limit => "CLI projection limit exceeded",
            Self::SinkRejected => "CLI projection sink rejected event",
            Self::Incomplete => "incomplete CLI stream",
        })
    }
}
impl std::error::Error for Error {}

/// Closed diagnostic classes only: never preserve an unrecognized type name or payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordClass {
    System,
    Assistant,
    User,
    Result,
    StreamEvent,
    MessageStart,
    ContentBlockStart,
    ContentBlockDelta,
    ContentBlockStop,
    MessageDelta,
    MessageStop,
    Ping,
    EndOfStream,
    #[serde(other)]
    Unknown,
}
/// Bounded sequence facts only: no message IDs, content, schema labels or arbitrary values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamDiagnostic {
    pub problem: Error,
    pub record: RecordClass,
    pub block_index: Option<usize>,
    pub active_message: bool,
    pub known_blocks: usize,
    pub block_finalized: Option<bool>,
}

/// Deserialization diagnostics never contain error text, scalar values or schema labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParseFailureKind {
    UnknownField,
    MissingField,
    InvalidType,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseFailure {
    pub kind: ParseFailureKind,
    /// Hash of a rejected field label only (never an arbitrary value or entire error).
    pub field_sha256: Option<[u8; 32]>,
    #[serde(default)]
    pub message_delta_shapes: Option<MessageDeltaShapes>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueShape {
    Missing,
    Null,
    String,
    Number,
    Bool,
    EmptyObject,
    Object,
    EmptyArray,
    Array,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDeltaShapes {
    pub delta: ValueShape,
    pub container: ValueShape,
    pub stop_details: ValueShape,
    pub stop_reason: ValueShape,
    pub stop_sequence: ValueShape,
    pub context_management: ValueShape,
    pub input_transformations: ValueShape,
}
fn message_delta_shapes(bytes: &[u8]) -> Option<MessageDeltaShapes> {
    // Failure-only, bounded by MAX_LINE_BYTES. Never retain keys or scalar contents.
    use serde_json::Value;
    fn shape(v: Option<&Value>) -> ValueShape {
        match v {
            None => ValueShape::Missing,
            Some(Value::Null) => ValueShape::Null,
            Some(Value::String(_)) => ValueShape::String,
            Some(Value::Number(_)) => ValueShape::Number,
            Some(Value::Bool(_)) => ValueShape::Bool,
            Some(Value::Array(a)) if a.is_empty() => ValueShape::EmptyArray,
            Some(Value::Array(_)) => ValueShape::Array,
            Some(Value::Object(o)) if o.is_empty() => ValueShape::EmptyObject,
            Some(Value::Object(_)) => ValueShape::Object,
        }
    }
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let event = value.get("event")?;
    if event.get("type")?.as_str()? != "message_delta" {
        return None;
    }
    let delta = event.get("delta");
    Some(MessageDeltaShapes {
        delta: shape(delta),
        container: shape(delta.and_then(|d| d.get("container"))),
        stop_details: shape(delta.and_then(|d| d.get("stop_details"))),
        stop_reason: shape(delta.and_then(|d| d.get("stop_reason"))),
        stop_sequence: shape(delta.and_then(|d| d.get("stop_sequence"))),
        context_management: shape(event.get("context_management")),
        input_transformations: shape(event.get("input_transformations")),
    })
}
fn parse_failure(bytes: &[u8]) -> Option<ParseFailure> {
    let result =
        serde_json::from_slice::<Header>(bytes).and_then(|header| match header.kind.as_str() {
            "stream_event" => serde_json::from_slice::<StreamEnvelope>(bytes).map(|_| ()),
            "assistant" => serde_json::from_slice::<MessageEnvelope>(bytes).map(|_| ()),
            "user" => serde_json::from_slice::<UserEnvelope>(bytes).map(|_| ()),
            "result" => serde_json::from_slice::<TerminalRecord>(bytes).map(|_| ()),
            _ => Ok(()),
        });
    let error = result.err()?;
    let text = error.to_string();
    let message_delta_shapes = message_delta_shapes(bytes);
    for (prefix, kind) in [
        ("unknown field `", ParseFailureKind::UnknownField),
        ("missing field `", ParseFailureKind::MissingField),
    ] {
        if let Some(label) = text
            .strip_prefix(prefix)
            .and_then(|rest| rest.split_once('`').map(|(label, _)| label))
        {
            return Some(ParseFailure {
                kind,
                field_sha256: Some(Sha256::digest(label.as_bytes()).into()),
                message_delta_shapes,
            });
        }
    }
    Some(ParseFailure {
        kind: if text.starts_with("invalid type:") {
            ParseFailureKind::InvalidType
        } else {
            ParseFailureKind::Other
        },
        field_sha256: None,
        message_delta_shapes,
    })
}

/// On a rejected presentation record, never degrade an uncorrelated tool observation
/// into a harmless display warning. Permission authority lives on a different channel.
fn tool_record(bytes: &[u8]) -> bool {
    fn tool(value: &serde_json::Value) -> bool {
        value
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|kind| {
                matches!(
                    kind,
                    "tool_use"
                        | "server_tool_use"
                        | "tool_result"
                        | "server_tool_result"
                        | "input_json_delta"
                )
            })
            || (value.get("name").is_some() && value.get("id").is_some())
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return true;
    };
    value
        .pointer("/message/content")
        .and_then(|v| v.as_array())
        .is_some_and(|blocks| blocks.iter().any(tool))
        || value.pointer("/event/content_block").is_some_and(tool)
        || value.pointer("/event/delta").is_some_and(tool)
}

fn record_class(bytes: &[u8]) -> RecordClass {
    #[derive(Deserialize)]
    struct Inner {
        #[serde(rename = "type")]
        kind: RecordClass,
    }
    #[derive(Deserialize)]
    struct Outer {
        #[serde(rename = "type")]
        kind: RecordClass,
        event: Option<Inner>,
    }
    serde_json::from_slice::<Outer>(bytes)
        .map(|r| {
            if r.kind == RecordClass::StreamEvent {
                r.event.map_or(RecordClass::Unknown, |e| e.kind)
            } else {
                r.kind
            }
        })
        .unwrap_or(RecordClass::Unknown)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Text,
    Thinking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Error,
    /// A future result subtype is terminal, but cannot establish success.
    Unknown,
}

/// Missing values stay unknown. No estimates, pricing, account or model metadata.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cache_write_1h: Option<u64>,
}

/// Display-only payloads: intentionally no Serialize or native protocol conversion.
/// Consumers may inspect fields for UI display, but must not log their contents.
#[derive(PartialEq, Eq)]
pub enum Event {
    Context {
        tokens: Option<u64>,
        compact_pre_tokens: Option<u64>,
    },
    Initialized,
    BlockDelta {
        message_id: String,
        block_index: usize,
        kind: TextKind,
        /// UTF-8 byte offset, not a character count.
        offset: usize,
        text: String,
    },
    BlockSnapshot {
        message_id: String,
        block_index: usize,
        kind: TextKind,
        text: String,
    },
    ExternalToolObserved {
        message_id: String,
        block_index: usize,
        tool_id: String,
        name: String,
    },
    ExternalToolResult {
        tool_id: String,
        /// Absent is unknown, not success.
        is_error: Option<bool>,
        text: String,
    },
    Terminal {
        outcome: Outcome,
        usage: Option<Usage>,
        metadata: Option<super::usage_metadata::Metadata>,
    },
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Initialized => "Initialized",
            Self::Context { .. } => "Context",
            Self::BlockDelta { .. } => "BlockDelta(<redacted>)",
            Self::BlockSnapshot { .. } => "BlockSnapshot(<redacted>)",
            Self::ExternalToolObserved { .. } => "ExternalToolObserved(<redacted>)",
            Self::ExternalToolResult { .. } => "ExternalToolResult(<redacted>)",
            Self::Terminal { .. } => "Terminal(<redacted>)",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub initialized: bool,
    pub records: u64,
    pub unknown_records: u64,
    pub projected_bytes: usize,
    pub outcome: Option<Outcome>,
    /// Display loss is not execution uncertainty. No raw rejected content is retained.
    pub presentation_incomplete: bool,
}

/// No Default: the caller must supply the expected canonical lowercase CLI UUID.
/// No Debug: unfinished NDJSON may contain arbitrary private data.
pub struct Decoder {
    session: String,
    allowlist: HashSet<String>,
    inventory: HashSet<String>,
    line: Vec<u8>,
    max_line_bytes: usize,
    max_projected_bytes: usize,
    messages: HashMap<String, MessageState>,
    tools: HashMap<String, ToolState>,
    active: Option<String>,
    preview_unreliable: HashSet<String>,
    snapshots: HashMap<String, [u8; 32]>,
    last_text: Option<(String, usize)>,
    tool_preview_limit: Option<usize>,
    cli_inventory: bool,
    exact_inventory: bool,
    last_text_snapshot: u64,
    last_presentation_problem: u64,
    blocks: usize,
    summary: Summary,
    failed: Option<Error>,
    failed_record: Option<RecordClass>,
    parse_failure: Option<ParseFailure>,
    diagnostic: Option<StreamDiagnostic>,
}

#[derive(Default)]
struct MessageState {
    blocks: BTreeMap<usize, BlockState>,
    started: bool,
    ended: bool,
}

#[derive(PartialEq, Eq)]
enum BlockKind {
    Display(TextKind),
    Tool { id: String, name: String },
}
struct BlockState {
    kind: BlockKind,
    text: String,
    stopped: bool,
    finalized: bool,
}
struct ToolState {
    message_id: String,
    index: usize,
    name: String,
    result: Option<(Option<bool>, [u8; 32])>,
}

impl Decoder {
    /// Empty allowlist means no tools. Inventory may be a subset, never a superset.
    /// Names are exact and case-sensitive; there are no wildcards or implicit tools.
    pub fn new(expected_session: &str, allowed_tools: &[&str]) -> Result<Self, Error> {
        if !canonical_uuid(expected_session) || allowed_tools.len() > MAX_ENTRIES {
            return Err(Error::InvalidConfiguration);
        }
        let mut allowlist = HashSet::new();
        for name in allowed_tools {
            if !valid_id(name) || !allowlist.insert((*name).to_owned()) {
                return Err(Error::InvalidConfiguration);
            }
        }
        Ok(Self {
            session: expected_session.to_owned(),
            allowlist,
            inventory: HashSet::new(),
            line: Vec::new(),
            max_line_bytes: MAX_LINE_BYTES,
            max_projected_bytes: MAX_PROJECTED_BYTES,
            messages: HashMap::new(),
            tools: HashMap::new(),
            active: None,
            preview_unreliable: HashSet::new(),
            snapshots: HashMap::new(),
            last_text: None,
            tool_preview_limit: None,
            cli_inventory: false,
            exact_inventory: false,
            last_text_snapshot: 0,
            last_presentation_problem: 0,
            blocks: 0,
            summary: Summary::default(),
            failed: None,
            failed_record: None,
            parse_failure: None,
            diagnostic: None,
        })
    }

    pub fn summary(&self) -> Summary {
        self.summary
    }

    pub fn failed_record(&self) -> Option<RecordClass> {
        self.failed_record
    }
    pub fn parse_failure(&self) -> Option<ParseFailure> {
        self.parse_failure
    }
    /// Native orchestration cannot silently omit catalog tools or inherit Coding's policy.
    pub(super) fn with_exact_inventory(mut self) -> Self {
        self.exact_inventory = true;
        self.max_line_bytes = super::orchestration_policy::MAX_STREAM_LINE_BYTES;
        self.max_projected_bytes = super::orchestration_policy::MAX_PROJECTED_BYTES;
        self
    }
    pub fn with_cli_tools(mut self) -> Self {
        self.cli_inventory = true;
        self.tool_preview_limit = Some(8192);
        self
    }
    pub fn diagnostic(&self) -> Option<StreamDiagnostic> {
        self.diagnostic
    }

    /// LF/CRLF records; the line bound excludes LF and includes CR. Arbitrarily
    /// large chunks are processed record-by-record without collecting their events.
    /// Sink error details are discarded. A rejected event is NEVER retried.
    pub fn feed<E>(
        &mut self,
        mut bytes: &[u8],
        mut sink: impl FnMut(Event) -> Result<(), E>,
    ) -> Result<(), Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        while !bytes.is_empty() {
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let count = newline.unwrap_or(bytes.len());
            if count > self.max_line_bytes - self.line.len() {
                return self.poison(Error::Limit);
            }
            self.line.reserve_exact(count);
            self.line.extend_from_slice(&bytes[..count]);
            if newline.is_none() {
                break;
            }
            if let Err(error) = self.consume(&mut sink) {
                return self.poison(error);
            }
            bytes = &bytes[count + 1..];
        }
        Ok(())
    }

    /// Accept a complete final JSON record without LF. EOF without a terminal is
    /// incomplete, never successful. Dropping the decoder emits no events.
    pub fn finish<E>(
        mut self,
        mut sink: impl FnMut(Event) -> Result<(), E>,
    ) -> Result<Summary, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if !self.line.is_empty() {
            self.consume(&mut sink)?;
        }
        if self.summary.outcome.is_none() {
            return Err(Error::Incomplete);
        }
        Ok(self.summary)
    }

    fn poison(&mut self, error: Error) -> Result<(), Error> {
        self.line.fill(0);
        self.line.clear();
        self.messages.clear();
        self.tools.clear();
        self.inventory.clear();
        self.active = None;
        self.summary.outcome = None;
        self.failed = Some(error);
        Err(error)
    }

    fn consume<E>(&mut self, sink: &mut impl FnMut(Event) -> Result<(), E>) -> Result<(), Error> {
        // Move the bounded line out so no parsed record is retained between feeds.
        let mut bytes = std::mem::take(&mut self.line);
        let result = (|| {
            let line = std::str::from_utf8(&bytes).map_err(|_| Error::MalformedRecord)?;
            if !line.trim_start_matches([' ', '\t', '\r']).starts_with('{') {
                return Err(Error::MalformedRecord);
            }
            let header: Header = parse(line)?;
            if let Some(parent) = &header.parent_tool_use_id {
                // Opaque CLI-owned child telemetry may have its own session identity.
                // It must name an observed root call and can never mutate the root's
                // transcript, actions or completion, including after the root result.
                if !self.cli_inventory || !self.tools.contains_key(parent) {
                    return Err(Error::Identity);
                }
                increment(&mut self.summary.records)?;
                return increment(&mut self.summary.unknown_records);
            }
            if let Some(session) = &header.session_id
                && session != &self.session
            {
                return Err(Error::Identity);
            }
            let known = matches!(
                header.kind.as_str(),
                "system" | "assistant" | "user" | "stream_event" | "result"
            );
            if known && header.session_id.as_deref() != Some(self.session.as_str()) {
                return Err(Error::Identity);
            }
            increment(&mut self.summary.records)?;
            // Result ends execution, not the stdout stream. Drain trailing telemetry,
            // but never accept another result, content mutation or a changed identity.
            if self.summary.outcome.is_some() {
                if matches!(header.kind.as_str(), "system" | "rate_limit_event")
                    && header.subtype.as_deref() != Some("init")
                {
                    return increment(&mut self.summary.unknown_records);
                }
                return Err(Error::Sequence);
            }
            if header.kind == "system" && header.subtype.as_deref() == Some("init") {
                if self.summary.initialized {
                    return Err(Error::Sequence);
                }
                let init: Init = parse(line)?;
                if init.tools.len() > MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                for tool in init.tools {
                    if !valid_id(&tool)
                        || (!self.cli_inventory && !self.allowlist.contains(&tool))
                        || !self.inventory.insert(tool)
                    {
                        return Err(Error::UnsupportedTool);
                    }
                }
                if header.mutates() {
                    return Err(Error::UnsupportedContent);
                }
                if self.exact_inventory && self.inventory != self.allowlist {
                    return Err(Error::UnsupportedTool);
                }
                self.summary.initialized = true;
                return emit(sink, Event::Initialized);
            }
            if self.exact_inventory
                && header.kind == "system"
                && header.subtype.as_deref() == Some("compact_boundary")
            {
                if !self.summary.initialized {
                    return Err(Error::MissingInit);
                }
                if header.mutates() {
                    return Err(Error::UnsupportedContent);
                }
                #[derive(Deserialize)]
                struct Boundary {
                    compact_metadata: Metadata,
                }
                #[derive(Deserialize)]
                struct Metadata {
                    trigger: String,
                    pre_tokens: u64,
                }
                let boundary: Boundary = parse(line)?;
                if !matches!(
                    boundary.compact_metadata.trigger.as_str(),
                    "auto" | "manual"
                ) || boundary.compact_metadata.pre_tokens > i64::MAX as u64
                {
                    return Err(Error::MalformedRecord);
                }
                return emit(
                    sink,
                    Event::Context {
                        tokens: None,
                        compact_pre_tokens: Some(boundary.compact_metadata.pre_tokens),
                    },
                );
            }
            match header.kind.as_str() {
                "assistant" | "user" | "stream_event" | "result" => {
                    if !self.summary.initialized {
                        return Err(Error::MissingInit);
                    }
                }
                _ => {
                    // Unknown observability is never execution authority.
                    self.summary.presentation_incomplete |= header.mutates();
                    if header.mutates() {
                        self.last_presentation_problem = self.summary.records;
                    }
                    return increment(&mut self.summary.unknown_records);
                }
            }
            // Never silently accept a second, contradictory mutation surface.
            if header.content.0
                || header.delta.0
                || (header.kind != "stream_event" && header.event.0)
                || (!matches!(header.kind.as_str(), "assistant" | "user") && header.message.0)
                || (header.kind != "user" && header.tool_use_result.0)
            {
                return Err(Error::UnsupportedContent);
            }
            match header.kind.as_str() {
                "assistant" => {
                    let record: MessageEnvelope = parse(line)?;
                    if let Some(uuid) = &record.uuid {
                        if !valid_id(uuid) {
                            return Err(Error::Identity);
                        }
                        let digest: [u8; 32] = Sha256::digest(line.as_bytes()).into();
                        if let Some(prior) = self.snapshots.get(uuid) {
                            return if prior == &digest {
                                Ok(())
                            } else {
                                Err(Error::Sequence)
                            };
                        }
                        if self.snapshots.len() >= MAX_ENTRIES {
                            return Err(Error::Limit);
                        }
                        self.snapshot(record.message, sink)?;
                        self.snapshots.insert(uuid.clone(), digest);
                        Ok(())
                    } else {
                        self.snapshot(record.message, sink)
                    }
                }
                "user" => {
                    let record: UserEnvelope = parse(line)?;
                    if record
                        .message
                        .role
                        .as_deref()
                        .is_some_and(|role| role != "user")
                    {
                        return Err(Error::UnsupportedContent);
                    }
                    if record.message.id.as_deref().is_some_and(|id| !valid_id(id)) {
                        return Err(Error::Identity);
                    }
                    if record.message.content.len() > MAX_ENTRIES {
                        return Err(Error::Limit);
                    }
                    for block in record.message.content {
                        self.tool_result(block, sink)?;
                    }
                    Ok(())
                }
                "stream_event" => {
                    let record: StreamEnvelope = parse(line)?;
                    self.stream(record.event, sink)
                }
                "result" => {
                    let record: TerminalRecord = parse(line)?;
                    let outcome = match record.subtype.as_str() {
                        "success" if !record.is_error => Outcome::Success,
                        "success"
                        | "error_during_execution"
                        | "error_max_turns"
                        | "error_max_budget_usd"
                        | "error_max_structured_output_retries" => Outcome::Error,
                        _ => Outcome::Unknown,
                    };
                    if outcome == Outcome::Success
                        && self.tools.values().any(|tool| tool.result.is_none())
                    {
                        return Err(Error::OrphanToolResult);
                    }
                    if self.active.take().is_some() {
                        self.summary.presentation_incomplete = true;
                    }
                    if self.summary.presentation_incomplete || self.last_text.is_none() {
                        match record.result {
                            Some(serde_json::Value::String(text)) => {
                                self.reconcile_result(text, sink)?
                            }
                            None | Some(serde_json::Value::Null) => {}
                            Some(_) => self.summary.presentation_incomplete = true,
                        }
                    }
                    emit(
                        sink,
                        Event::Terminal {
                            outcome,
                            usage: record.usage,
                            metadata: self
                                .exact_inventory
                                .then(|| super::usage_metadata::parse(line))
                                .flatten(),
                        },
                    )?;
                    self.summary.outcome = Some(outcome);
                    Ok(())
                }
                _ => unreachable!(),
            }
        })();
        if let Err(problem) = result {
            #[derive(Deserialize)]
            struct Index {
                index: Option<usize>,
            }
            #[derive(Deserialize)]
            struct Envelope {
                event: Option<Index>,
            }
            let index = serde_json::from_slice::<Envelope>(&bytes)
                .ok()
                .and_then(|e| e.event.and_then(|e| e.index))
                .filter(|i| *i < MAX_ENTRIES);
            let message = self.active.as_ref().and_then(|id| self.messages.get(id));
            self.diagnostic = Some(StreamDiagnostic {
                problem,
                record: record_class(&bytes),
                block_index: index,
                active_message: message.is_some(),
                known_blocks: message.map_or(0, |m| m.blocks.len()),
                block_finalized: index
                    .and_then(|i| message.and_then(|m| m.blocks.get(&i)))
                    .map(|b| b.finalized),
            });
            self.failed_record = Some(record_class(&bytes));
            if result == Err(Error::MalformedRecord) {
                self.parse_failure = parse_failure(&bytes);
            }
        }
        // Only presentation failures are recoverable. Identity, tool authority,
        // terminal contradictions, resource bounds and sink/storage failure stay hard.
        let recoverable = self.summary.initialized
            && self.summary.outcome.is_none()
            && matches!(
                result,
                Err(Error::MalformedRecord | Error::Sequence | Error::UnsupportedContent)
            )
            && !tool_record(&bytes)
            && serde_json::from_slice::<Header>(&bytes).is_ok_and(|h| {
                matches!(h.kind.as_str(), "assistant" | "stream_event")
                    && h.session_id.as_deref() == Some(self.session.as_str())
                    && h.parent_tool_use_id.is_none()
            });
        if recoverable {
            self.summary.presentation_incomplete = true;
            self.last_presentation_problem = self.summary.records;
            if let Some(id) = &self.active {
                self.preview_unreliable.insert(id.clone());
            }
        }
        bytes.fill(0);
        bytes.clear();
        self.line = bytes;
        if recoverable { Ok(()) } else { result }
    }

    fn reconcile_result<E>(
        &mut self,
        text: String,
        sink: &mut impl FnMut(Event) -> Result<(), E>,
    ) -> Result<(), Error> {
        if text.is_empty() {
            return Ok(());
        }
        let last = self
            .last_text
            .as_ref()
            .and_then(|(id, index)| self.messages.get(id).and_then(|m| m.blocks.get(index)));
        // Prefer complete per-block output received after the presentation problem.
        // A result string can combine several text blocks; never append it to the last
        // complete block or overwrite an earlier complete message after later display loss.
        if last.is_some_and(|block| {
            block.finalized && self.last_text_snapshot > self.last_presentation_problem
        }) {
            return Ok(());
        }
        let replace_preview = last.is_some_and(|block| !block.finalized);
        let (message_id, block_index) = if replace_preview {
            self.last_text.clone().ok_or(Error::Sequence)?
        } else {
            let mut id = "pipkin_cli_result".to_owned();
            while self.messages.contains_key(&id) {
                id.push('_');
            }
            if !valid_id(&id) {
                return Err(Error::Limit);
            }
            (id, 0)
        };
        self.charge(text.len())?;
        emit(
            sink,
            Event::BlockSnapshot {
                message_id: message_id.clone(),
                block_index,
                kind: TextKind::Text,
                text,
            },
        )?;
        // Final result text replaces the message's text preview as a whole, while
        // thinking/tool blocks retain their identities. No content-based deduplication.
        if replace_preview && let Some(message) = self.messages.get(&message_id) {
            for (&index, block) in &message.blocks {
                if index != block_index && matches!(block.kind, BlockKind::Display(TextKind::Text))
                {
                    emit(
                        sink,
                        Event::BlockSnapshot {
                            message_id: message_id.clone(),
                            block_index: index,
                            kind: TextKind::Text,
                            text: String::new(),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    fn message(&mut self, id: &str) -> Result<(), Error> {
        if !valid_id(id) {
            return Err(Error::Identity);
        }
        if !self.messages.contains_key(id) {
            if self.messages.len() == MAX_ENTRIES {
                return Err(Error::Limit);
            }
            self.messages.insert(id.to_owned(), MessageState::default());
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        if bytes > self.max_projected_bytes - self.summary.projected_bytes {
            return Err(Error::Limit);
        }
        self.summary.projected_bytes += bytes;
        Ok(())
    }

    fn insert_block(
        &mut self,
        message: &str,
        index: usize,
        block: BlockState,
    ) -> Result<(), Error> {
        if index >= MAX_ENTRIES || self.blocks == MAX_ENTRIES {
            return Err(Error::Limit);
        }
        let state = self.messages.get_mut(message).ok_or(Error::Sequence)?;
        if state.blocks.insert(index, block).is_some() {
            return Err(Error::Sequence);
        }
        self.blocks += 1;
        Ok(())
    }

    fn observe_tool<E>(
        &mut self,
        message: &str,
        index: usize,
        id: &str,
        name: &str,
        sink: &mut impl FnMut(Event) -> Result<(), E>,
    ) -> Result<(), Error> {
        if !valid_id(id) || !valid_id(name) {
            return Err(Error::Identity);
        }
        if !self.inventory.contains(name) {
            return Err(Error::UnsupportedTool);
        }
        if let Some(old) = self.tools.get(id) {
            if old.message_id != message || old.index != index || old.name != name {
                return Err(Error::Identity);
            }
            return Ok(());
        }
        if self.tools.len() == MAX_ENTRIES {
            return Err(Error::Limit);
        }
        self.tools.insert(
            id.to_owned(),
            ToolState {
                message_id: message.to_owned(),
                index,
                name: name.to_owned(),
                result: None,
            },
        );
        emit(
            sink,
            Event::ExternalToolObserved {
                message_id: message.to_owned(),
                block_index: index,
                tool_id: id.to_owned(),
                name: name.to_owned(),
            },
        )
    }

    fn snapshot<E>(
        &mut self,
        message: AssistantMessage,
        sink: &mut impl FnMut(Event) -> Result<(), E>,
    ) -> Result<(), Error> {
        if message
            .role
            .as_deref()
            .is_some_and(|role| role != "assistant")
        {
            return Err(Error::UnsupportedContent);
        }
        self.message(&message.id)?;
        if self.exact_inventory {
            let usage = message.usage.unwrap_or_default();
            let tokens = usage
                .input_tokens
                .zip(usage.cache_read_input_tokens)
                .zip(usage.cache_creation_input_tokens)
                .and_then(|((a, b), c)| a.checked_add(b)?.checked_add(c))
                .filter(|n| *n <= i64::MAX as u64);
            emit(
                sink,
                Event::Context {
                    tokens,
                    compact_pre_tokens: None,
                },
            )?;
        }
        // Another message's final while a stream is active is ambiguous.
        if self.active.as_deref().is_some_and(|id| id != message.id) {
            return Err(Error::Identity);
        }
        if message.content.len() > MAX_ENTRIES {
            return Err(Error::Limit);
        }
        let old = &self.messages[&message.id];
        // CLI assistant envelopes finalize individual blocks, before block_stop.
        // Combined snapshots remain supported for saved/older fixture envelopes.
        let existing_tool = message.content.first().and_then(|content| match content {
            ContentBlock::ToolUse { id, .. } => self
                .tools
                .get(id)
                .filter(|tool| tool.message_id == message.id)
                .map(|tool| tool.index),
            _ => None,
        });
        let base = if message.content.len() == 1 {
            if let Some(index) = existing_tool {
                index
            } else if old.started && !self.preview_unreliable.contains(&message.id) {
                old.blocks.last_key_value().map_or(0, |(i, _)| *i)
            } else {
                old.blocks
                    .iter()
                    .find(|(_, b)| !b.finalized)
                    .map_or(old.blocks.len(), |(i, _)| *i)
            }
        } else {
            0
        };
        for (offset, content) in message.content.into_iter().enumerate() {
            let index = base + offset;
            if index >= 256 {
                return Err(Error::Limit);
            }
            let mut block = content.into_state();
            let previous = self.messages[&message.id].blocks.get(&index);
            if previous.is_none() && self.blocks == MAX_ENTRIES {
                return Err(Error::Limit);
            }
            if let Some(previous) = previous {
                if previous.kind != block.kind {
                    return Err(
                        if matches!(previous.kind, BlockKind::Tool { .. })
                            || matches!(block.kind, BlockKind::Tool { .. })
                        {
                            Error::Identity
                        } else {
                            Error::Sequence
                        },
                    );
                }
                if previous.finalized && previous.text != block.text {
                    return Err(Error::Sequence);
                }
                block.stopped = previous.stopped;
            }
            block.finalized = true;
            let changed = previous.is_none_or(|previous| previous.text != block.text);
            if block.kind == BlockKind::Display(TextKind::Text) {
                self.last_text = Some((message.id.clone(), index));
                self.last_text_snapshot = self.summary.records;
            }
            match &block.kind {
                BlockKind::Display(kind) if changed => {
                    self.charge(block.text.len())?;
                    emit(
                        sink,
                        Event::BlockSnapshot {
                            message_id: message.id.clone(),
                            block_index: index,
                            kind: *kind,
                            text: block.text.clone(),
                        },
                    )?;
                }
                BlockKind::Tool { id, name } => {
                    self.observe_tool(&message.id, index, id, name, sink)?
                }
                _ => {}
            }
            if self.messages[&message.id].blocks.contains_key(&index) {
                self.messages
                    .get_mut(&message.id)
                    .ok_or(Error::Sequence)?
                    .blocks
                    .insert(index, block);
            } else {
                self.insert_block(&message.id, index, block)?;
            }
        }
        Ok(())
    }

    fn stream<E>(
        &mut self,
        event: StreamEvent,
        sink: &mut impl FnMut(Event) -> Result<(), E>,
    ) -> Result<(), Error> {
        if let StreamEvent::MessageStart { message } = event {
            if self.active.is_some()
                || !message.content.is_empty()
                || message
                    .role
                    .as_deref()
                    .is_some_and(|role| role != "assistant")
            {
                return Err(Error::Sequence);
            }
            self.message(&message.id)?;
            let state = self.messages.get_mut(&message.id).ok_or(Error::Sequence)?;
            if state.started || state.ended {
                return Err(Error::Sequence);
            }
            state.started = true;
            self.active = Some(message.id);
            return Ok(());
        }
        if matches!(event, StreamEvent::Ping | StreamEvent::Unknown) {
            return Ok(());
        }
        let id = self.active.clone().ok_or(Error::Sequence)?;
        if self.preview_unreliable.contains(&id) {
            if matches!(event, StreamEvent::MessageStop) {
                self.messages.get_mut(&id).ok_or(Error::Sequence)?.ended = true;
                self.active = None;
            }
            return Ok(());
        }
        match event {
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let state = &self.messages[&id];
                if state.ended || state.blocks.contains_key(&index) || index != state.blocks.len() {
                    return Err(Error::Sequence);
                }
                if index >= 256 || self.blocks == MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                let mut block = content_block.into_state();
                block.stopped = false;
                if block.kind == BlockKind::Display(TextKind::Text) {
                    self.last_text = Some((id.clone(), index));
                }
                match &block.kind {
                    BlockKind::Display(kind) => {
                        self.charge(block.text.len())?;
                        emit(
                            sink,
                            Event::BlockDelta {
                                message_id: id.clone(),
                                block_index: index,
                                kind: *kind,
                                offset: 0,
                                text: block.text.clone(),
                            },
                        )?;
                    }
                    BlockKind::Tool { id: tool_id, name } => {
                        self.observe_tool(&id, index, tool_id, name, sink)?
                    }
                }
                self.insert_block(&id, index, block)
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                let state = &self.messages[&id];
                let block = state.blocks.get(&index).ok_or(Error::Sequence)?;
                if state.ended || block.stopped || block.finalized {
                    return Err(Error::Sequence);
                }
                let (kind, text) = match delta {
                    Delta::Text { text } => (TextKind::Text, text),
                    Delta::Thinking { thinking, .. } => (TextKind::Thinking, thinking),
                    Delta::InputJson { .. } if matches!(block.kind, BlockKind::Tool { .. }) => {
                        return Ok(());
                    }
                    Delta::Signature { .. }
                        if block.kind == BlockKind::Display(TextKind::Thinking) =>
                    {
                        return Ok(());
                    }
                    _ => return Err(Error::UnsupportedContent),
                };
                if block.kind != BlockKind::Display(kind) {
                    return Err(Error::UnsupportedContent);
                }
                let offset = block.text.len();
                self.charge(text.len())?;
                let block = self
                    .messages
                    .get_mut(&id)
                    .and_then(|state| state.blocks.get_mut(&index))
                    .ok_or(Error::Sequence)?;
                block.text.reserve_exact(text.len());
                block.text.push_str(&text);
                if text.is_empty() {
                    return Ok(());
                }
                emit(
                    sink,
                    Event::BlockDelta {
                        message_id: id,
                        block_index: index,
                        kind,
                        offset,
                        text,
                    },
                )
            }
            StreamEvent::ContentBlockStop { index } => {
                let block = self
                    .messages
                    .get_mut(&id)
                    .and_then(|state| state.blocks.get_mut(&index))
                    .ok_or(Error::Sequence)?;
                // Snapshot can arrive before block_stop; it does not close streaming.
                if block.stopped {
                    return Err(Error::Sequence);
                }
                self.messages
                    .get_mut(&id)
                    .and_then(|state| state.blocks.get_mut(&index))
                    .ok_or(Error::Sequence)?
                    .stopped = true;
                Ok(())
            }
            StreamEvent::MessageDelta { .. } => Ok(()),
            StreamEvent::MessageStop => {
                if self.messages[&id]
                    .blocks
                    .values()
                    .any(|block| !block.stopped)
                {
                    return Err(Error::Sequence);
                }
                self.messages.get_mut(&id).ok_or(Error::Sequence)?.ended = true;
                self.active = None;
                Ok(())
            }
            _ => Err(Error::Sequence),
        }
    }

    fn tool_result<E>(
        &mut self,
        result: ResultBlock,
        sink: &mut impl FnMut(Event) -> Result<(), E>,
    ) -> Result<(), Error> {
        let ResultBlock::ToolResult {
            tool_use_id,
            is_error,
            content,
        } = result;
        if !valid_id(&tool_use_id) {
            return Err(Error::Identity);
        }
        let previous = self
            .tools
            .get(&tool_use_id)
            .ok_or(Error::OrphanToolResult)?;
        let mut text = content.display_text();
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if let Some((old_error, old_text)) = &previous.result {
            return if old_error == &is_error && old_text == &digest {
                Ok(())
            } else {
                Err(Error::Sequence)
            };
        }
        // Native orchestration presents the actual durable MCP receipt. Keep the
        // SDK correlation digest, but do not duplicate or charge its echoed output.
        if self.exact_inventory {
            text.clear();
        }
        if let Some(limit) = self.tool_preview_limit {
            // Reserve half the projection budget for action identities and the final
            // answer. Verbose build/search output must not terminate useful tool work.
            let available = limit
                .min((self.max_projected_bytes / 2).saturating_sub(self.summary.projected_bytes));
            if text.len() > available {
                const NOTE: &str = "\n[CLI tool output truncated]";
                let mut end = available.saturating_sub(NOTE.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                if available >= NOTE.len() {
                    text.push_str(NOTE);
                }
                self.summary.presentation_incomplete = true;
                self.last_presentation_problem = self.summary.records;
            }
        }
        self.charge(text.len())?;
        self.tools
            .get_mut(&tool_use_id)
            .ok_or(Error::OrphanToolResult)?
            .result = Some((is_error, digest));
        emit(
            sink,
            Event::ExternalToolResult {
                tool_id: tool_use_id,
                is_error,
                text,
            },
        )
    }
}

fn emit<E>(sink: &mut impl FnMut(Event) -> Result<(), E>, event: Event) -> Result<(), Error> {
    sink(event).map_err(|_| Error::SinkRejected)
}
fn increment(value: &mut u64) -> Result<(), Error> {
    *value = value.checked_add(1).ok_or(Error::Limit)?;
    Ok(())
}
fn canonical_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_BYTES && id.bytes().all(|byte| byte.is_ascii_graphic())
}
fn parse<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, Error> {
    serde_json::from_str(line).map_err(|_| Error::MalformedRecord)
}

// Serde structs otherwise accept positional arrays as well as objects. The CLI
// contract uses objects; do not admit array-shaped messages or usage records.
fn object<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    de.deserialize_map(Object(std::marker::PhantomData))
}
fn optional_usage<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Usage>, D::Error> {
    let value = serde_json::Value::deserialize(de)?;
    if !value.is_object() {
        return Ok(None);
    }
    let count = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_u64)
            .filter(|n| *n <= i64::MAX as u64)
    };
    let output_tokens = count(value.get("output_tokens"));
    let cache_creation_input_tokens = count(value.get("cache_creation_input_tokens"));
    let reasoning_tokens = count(value.pointer("/output_tokens_details/thinking_tokens"))
        .filter(|n| output_tokens.is_some_and(|total| *n <= total));
    let cache_write_1h = count(value.pointer("/cache_creation/ephemeral_1h_input_tokens"))
        .filter(|n| cache_creation_input_tokens.is_some_and(|total| *n <= total));
    let usage = Usage {
        input_tokens: count(value.get("input_tokens")),
        output_tokens,
        cache_read_input_tokens: count(value.get("cache_read_input_tokens")),
        cache_creation_input_tokens,
        reasoning_tokens,
        cache_write_1h,
    };
    Ok((usage != Usage::default()).then_some(usage))
}

// Typed deserialization deliberately skips unknown metadata rather than allocating
// arbitrary JSON Values. Present detects even null mutation fields on unknown types.
#[derive(Default)]
struct Present(bool);
impl<'de> Deserialize<'de> for Present {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        IgnoredAny::deserialize(de)?;
        Ok(Self(true))
    }
}
fn optional_session<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    let id = String::deserialize(de)?;
    if !canonical_uuid(&id) {
        return Err(serde::de::Error::custom("invalid identity"));
    }
    Ok(Some(id))
}
fn discard_text<'de, D: Deserializer<'de>>(de: D) -> Result<(), D::Error> {
    struct Text;
    impl serde::de::Visitor<'_> for Text {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("text")
        }
        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
    }
    de.deserialize_str(Text)
}

#[derive(Deserialize)]
struct Header {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "optional_session")]
    session_id: Option<String>,
    subtype: Option<String>,
    parent_tool_use_id: Option<String>,
    #[serde(default)]
    message: Present,
    #[serde(default)]
    event: Present,
    #[serde(default)]
    content: Present,
    #[serde(default)]
    delta: Present,
    #[serde(default)]
    tool_use_result: Present,
}
impl Header {
    fn mutates(&self) -> bool {
        self.message.0 || self.event.0 || self.content.0 || self.delta.0 || self.tool_use_result.0
    }
}
#[derive(Deserialize)]
struct Init {
    tools: Vec<String>,
}
#[derive(Deserialize)]
struct MessageEnvelope {
    uuid: Option<String>,
    #[serde(deserialize_with = "object")]
    message: AssistantMessage,
}
#[derive(Deserialize)]
struct AssistantMessage {
    #[serde(default, deserialize_with = "optional_usage")]
    usage: Option<Usage>,
    id: String,
    role: Option<String>,
    content: Vec<ContentBlock>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(rename = "input")]
        _input: IgnoredAny,
    },
}
impl ContentBlock {
    fn into_state(self) -> BlockState {
        let (kind, text) = match self {
            Self::Text { text } => (BlockKind::Display(TextKind::Text), text),
            Self::Thinking { thinking } => (BlockKind::Display(TextKind::Thinking), thinking),
            Self::ToolUse { id, name, .. } => (BlockKind::Tool { id, name }, String::new()),
        };
        BlockState {
            kind,
            text,
            stopped: true,
            finalized: false,
        }
    }
}
#[derive(Deserialize)]
struct StreamEnvelope {
    event: StreamEvent,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    MessageStart {
        #[serde(deserialize_with = "object")]
        message: AssistantMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: Delta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        #[serde(rename = "delta")]
        _delta: IgnoredAny,
        #[serde(rename = "usage")]
        _usage: Option<IgnoredAny>,
        // CLI-owned context telemetry, never native compaction/content or authority.
        #[serde(default, rename = "context_management")]
        _context_management: Option<IgnoredAny>,
        #[serde(default, rename = "input_transformations")]
        _input_transformations: Option<IgnoredAny>,
    },
    MessageStop,
    Ping,
    #[serde(other)]
    Unknown,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking {
        thinking: String,
        // Display telemetry is not usage, content or authority. Discard it without
        // imposing a schema on values this adapter never consumes.
        #[serde(default, rename = "estimated_tokens")]
        _estimated_tokens: Option<IgnoredAny>,
    },
    #[serde(rename = "input_json_delta")]
    InputJson {
        #[serde(rename = "partial_json", deserialize_with = "discard_text")]
        _partial_json: (),
    },
    #[serde(rename = "signature_delta")]
    Signature {
        #[serde(rename = "signature", deserialize_with = "discard_text")]
        _signature: (),
    },
}
#[derive(Deserialize)]
struct UserEnvelope {
    #[serde(deserialize_with = "object")]
    message: UserMessage,
}
#[derive(Deserialize)]
struct UserMessage {
    id: Option<String>,
    role: Option<String>,
    content: Vec<ResultBlock>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResultBlock {
    ToolResult {
        tool_use_id: String,
        // The published client-tool result protocol defaults an omitted flag to
        // reported success. Explicit null remains unknown; this is NOT action proof.
        #[serde(default = "reported_success_by_default")]
        is_error: Option<bool>,
        content: ResultContent,
    },
}
fn reported_success_by_default() -> Option<bool> {
    Some(false)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResultContent {
    Text(String),
    Blocks(Vec<ResultText>),
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResultText {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}
impl ResultContent {
    fn display_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Blocks(blocks) => {
                let mut text = String::new();
                for block in blocks {
                    match block {
                        ResultText::Text { text: part } => {
                            text.reserve_exact(part.len());
                            text.push_str(&part);
                        }
                        ResultText::Other => {
                            text.push_str("\n[Non-text CLI tool output omitted]\n")
                        }
                    }
                }
                text
            }
        }
    }
}
#[derive(Deserialize)]
struct TerminalRecord {
    subtype: String,
    is_error: bool,
    #[serde(default, deserialize_with = "optional_usage")]
    usage: Option<Usage>,
    #[serde(default)]
    result: Option<serde_json::Value>,
}

#[cfg(test)]
#[path = "events/tests.rs"]
mod tests;

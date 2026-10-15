//! Pure, bounded display projection of the CLI 2.1.296 stream-json shapes.
//!
//! Qualification source: the adjacent phase-0 `stream.rs`, the probe and
//! `docs/claude-cli-bridge-evidence.md`. Tests are synthetic, not live qualification.
//! This is deliberately NOT a native message/tool-call decoder or execution authority.
//! Inputs, signatures, account metadata, stderr and terminal answer summaries are
//! discarded. Nothing here performs I/O, resolves credentials, or executes tools.
//!
//! Contract: deltas are ordered, exactly-once byte-offset appends keyed by message ID
//! and block index. The transport must not replay delta records (they have no reliable
//! sequence number). A first assistant snapshot replaces differing partial blocks;
//! identical blocks emit nothing. Identical repeated final snapshots are no-ops;
//! conflicting repeated finals and deltas after a final fail closed. A replacement
//! MUST replace, never append. Events remain provisional until `finish` succeeds AND
//! the supervisor independently validates process exit. Sink acceptance is synchronous:
//! no internal event queue, retry, or silent drop. Any error permanently poisons the
//! decoder; previously accepted events cannot be rolled back by this module.
//!
//! One decoder is one turn. Unknown envelopes are counted, never terminal; unknown
//! content mutations and nested/subagent streams are unsupported and fail closed.
//! Tool results must follow an observed, inventory-authorized call; orphan results
//! are rejected immediately (no untrusted pending-result buffer). Tool observations
//! contain no arguments. Debug output is redacted, even for display payloads.

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
}

/// Display-only payloads: intentionally no Serialize or native protocol conversion.
/// Consumers may inspect fields for UI display, but must not log their contents.
#[derive(PartialEq, Eq)]
pub enum Event {
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
    },
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Initialized => "Initialized",
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
}

/// No Default: the caller must supply the expected canonical lowercase CLI UUID.
/// No Debug: unfinished NDJSON may contain arbitrary private data.
pub struct Decoder {
    session: String,
    allowlist: HashSet<String>,
    inventory: HashSet<String>,
    line: Vec<u8>,
    messages: HashMap<String, MessageState>,
    tools: HashMap<String, ToolState>,
    active: Option<String>,
    blocks: usize,
    summary: Summary,
    failed: Option<Error>,
    failed_record: Option<RecordClass>,
    parse_failure: Option<ParseFailure>,
}

#[derive(Default)]
struct MessageState {
    blocks: BTreeMap<usize, BlockState>,
    started: bool,
    finalized: bool,
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
}
struct ToolState {
    message_id: String,
    index: usize,
    name: String,
    result: Option<(Option<bool>, String)>,
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
            messages: HashMap::new(),
            tools: HashMap::new(),
            active: None,
            blocks: 0,
            summary: Summary::default(),
            failed: None,
            failed_record: None,
            parse_failure: None,
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
            if count > MAX_LINE_BYTES - self.line.len() {
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
            if self.summary.outcome.is_some() {
                return Err(Error::Sequence);
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
            if header.parent_tool_use_id.is_some() {
                return Err(Error::UnsupportedContent);
            }
            increment(&mut self.summary.records)?;
            if header.kind == "system" && header.subtype.as_deref() == Some("init") {
                if self.summary.initialized {
                    return Err(Error::Sequence);
                }
                let init: Init = parse(line)?;
                if init.tools.len() > MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                for tool in init.tools {
                    if !self.allowlist.contains(&tool) || !self.inventory.insert(tool) {
                        return Err(Error::UnsupportedTool);
                    }
                }
                if header.mutates() {
                    return Err(Error::UnsupportedContent);
                }
                self.summary.initialized = true;
                return emit(sink, Event::Initialized);
            }
            match header.kind.as_str() {
                "assistant" | "user" | "stream_event" | "result" => {
                    if !self.summary.initialized {
                        return Err(Error::MissingInit);
                    }
                }
                _ => {
                    if header.mutates() {
                        return Err(Error::UnsupportedContent);
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
                    self.snapshot(record.message, sink)
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
                    if self.active.is_some()
                        || (outcome == Outcome::Success
                            && self.tools.values().any(|tool| tool.result.is_none()))
                    {
                        return Err(Error::Sequence);
                    }
                    emit(
                        sink,
                        Event::Terminal {
                            outcome,
                            usage: record.usage,
                        },
                    )?;
                    self.summary.outcome = Some(outcome);
                    Ok(())
                }
                _ => unreachable!(),
            }
        })();
        if result.is_err() {
            self.failed_record = Some(record_class(&bytes));
            if result == Err(Error::MalformedRecord) {
                self.parse_failure = parse_failure(&bytes);
            }
        }
        bytes.fill(0);
        bytes.clear();
        self.line = bytes;
        result
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
        if bytes > MAX_PROJECTED_BYTES - self.summary.projected_bytes {
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
        // Another message's final while a stream is active is ambiguous.
        if self.active.as_deref().is_some_and(|id| id != message.id) {
            return Err(Error::Identity);
        }
        if message.content.len() > MAX_ENTRIES {
            return Err(Error::Limit);
        }
        let old = &self.messages[&message.id];
        if old
            .blocks
            .keys()
            .any(|index| *index >= message.content.len())
            || (old.finalized && old.blocks.len() != message.content.len())
        {
            return Err(Error::Sequence);
        }
        let finalized = old.finalized;
        for (index, content) in message.content.into_iter().enumerate() {
            let mut block = content.into_state();
            let previous = self.messages[&message.id].blocks.get(&index);
            if previous.is_none() && self.blocks == MAX_ENTRIES {
                return Err(Error::Limit);
            }
            if let Some(previous) = previous {
                if previous.kind != block.kind || (finalized && previous.text != block.text) {
                    return Err(Error::Sequence);
                }
                block.stopped = previous.stopped;
            }
            let changed = previous.is_none_or(|previous| previous.text != block.text);
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
        self.messages
            .get_mut(&message.id)
            .ok_or(Error::Sequence)?
            .finalized = true;
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
            if state.started || state.finalized {
                return Err(Error::Sequence);
            }
            state.started = true;
            self.active = Some(message.id);
            return Ok(());
        }
        if matches!(event, StreamEvent::Ping) {
            return Ok(());
        }
        let id = self.active.clone().ok_or(Error::Sequence)?;
        match event {
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let state = &self.messages[&id];
                if state.finalized
                    || state.blocks.contains_key(&index)
                    || index != state.blocks.len()
                {
                    return Err(Error::Sequence);
                }
                if index >= MAX_ENTRIES || self.blocks == MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                let mut block = content_block.into_state();
                block.stopped = false;
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
                if state.finalized || block.stopped {
                    return Err(Error::Sequence);
                }
                let (kind, text) = match delta {
                    Delta::Text { text } => (TextKind::Text, text),
                    Delta::Thinking { thinking } => (TextKind::Thinking, thinking),
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
        let text = content.display_text();
        if let Some((old_error, old_text)) = &previous.result {
            return if old_error == &is_error && old_text == &text {
                Ok(())
            } else {
                Err(Error::Sequence)
            };
        }
        self.charge(text.len())?;
        self.tools
            .get_mut(&tool_use_id)
            .ok_or(Error::OrphanToolResult)?
            .result = Some((is_error, text.clone()));
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
    #[derive(Deserialize)]
    struct ObjectUsage(#[serde(deserialize_with = "object")] Usage);
    Ok(Option::<ObjectUsage>::deserialize(de)?.map(|usage| usage.0))
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
    #[serde(deserialize_with = "object")]
    message: AssistantMessage,
}
#[derive(Deserialize)]
struct AssistantMessage {
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
        }
    }
}
#[derive(Deserialize)]
struct StreamEnvelope {
    event: StreamEvent,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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
        #[serde(rename = "delta", deserialize_with = "object")]
        _delta: StopDelta,
        #[serde(rename = "usage")]
        _usage: Option<IgnoredAny>,
        // CLI-owned context telemetry, never native compaction/content or authority.
        #[serde(
            default,
            rename = "context_management",
            deserialize_with = "discard_context_management"
        )]
        _context_management: (),
        #[serde(
            default,
            rename = "input_transformations",
            deserialize_with = "empty_transformations"
        )]
        _input_transformations: (),
    },
    MessageStop,
    Ping,
}
fn discard_context_management<'de, D: Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    // Published BetaContextManagementResponse. Only known closed numeric telemetry
    // is accepted; no text, tool arguments or executable instructions are retained.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Metadata {
        applied_edits: Vec<Edit>,
    }
    #[derive(Deserialize)]
    #[serde(tag = "type", deny_unknown_fields)]
    enum Edit {
        #[serde(rename = "clear_thinking_20251015")]
        Thinking {
            #[serde(rename = "cleared_input_tokens")]
            _tokens: u64,
            #[serde(rename = "cleared_thinking_turns")]
            _turns: u64,
        },
        #[serde(rename = "clear_tool_uses_20250919")]
        Tools {
            #[serde(rename = "cleared_input_tokens")]
            _tokens: u64,
            #[serde(rename = "cleared_tool_uses")]
            _uses: u64,
        },
    }
    #[derive(Deserialize)]
    struct ObjectMetadata(#[serde(deserialize_with = "object")] Metadata);
    let metadata = Option::<ObjectMetadata>::deserialize(deserializer)?;
    if metadata.is_some_and(|m| m.0.applied_edits.len() > MAX_ENTRIES) {
        return Err(serde::de::Error::custom("context metadata limit exceeded"));
    }
    Ok(())
}

fn empty_transformations<'de, D: Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    let value = Option::<Vec<IgnoredAny>>::deserialize(deserializer)?;
    if value.is_none_or(|items| items.is_empty()) {
        Ok(())
    } else {
        Err(serde::de::Error::custom(
            "nonempty input transformations are unqualified",
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopDelta {
    #[serde(default, rename = "container")]
    _container: (),
    #[serde(default, rename = "stop_details")]
    _stop_details: (),
    #[serde(
        default,
        rename = "stop_reason",
        deserialize_with = "discard_optional_text"
    )]
    _stop_reason: (),
    #[serde(
        default,
        rename = "stop_sequence",
        deserialize_with = "discard_optional_text"
    )]
    _stop_sequence: (),
}
fn discard_optional_text<'de, D: Deserializer<'de>>(de: D) -> Result<(), D::Error> {
    // Stop metadata is discarded immediately after deserialization.
    let _ = Option::<String>::deserialize(de)?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Delta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
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
    Text { text: String },
}
impl ResultContent {
    fn display_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Blocks(blocks) => {
                let mut text = String::new();
                for ResultText::Text { text: part } in blocks {
                    text.reserve_exact(part.len());
                    text.push_str(&part);
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
    #[serde(default, rename = "result", deserialize_with = "discard_text")]
    _result: (),
}

#[cfg(test)]
#[path = "events/tests.rs"]
mod tests;

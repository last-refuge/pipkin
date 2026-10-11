//! Mapping a read reference session onto native records, as a pure plan. Nothing here writes;
//! a plan is either complete or refused with every reason found.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pipkin_engine_protocol::{Entry, EntryKind, ImageInput, InputMode, Origin, RunOutcome, Usage};
use serde_json::{Map, Value, json};

use super::ENGINE;
use super::context::{self, SUMMARY_PREFIX, SUMMARY_SUFFIX};
use super::legacy::{DocScope, Legacy, LegacyEntry};
use crate::store::fingerprint;

/// The refusal for unfinished work (the plan never turns queued/placed work into native work).
pub const IN_FLIGHT: &str = "in-flight work; quiesce the session in the legacy engine first";

/// `durable/src/types.ts:39`.
pub const ROOT_CONVERSATION_ID: i64 = 1;

/// `meta.json` of a reference session (`session-catalog.ts:73-76`).
#[derive(Clone, Debug)]
pub struct Meta {
    pub created_at: i64,
    pub cwd: String,
}

#[derive(Clone, Debug)]
pub struct PlannedSession {
    pub id: String,
    pub conversation: i64,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub origin: Origin,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub subagents_enabled: bool,
    pub parent: Option<String>,
    pub root: Option<String>,
    /// Entries in id order, with legacy ids; `retired` already applied.
    pub entries: Vec<Entry>,
    pub retired: Vec<u64>,
    pub kv: Vec<(String, String)>,
    /// (bucket `model|tool`, key, usage JSON as stored by the source).
    pub usage: Vec<(&'static str, String, String)>,
}

#[derive(Clone, Debug)]
pub struct PlannedRequest {
    pub session: String,
    pub key: String,
    pub fingerprint: String,
    pub mode: InputMode,
    pub withdrawn: bool,
    pub outcome: RunOutcome,
    pub text: String,
    pub images: Vec<ImageInput>,
    pub created_at: i64,
    pub settled_at: i64,
}

#[derive(Clone, Debug)]
pub struct PlannedTask {
    pub root: String,
    pub child: u64,
    pub parent_session: String,
    pub parent_call: String,
    pub child_session: String,
    pub task: String,
    pub status: String,
    pub result: Option<String>,
    pub result_delivered: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub sessions: Vec<PlannedSession>,
    pub requests: Vec<PlannedRequest>,
    pub tasks: Vec<PlannedTask>,
    /// Counts and mappings for the manifest.
    pub summary: Value,
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn i64_of(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

/// The child session id of a task-owned conversation.
pub fn child_session_id(source: &str, conversation: i64) -> String {
    format!("{source}:c{conversation}")
}

/// A `pi.tool` task whose assistant call is `subagent` (`subagents-provider.ts:57-100`).
#[derive(Clone, Debug)]
struct SubagentCall {
    task_id: i64,
    call_id: String,
    arguments: Value,
    assistant: i64,
    owner: i64,
}

fn subagent_call(legacy: &Legacy, owner: &Value) -> Result<SubagentCall, String> {
    let owner_conv = i64_of(owner, "conversationId").ok_or("owner has no conversation")?;
    let task_id = i64_of(owner, "taskId").ok_or("owner has no task")?;
    let task = legacy
        .tasks
        .get(&task_id)
        .ok_or_else(|| format!("owner task {task_id} is missing"))?;
    let kind = str_of(task, "kind").unwrap_or("");
    if kind != "pi.tool" {
        return Err(format!(
            "owner task {task_id} is a {kind:?} task, not a subagent tool call"
        ));
    }
    let input = task.get("input").cloned().unwrap_or(Value::Null);
    let (Some(assistant), Some(call_id)) = (i64_of(&input, "assistant"), str_of(&input, "callId"))
    else {
        return Err(format!(
            "owner task {task_id} has no {{assistant, callId}} input"
        ));
    };
    let entry = legacy
        .entry(assistant)
        .filter(|e| e.kind() == "pi.assistant" && e.conversation == owner_conv)
        .ok_or_else(|| format!("owner task {task_id} names a missing assistant entry"))?;
    let call = entry
        .model()
        .and_then(|m| m.first())
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| str_of(b, "type") == Some("toolCall") && str_of(b, "id") == Some(call_id))
        })
        .ok_or_else(|| {
            format!("owner task {task_id}'s call {call_id} is not in its assistant entry")
        })?;
    if str_of(call, "name") != Some("subagent") {
        return Err(format!(
            "owner task {task_id} runs tool {:?}, not subagent",
            str_of(call, "name").unwrap_or("")
        ));
    }
    Ok(SubagentCall {
        task_id,
        call_id: call_id.to_owned(),
        arguments: call.get("arguments").cloned().unwrap_or(Value::Null),
        assistant,
        owner: owner_conv,
    })
}

/// Whether the `subagent` tool is selected by an agent document (`harness/agent.ts:155-239`
/// with the coding agent's default selection: every installed extension, settings carry no
/// `extensions`). A missing document selects everything.
pub fn subagents_enabled(agent: Option<&Value>) -> bool {
    let Some(agent) = agent else { return true };
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let extension = match agent.get("extensions") {
        Some(Value::Array(_)) => names(&agent["extensions"]).iter().any(|n| n == "subagent"),
        Some(Value::Object(edit)) => !edit
            .get("remove")
            .map(names)
            .unwrap_or_default()
            .iter()
            .any(|n| n == "subagent"),
        _ => true,
    };
    let tool = match agent.get("tools") {
        Some(Value::Array(_)) => names(&agent["tools"]).iter().any(|n| n == "subagent"),
        Some(Value::Object(filter)) => !filter
            .get("remove")
            .map(names)
            .unwrap_or_default()
            .iter()
            .any(|n| n == "subagent"),
        _ => true,
    };
    extension && tool
}

/// Text and images of a user message's content (string or blocks).
fn user_input(message: &Value) -> (String, Vec<ImageInput>) {
    match message.get("content") {
        Some(Value::String(text)) => (text.clone(), vec![]),
        Some(Value::Array(blocks)) => {
            let mut text = String::new();
            let mut images = vec![];
            for b in blocks {
                match str_of(b, "type") {
                    Some("text") => text.push_str(str_of(b, "text").unwrap_or("")),
                    Some("image") => images.push(ImageInput {
                        mime_type: str_of(b, "mimeType").unwrap_or("").to_owned(),
                        data: str_of(b, "data").unwrap_or("").to_owned(),
                    }),
                    _ => {}
                }
            }
            (text, images)
        }
        _ => (String::new(), vec![]),
    }
}

/// `subagent.ts:13-17`: concatenated text parts of an answer entry.
fn answer_text(entry: &LegacyEntry) -> String {
    entry
        .model()
        .and_then(|m| m.first())
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| str_of(b, "type") == Some("text"))
                .filter_map(|b| str_of(b, "text"))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn outcome_of(submission: &Value) -> RunOutcome {
    if str_of(submission, "status") == Some("done") {
        return RunOutcome::Completed;
    }
    let reason = str_of(submission, "reason").unwrap_or("unanswered");
    let detail = match submission.get("detail") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    match reason {
        "aborted" => RunOutcome::Aborted,
        "model_error" => RunOutcome::ModelError { detail },
        "no_model" => RunOutcome::NoModel { detail },
        _ => RunOutcome::Failed {
            detail: if detail.is_empty() {
                reason.to_owned()
            } else {
                format!("{reason}: {detail}")
            },
        },
    }
}

fn task_status(task: &Value) -> String {
    let outcome = task
        .get("state")
        .and_then(|s| s.get("outcome"))
        .and_then(|o| str_of(o, "status"));
    match outcome {
        Some("completed") => "completed".into(),
        Some("aborted") => "stopped".into(),
        _ => "failed".into(),
    }
}

fn doc_kv_key(prefix: &str, kind: &str, key: Option<&str>) -> String {
    match key {
        Some(k) => format!("{prefix}{kind}.{k}"),
        None => format!("{prefix}{kind}"),
    }
}

fn json_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

struct ConversationPlan {
    native: String,
    parent: Option<String>,
    call: Option<SubagentCall>,
}

/// Build the native plan, or every reason the session cannot be imported.
pub fn plan(source: &str, meta: &Meta, legacy: &Legacy) -> Result<Plan, Vec<String>> {
    let mut reasons: Vec<String> = Vec::new();
    let mut in_flight: Vec<String> = Vec::new();

    // ------------------------------------------------------------ conversations
    let mut conversations: BTreeMap<i64, ConversationPlan> = BTreeMap::new();
    conversations.insert(
        ROOT_CONVERSATION_ID,
        ConversationPlan {
            native: source.to_owned(),
            parent: None,
            call: None,
        },
    );
    for (&id, record) in &legacy.conversations {
        if record.get("parent").is_some_and(|p| !p.is_null()) {
            reasons.push(format!(
                "conversation {id} is a fork; forked history is not imported"
            ));
            continue;
        }
        if id == ROOT_CONVERSATION_ID {
            continue;
        }
        match record.get("owner").filter(|o| !o.is_null()) {
            None => reasons.push(format!(
                "conversation {id} is an ownerless extra conversation; its role is not mapped"
            )),
            Some(owner) => match subagent_call(legacy, owner) {
                Ok(call) => {
                    conversations.insert(
                        id,
                        ConversationPlan {
                            native: child_session_id(source, id),
                            parent: None,
                            call: Some(call),
                        },
                    );
                }
                Err(why) => reasons.push(format!("conversation {id}: {why}")),
            },
        }
    }
    // Parents resolve once every conversation is known (owners precede children by id).
    let natives: HashMap<i64, String> = conversations
        .iter()
        .map(|(id, c)| (*id, c.native.clone()))
        .collect();
    for (id, conversation) in conversations.iter_mut() {
        if let Some(call) = &conversation.call {
            match natives.get(&call.owner) {
                Some(parent) => conversation.parent = Some(parent.clone()),
                None => reasons.push(format!(
                    "conversation {id} is owned by unmapped conversation {}",
                    call.owner
                )),
            }
        }
    }

    // ------------------------------------------------------------ in-flight work
    for (id, task) in &legacy.tasks {
        let status = task
            .get("state")
            .and_then(|s| str_of(s, "status"))
            .unwrap_or("unknown");
        if status != "terminal" {
            in_flight.push(format!(
                "task {id} ({}) is {status}",
                str_of(task, "kind").unwrap_or("?")
            ));
        }
    }
    for (id, submission) in &legacy.submissions {
        if let Some(status @ ("queued" | "placed")) = str_of(submission, "status") {
            in_flight.push(format!("submission {id} is {status}"));
        }
    }
    for doc in &legacy.documents {
        let DocScope::Conversation(conv) = doc.scope else {
            continue;
        };
        match doc.kind.as_str() {
            "pi.inbox" => {
                if doc
                    .value
                    .get("items")
                    .and_then(Value::as_array)
                    .is_some_and(|i| !i.is_empty())
                {
                    in_flight.push(format!("conversation {conv} has queued inbox items"));
                }
            }
            "pi.live" => {
                let v = &doc.value;
                if v.get("run").is_some_and(|r| !r.is_null()) {
                    in_flight.push(format!("conversation {conv} has a live run"));
                }
                if v.get("generation").is_some_and(|r| !r.is_null()) {
                    in_flight.push(format!("conversation {conv} has a live generation"));
                }
                if v.get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|t| t.iter().any(|slot| str_of(slot, "status") != Some("done")))
                {
                    in_flight.push(format!("conversation {conv} has live tool slots"));
                }
                if v.get("compactions")
                    .and_then(Value::as_array)
                    .is_some_and(|c| !c.is_empty())
                {
                    in_flight.push(format!("conversation {conv} has a live compaction"));
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ entries
    let mut by_conversation: BTreeMap<i64, Vec<&LegacyEntry>> = BTreeMap::new();
    for entry in &legacy.entries {
        if !conversations.contains_key(&entry.conversation) {
            if !legacy.conversations.contains_key(&entry.conversation) {
                reasons.push(format!(
                    "entry {} belongs to unknown conversation {}",
                    entry.id, entry.conversation
                ));
            }
            continue;
        }
        by_conversation
            .entry(entry.conversation)
            .or_default()
            .push(entry);
    }

    // Input submissions by their user entry.
    let mut request_of_entry: HashMap<i64, String> = HashMap::new();
    for (_, submission) in &legacy.submissions {
        if str_of(submission, "type") == Some("input")
            && let (Some(entry), Some(key)) =
                (i64_of(submission, "entry"), str_of(submission, "requestId"))
            && !key.is_empty()
        {
            request_of_entry.insert(entry, key.to_owned());
        }
    }

    let mut kind_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut normalized_entries: Vec<i64> = Vec::new();
    let mut raw_entries: Vec<i64> = Vec::new();
    let mut retired_total = 0u64;
    let mut sessions: Vec<PlannedSession> = Vec::new();

    for (&conv, conversation) in &conversations {
        let entries = by_conversation.get(&conv).cloned().unwrap_or_default();
        let ids: BTreeSet<i64> = entries.iter().map(|e| e.id).collect();
        let mut at = meta.created_at;
        let mut planned: Vec<Entry> = Vec::with_capacity(entries.len());
        let mut retired: BTreeSet<u64> = BTreeSet::new();
        let mut kv: Vec<(String, String)> = Vec::new();

        for e in &entries {
            *kind_counts.entry(e.kind().to_owned()).or_default() += 1;
            let model: &[Value] = e.model().map(Vec::as_slice).unwrap_or(&[]);
            let edits: &[Value] = e.edits().map(Vec::as_slice).unwrap_or(&[]);
            let mut omit_targets: Vec<u64> = Vec::new();
            for edit in edits {
                match (str_of(edit, "action"), i64_of(edit, "target")) {
                    (Some("omit"), Some(target)) if ids.contains(&target) && target < e.id => {
                        omit_targets.push(target as u64)
                    }
                    (Some("omit"), _) => reasons.push(format!(
                        "entry {} omits an entry that is not earlier in its conversation",
                        e.id
                    )),
                    (Some("replace"), _) => reasons.push(format!(
                        "entry {} carries a context edit with action replace; replaced context is not imported",
                        e.id
                    )),
                    (other, _) => reasons.push(format!(
                        "entry {} carries an unknown context edit {:?}",
                        e.id,
                        other.unwrap_or("")
                    )),
                }
            }
            let kind = e.kind();
            let mut keep_raw = false;
            let mut envelope: Option<Value> = None;
            let entry_kind: Option<EntryKind> = match kind {
                "pi.user" | "pi.assistant" | "pi.tool-result" => {
                    let role = match kind {
                        "pi.user" => "user",
                        "pi.assistant" => "assistant",
                        _ => "toolResult",
                    };
                    if e.head().is_some() || !edits.is_empty() {
                        reasons.push(format!(
                            "entry {} ({kind}) carries a head or context edits",
                            e.id
                        ));
                        None
                    } else if model.len() != 1 || str_of(&model[0], "role") != Some(role) {
                        reasons.push(format!(
                            "entry {} ({kind}) does not hold exactly one {role} message",
                            e.id
                        ));
                        None
                    } else {
                        match context::normalize_message(&model[0]) {
                            Ok((message, normalized)) => {
                                if message.timestamp() > 0 {
                                    at = message.timestamp();
                                }
                                if normalized {
                                    keep_raw = true;
                                    normalized_entries.push(e.id);
                                } else if let Value::Object(map) = &e.record {
                                    let mut rest = map.clone();
                                    rest.remove("model");
                                    for k in ["id", "conversationId", "kind"] {
                                        rest.remove(k);
                                    }
                                    if !rest.is_empty() {
                                        let mut env = map.clone();
                                        env.remove("model");
                                        envelope = Some(Value::Object(env));
                                    }
                                }
                                Some(EntryKind::Message {
                                    message,
                                    mode: InputMode::Prompt,
                                    request: if kind == "pi.user" {
                                        request_of_entry.get(&e.id).cloned()
                                    } else {
                                        None
                                    },
                                    partial: false,
                                })
                            }
                            Err(why) => {
                                reasons.push(format!("entry {} ({kind}): message {why}", e.id));
                                None
                            }
                        }
                    }
                }
                "pi.compaction" => {
                    keep_raw = true;
                    let head = e.head().and_then(Value::as_i64);
                    let text = (model.len() == 1 && str_of(&model[0], "role") == Some("user"))
                        .then(|| match model[0].get("content") {
                            Some(Value::String(s)) => Some(s.clone()),
                            Some(Value::Array(b))
                                if b.len() == 1 && str_of(&b[0], "type") == Some("text") =>
                            {
                                str_of(&b[0], "text").map(str::to_owned)
                            }
                            _ => None,
                        })
                        .flatten();
                    let summary = text.as_deref().and_then(|t| {
                        t.strip_prefix(SUMMARY_PREFIX)
                            .and_then(|t| t.strip_suffix(SUMMARY_SUFFIX))
                    });
                    match (head, summary) {
                        (Some(head), Some(summary))
                            if head <= e.id && head >= 0 && edits.is_empty() =>
                        {
                            if let Some(ts) = i64_of(&model[0], "timestamp").filter(|t| *t > 0) {
                                at = ts;
                            }
                            Some(EntryKind::Compaction {
                                summary: summary.to_owned(),
                                first_kept: head as u64,
                                tokens_before: 0,
                            })
                        }
                        _ => {
                            reasons.push(format!(
                                "entry {} (pi.compaction) does not have the reference summary shape",
                                e.id
                            ));
                            None
                        }
                    }
                }
                "pi.reset" => {
                    keep_raw = true;
                    if e.head().and_then(Value::as_i64) != Some(e.id) || !edits.is_empty() {
                        reasons.push(format!(
                            "entry {} (pi.reset) is not a self-headed reset",
                            e.id
                        ));
                        None
                    } else if model.is_empty() {
                        Some(EntryKind::Reset { handoff: None })
                    } else if model.len() == 1 && str_of(&model[0], "role") == Some("user") {
                        match context::normalize_message(&model[0]) {
                            Ok((message, _)) => {
                                if message.timestamp() > 0 {
                                    at = message.timestamp();
                                }
                                Some(EntryKind::Reset {
                                    handoff: Some(message),
                                })
                            }
                            Err(why) => {
                                reasons.push(format!("entry {} (pi.reset): handoff {why}", e.id));
                                None
                            }
                        }
                    } else {
                        reasons.push(format!(
                            "entry {} (pi.reset) has an unexpected handoff",
                            e.id
                        ));
                        None
                    }
                }
                "pi.cancelled-input" | "pi.system" => {
                    if e.head().is_some() {
                        reasons.push(format!("entry {} ({kind}) carries a head", e.id));
                        None
                    } else if kind == "pi.cancelled-input" && !model.is_empty() {
                        reasons.push(format!(
                            "entry {} (pi.cancelled-input) carries model messages",
                            e.id
                        ));
                        None
                    } else if kind == "pi.system"
                        && model.iter().any(|m| str_of(m, "role") != Some("system"))
                    {
                        reasons.push(format!(
                            "entry {} (pi.system) carries non-system messages",
                            e.id
                        ));
                        None
                    } else {
                        Some(EntryKind::Custom {
                            custom_type: kind.to_owned(),
                            data: e.record.clone(),
                        })
                    }
                }
                other => {
                    keep_raw = true;
                    if !model.is_empty() || !edits.is_empty() || e.head().is_some() {
                        reasons.push(format!(
                            "entry {} has unknown kind {other:?} with model messages, context edits or a head; its context semantics are not mapped",
                            e.id
                        ));
                        None
                    } else {
                        Some(EntryKind::Custom {
                            custom_type: other.to_owned(),
                            data: e.record.clone(),
                        })
                    }
                }
            };
            if let Some(kind) = entry_kind {
                retired.extend(omit_targets);
                planned.push(Entry {
                    id: e.id as u64,
                    at_ms: at,
                    run: None,
                    retired: false,
                    kind,
                });
                if keep_raw {
                    raw_entries.push(e.id);
                    kv.push((format!("pi.entry.{}", e.id), e.raw.clone()));
                } else if let Some(envelope) = envelope {
                    kv.push((format!("pi.entry.{}", e.id), json_text(&envelope)));
                }
            }
        }
        for entry in planned.iter_mut() {
            if retired.contains(&entry.id) {
                entry.retired = true;
            }
        }
        retired_total += retired.len() as u64;

        // The effective context must survive the mapping exactly.
        match context::reference_messages(&entries) {
            Ok(reference) => {
                let native = context::native_messages(&context::native_range(&planned));
                if reference != native && planned.len() == entries.len() {
                    reasons.push(format!(
                        "the imported model context of conversation {conv} would differ from the source"
                    ));
                }
            }
            Err(why) => reasons.push(format!("conversation {conv}: {why}")),
        }

        // Documents of this conversation.
        let agent = legacy.doc("pi.agent", conv).map(|d| &d.value);
        let model = agent.and_then(|a| a.get("model")).and_then(|m| {
            Some(format!(
                "{}/{}",
                str_of(m, "provider")?,
                str_of(m, "modelId")?
            ))
        });
        let thinking = agent
            .and_then(|a| str_of(a, "thinkingLevel"))
            .map(str::to_owned);
        if let Some(provider) = legacy.doc("pi.provider", conv)
            && let Some(id) = str_of(&provider.value, "sessionId")
        {
            kv.push(("provider_session_id".into(), id.to_owned()));
        }
        let mut usage = Vec::new();
        if let Some(doc) = legacy.doc("pi.usage", conv) {
            for (bucket, native_bucket) in [("models", "model"), ("tools", "tool")] {
                for (key, value) in doc
                    .value
                    .get(bucket)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    if serde_json::from_value::<Usage>(value.clone()).is_err() {
                        reasons.push(format!(
                            "conversation {conv}: usage {bucket}/{key} does not match the native usage model"
                        ));
                    }
                    usage.push((native_bucket, key.clone(), json_text(value)));
                }
            }
        }
        for doc in &legacy.documents {
            if doc.scope == DocScope::Conversation(conv) {
                kv.push((
                    doc_kv_key("pi.doc.", &doc.kind, doc.key.as_deref()),
                    json_text(
                        &json!({"version": doc.version, "value": doc.value, "record": doc.record}),
                    ),
                ));
            }
        }
        if let Some(record) = legacy.conversations.get(&conv) {
            kv.push(("pi.conversation".into(), json_text(record)));
        }
        let submissions: Vec<&Value> = legacy
            .submissions
            .iter()
            .filter(|(_, s)| i64_of(s, "conversationId") == Some(conv))
            .map(|(_, s)| s)
            .collect();
        if !submissions.is_empty() {
            kv.push(("pi.submissions".into(), json_text(&json!(submissions))));
        }
        let subagent_tasks: BTreeSet<i64> = conversations
            .values()
            .filter_map(|c| c.call.as_ref().map(|c| c.task_id))
            .collect();
        let tasks: Vec<&Value> = legacy
            .tasks
            .iter()
            .filter(|(id, t)| {
                i64_of(t, "conversationId") == Some(conv) && !subagent_tasks.contains(id)
            })
            .map(|(_, t)| t)
            .collect();
        if !tasks.is_empty() {
            kv.push(("pi.tasks".into(), json_text(&json!(tasks))));
        }
        if let Some(call) = &conversation.call
            && let Some(task) = legacy.tasks.get(&call.task_id)
        {
            kv.push(("pi.owner-task".into(), json_text(task)));
        }
        if let Some(max) = planned.last().map(|e| e.id) {
            kv.push(("entry_floor".into(), (max + 1).to_string()));
        }

        let is_root = conv == ROOT_CONVERSATION_ID;
        let cwd = if is_root {
            meta.cwd.clone()
        } else {
            agent
                .and_then(|a| str_of(a, "cwd"))
                .map(str::to_owned)
                .unwrap_or_else(|| meta.cwd.clone())
        };
        let created_at = match &conversation.call {
            Some(call) => legacy
                .entry(call.assistant)
                .and_then(|e| e.model()?.first().and_then(|m| i64_of(m, "timestamp")))
                .filter(|t| *t > 0)
                .unwrap_or(meta.created_at),
            None => meta.created_at,
        };
        let updated_at = planned
            .iter()
            .map(|e| e.at_ms)
            .max()
            .unwrap_or(created_at)
            .max(created_at);
        sessions.push(PlannedSession {
            id: conversation.native.clone(),
            conversation: conv,
            cwd,
            created_at,
            updated_at,
            origin: Origin::Imported {
                engine: ENGINE.into(),
                source: conversation.native.clone(),
            },
            model,
            thinking,
            subagents_enabled: subagents_enabled(agent),
            parent: conversation.parent.clone(),
            root: (!is_root).then(|| source.to_owned()),
            retired: retired.into_iter().collect(),
            entries: planned,
            kv,
            usage,
        });
    }

    // Documents outside mapped conversations.
    for doc in &legacy.documents {
        match doc.scope {
            DocScope::Conversation(conv) if !conversations.contains_key(&conv) => {
                if !legacy.conversations.contains_key(&conv) {
                    reasons.push(format!(
                        "document {} ({}) belongs to unknown conversation {conv}",
                        doc.id, doc.kind
                    ));
                }
            }
            DocScope::Session | DocScope::Task(_) => {
                let prefix = match doc.scope {
                    DocScope::Task(t) => format!("pi.task-doc.{t}."),
                    _ => "pi.session-doc.".into(),
                };
                if let Some(root) = sessions.first_mut() {
                    root.kv.push((
                        doc_kv_key(&prefix, &doc.kind, doc.key.as_deref()),
                        json_text(&json!({"version": doc.version, "value": doc.value, "record": doc.record})),
                    ));
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ submissions -> requests
    let mut requests = Vec::new();
    let mut unkeyed = 0u64;
    let mut writes = 0u64;
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut submission_counts: BTreeMap<String, u64> = BTreeMap::new();
    for (id, submission) in &legacy.submissions {
        let status = str_of(submission, "status").unwrap_or("?");
        *submission_counts
            .entry(format!(
                "{}.{status}",
                str_of(submission, "type").unwrap_or("?")
            ))
            .or_default() += 1;
        if str_of(submission, "type") != Some("input") {
            writes += 1;
            continue;
        }
        if matches!(status, "queued" | "placed") {
            continue;
        }
        let Some(key) = str_of(submission, "requestId").filter(|k| !k.is_empty()) else {
            unkeyed += 1;
            continue;
        };
        let conv = i64_of(submission, "conversationId").unwrap_or(-1);
        let Some(session) = conversations.get(&conv).map(|c| c.native.clone()) else {
            if !legacy.conversations.contains_key(&conv) {
                reasons.push(format!(
                    "submission {id} belongs to unknown conversation {conv}"
                ));
            }
            continue;
        };
        if !seen.insert((session.clone(), key.to_owned())) {
            reasons.push(format!(
                "request id {key:?} identifies more than one submission"
            ));
            continue;
        }
        let user = i64_of(submission, "entry").and_then(|e| legacy.entry(e));
        let user_message = user.and_then(|e| e.model()).and_then(|m| m.first());
        let (text, images) = user_message.map(user_input).unwrap_or_default();
        let created_at = user_message
            .and_then(|m| i64_of(m, "timestamp"))
            .unwrap_or(meta.created_at);
        let settled_at = i64_of(submission, "answer")
            .and_then(|a| legacy.entry(a))
            .and_then(|e| e.model()?.first().and_then(|m| i64_of(m, "timestamp")))
            .unwrap_or(created_at);
        let mode = InputMode::Prompt;
        requests.push(PlannedRequest {
            session,
            key: key.to_owned(),
            fingerprint: fingerprint(mode, &text, &images),
            mode,
            withdrawn: user.is_none(),
            outcome: outcome_of(submission),
            text,
            images,
            created_at,
            settled_at,
        });
    }

    // ------------------------------------------------------------ subagent tasks
    let mut tasks = Vec::new();
    let mut child_number = 0u64;
    for (&conv, conversation) in &conversations {
        let Some(call) = &conversation.call else {
            continue;
        };
        let Some(parent) = &conversation.parent else {
            continue;
        };
        child_number += 1;
        let task = legacy
            .tasks
            .get(&call.task_id)
            .cloned()
            .unwrap_or(Value::Null);
        let request_key = format!("subagent:{}", call.task_id);
        let result = legacy
            .submissions
            .iter()
            .find(|(_, s)| {
                i64_of(s, "conversationId") == Some(conv)
                    && str_of(s, "requestId") == Some(request_key.as_str())
                    && str_of(s, "status") == Some("done")
            })
            .and_then(|(_, s)| i64_of(s, "answer"))
            .and_then(|a| legacy.entry(a))
            .map(answer_text);
        let delivered = legacy.entries.iter().any(|e| {
            e.conversation == call.owner
                && e.kind() == "pi.tool-result"
                && e.model()
                    .and_then(|m| m.first())
                    .and_then(|m| str_of(m, "toolCallId"))
                    == Some(call.call_id.as_str())
        });
        let child = sessions.iter().find(|s| s.conversation == conv);
        tasks.push(PlannedTask {
            root: source.to_owned(),
            child: child_number,
            parent_session: parent.clone(),
            parent_call: call.call_id.clone(),
            child_session: conversation.native.clone(),
            task: str_of(&call.arguments, "task")
                .unwrap_or("Subagent")
                .to_owned(),
            status: task_status(&task),
            result,
            result_delivered: delivered,
            created_at: child.map_or(meta.created_at, |c| c.created_at),
            updated_at: child.map_or(meta.created_at, |c| c.updated_at),
        });
    }

    if !in_flight.is_empty() {
        reasons.insert(0, format!("{IN_FLIGHT} ({})", in_flight.join("; ")));
    }
    if !reasons.is_empty() {
        let mut unique = Vec::new();
        for r in reasons {
            if !unique.contains(&r) {
                unique.push(r);
            }
        }
        return Err(unique);
    }

    let mut ids = Map::new();
    for (conv, c) in &conversations {
        ids.insert(conv.to_string(), Value::String(c.native.clone()));
    }
    let summary = json!({
        "conversations": ids,
        "counts": {
            "entries": kind_counts,
            "entriesTotal": legacy.entries.len(),
            "retired": retired_total,
            "submissions": submission_counts,
            "requests": requests.len(),
            "inputSubmissionsWithoutKey": unkeyed,
            "writeSubmissions": writes,
            "tasks": legacy.tasks.len(),
            "subagentTasks": tasks.len(),
            "documents": legacy.documents.len(),
            "retiredDocuments": legacy.retired_documents,
            "conversations": conversations.len(),
        },
        "entryIds": "kept; native ids continue above kv entry_floor",
        "normalizedEntries": normalized_entries,
        "rawEntryRecords": raw_entries,
        "notes": [
            "messages whose native JSON differs from the source (normalizedEntries, e.g. user content given as a plain string) keep their source record byte for byte in kv pi.entry.<id>",
            "compaction, reset and unknown-kind entries (rawEntryRecords) keep their source record byte for byte in kv pi.entry.<id>; other message entries keep any non-model fields (byTaskId, data) in kv pi.entry.<id>",
            "request modes are not recorded by the source; imported requests are recorded as prompt",
            "compaction tokens_before is not recorded by the source; imported as 0",
            "documents are imported at their latest value; rewindable pi.agent history is not imported",
        ],
    });
    Ok(Plan {
        sessions,
        requests,
        tasks,
        summary,
    })
}

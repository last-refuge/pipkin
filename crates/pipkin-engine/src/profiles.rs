//! Subagent profiles and launch records (plan §6, SUB-1). Pure: no storage, no network.
//!
//! A profile names a role and fixes what a child may do: its instructions, an optional
//! model/thinking override, the tools it is offered and whether it starts fresh or with a
//! bounded excerpt of the parent's conversation. Built-in profiles are `worker` (every tool a
//! child can have, fresh context), `reviewer` (reads only: `read`, `plan_read`, `web_read`)
//! and `scout` (reads plus the web; it cannot change the workspace but it does reach the
//! network, so it is not called read-only). Settings may add or override profiles under
//! `subagents.profiles`.
//!
//! At launch the resolved policy is frozen into a [`LaunchRecord`] stored with the child's
//! task row. Enforcement reads the record, never the current settings, so editing a profile
//! cannot change a child that is already running or recovering. Children never receive the
//! `subagent` or `ask_user_question` tools, and a context excerpt is text only: it carries no
//! goal, no permissions and no scheduler.

use pipkin_engine_protocol::{Entry, EntryKind, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Settings;

/// Bumped when the record's meaning changes.
pub const PROFILE_VERSION: u32 = 1;
pub const DEFAULT_PROFILE: &str = "worker";
/// Tools a reviewer may use: reading the workspace and retained material only.
pub const REVIEWER_TOOLS: &[&str] = &["read", "plan_read", "web_read"];
/// Tools a scout may use: reads plus the web.
pub const SCOUT_TOOLS: &[&str] = &["read", "plan_read", "web_search", "web_fetch", "web_read"];
/// Tools that never reach a child regardless of profile.
pub const NEVER_FOR_CHILDREN: &[&str] = &["subagent", "ask_user_question"];
/// Bounds of an inherited context excerpt.
pub const CONTEXT_MAX_MESSAGES: usize = 20;
pub const CONTEXT_MAX_CHARS: usize = 16 * 1024;
/// Default caps (settings `subagents.maxConcurrent` / `subagents.maxTotal`).
pub const DEFAULT_MAX_CONCURRENT: usize = 4;
pub const DEFAULT_MAX_TOTAL: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// The task text only.
    Fresh,
    /// The task plus a bounded excerpt of the parent's recent conversation.
    Inherit,
}

impl ContextMode {
    pub fn parse(s: &str) -> Option<ContextMode> {
        match s {
            "fresh" => Some(ContextMode::Fresh),
            "inherit" => Some(ContextMode::Inherit),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ContextMode::Fresh => "fresh",
            ContextMode::Inherit => "inherit",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolPolicy {
    /// Every tool a child can have.
    All,
    /// Exactly these tools (already validated against the offerable set).
    Only(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub instructions: Option<String>,
    /// `provider/modelId`; `None` inherits the parent's model.
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub tools: ToolPolicy,
    pub context: ContextMode,
    pub builtin: bool,
}

impl Profile {
    /// Whether the profile can change the workspace or reach the network, for honest labels.
    pub fn access_label(&self) -> &'static str {
        match &self.tools {
            ToolPolicy::All => "full access",
            ToolPolicy::Only(tools) => {
                let mutates = tools
                    .iter()
                    .any(|t| matches!(t.as_str(), "write" | "edit" | "bash" | "plan_update"));
                let network = tools
                    .iter()
                    .any(|t| matches!(t.as_str(), "web_search" | "web_fetch" | "bash"));
                match (mutates, network) {
                    (false, false) => "read-only",
                    (false, true) => "workspace read-only; web access",
                    (true, _) => "can change the workspace",
                }
            }
        }
    }

    pub fn allows(&self, tool: &str) -> bool {
        if NEVER_FOR_CHILDREN.contains(&tool) {
            return false;
        }
        match &self.tools {
            ToolPolicy::All => true,
            ToolPolicy::Only(tools) => tools.iter().any(|t| t == tool),
        }
    }
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

pub fn builtin_profiles() -> Vec<Profile> {
    vec![
        Profile {
            name: "worker".into(),
            description: "Does the task with every tool a child can have; starts without the parent's conversation.".into(),
            instructions: None,
            model: None,
            thinking: None,
            tools: ToolPolicy::All,
            context: ContextMode::Fresh,
            builtin: true,
        },
        Profile {
            name: "reviewer".into(),
            description: "Reviews code or results and reports findings; can read files and retained material but cannot change anything or reach the network.".into(),
            instructions: Some("You are reviewing, not implementing. Read what you need and report precise findings with file paths and line references. You cannot modify files, run commands or fetch pages; if the task needs that, say so instead of pretending.".into()),
            model: None,
            thinking: None,
            tools: ToolPolicy::Only(strings(REVIEWER_TOOLS)),
            context: ContextMode::Fresh,
            builtin: true,
        },
        Profile {
            name: "scout".into(),
            description: "Researches a question using the workspace and the web and reports back with cited sources; cannot change the workspace.".into(),
            instructions: Some("You are researching, not implementing. Use the web tools and cite sources by id. You cannot modify files or run commands.".into()),
            model: None,
            thinking: None,
            tools: ToolPolicy::Only(strings(SCOUT_TOOLS)),
            context: ContextMode::Fresh,
            builtin: true,
        },
    ]
}

/// Caps from settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub max_concurrent: usize,
    pub max_total: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            max_total: DEFAULT_MAX_TOTAL,
        }
    }
}

pub fn caps_from_settings(settings: &Settings) -> Caps {
    let section = settings.other.get("subagents");
    let read = |key: &str, default: usize| {
        section
            .and_then(|s| s.get(key))
            .and_then(Value::as_u64)
            .map(|n| (n as usize).max(1))
            .unwrap_or(default)
    };
    Caps {
        max_concurrent: read("maxConcurrent", DEFAULT_MAX_CONCURRENT),
        max_total: read("maxTotal", DEFAULT_MAX_TOTAL),
    }
}

/// Built-in profiles with settings-defined ones merged over them (same name replaces).
/// `offerable` is the set of tool names a child could be given; a profile naming anything
/// else is rejected with a reason rather than silently narrowed.
pub fn profiles_from_settings(
    settings: &Settings,
    offerable: &[String],
) -> Result<Vec<Profile>, String> {
    let mut profiles = builtin_profiles();
    let Some(defined) = settings
        .other
        .get("subagents")
        .and_then(|s| s.get("profiles"))
        .and_then(Value::as_object)
    else {
        return Ok(profiles);
    };
    for (name, body) in defined {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "subagents.profiles: {name:?} is not a valid profile name (letters, digits, - and _, at most 64)"
            ));
        }
        let Some(body) = body.as_object() else {
            return Err(format!("subagents.profiles.{name} must be an object"));
        };
        let text = |key: &str| -> Result<Option<String>, String> {
            match body.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => {
                    let s = s.trim();
                    Ok((!s.is_empty()).then(|| s.to_owned()))
                }
                Some(_) => Err(format!("subagents.profiles.{name}.{key} must be a string")),
            }
        };
        let tools = match body.get("tools") {
            None | Some(Value::Null) => ToolPolicy::All,
            Some(Value::String(s)) if s == "all" => ToolPolicy::All,
            Some(Value::Array(items)) => {
                let mut list = Vec::new();
                for item in items {
                    let Some(tool) = item.as_str().map(str::trim).filter(|t| !t.is_empty()) else {
                        return Err(format!(
                            "subagents.profiles.{name}.tools must list tool names"
                        ));
                    };
                    if NEVER_FOR_CHILDREN.contains(&tool) {
                        return Err(format!(
                            "subagents.profiles.{name}.tools: {tool} is never available to a subagent"
                        ));
                    }
                    if !offerable.iter().any(|o| o == tool) {
                        return Err(format!(
                            "subagents.profiles.{name}.tools: unknown tool {tool} (available: {})",
                            offerable.join(", ")
                        ));
                    }
                    if !list.contains(&tool.to_owned()) {
                        list.push(tool.to_owned());
                    }
                }
                ToolPolicy::Only(list)
            }
            Some(_) => {
                return Err(format!(
                    "subagents.profiles.{name}.tools must be \"all\" or a list of tool names"
                ));
            }
        };
        let context = match text("context")? {
            None => ContextMode::Fresh,
            Some(mode) => ContextMode::parse(&mode).ok_or_else(|| {
                format!("subagents.profiles.{name}.context must be \"fresh\" or \"inherit\"")
            })?,
        };
        let profile = Profile {
            name: name.to_owned(),
            description: text("description")?.unwrap_or_default(),
            instructions: text("instructions")?,
            model: text("model")?,
            thinking: text("thinking")?,
            tools,
            context,
            builtin: false,
        };
        if let Some(at) = profiles.iter().position(|p| p.name == profile.name) {
            profiles[at] = profile;
        } else {
            profiles.push(profile);
        }
    }
    Ok(profiles)
}

/// The policy frozen at launch and stored with the child's task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecord {
    pub version: u32,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// `None` means every tool a child can have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    pub context: ContextMode,
    /// The parent's session revision when the excerpt was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_revision: Option<u64>,
    /// Messages included in the excerpt and messages left out for the bound.
    #[serde(default)]
    pub context_messages: usize,
    #[serde(default)]
    pub context_omitted: usize,
    pub created_at_ms: i64,
}

impl LaunchRecord {
    pub fn allows(&self, tool: &str) -> bool {
        if NEVER_FOR_CHILDREN.contains(&tool) {
            return false;
        }
        match &self.tools {
            None => true,
            Some(tools) => tools.iter().any(|t| t == tool),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("serializes")
    }

    pub fn from_json(s: &str) -> Option<LaunchRecord> {
        serde_json::from_str(s).ok()
    }
}

/// A bounded excerpt of the parent's recent user and assistant messages (oldest first), and
/// how many were left out. Thinking, tool rounds and system entries are never included.
pub fn context_excerpt(entries: &[Entry]) -> (Vec<(String, String)>, usize) {
    let messages: Vec<(String, String)> = entries
        .iter()
        .filter(|e| !e.retired)
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message: m @ Message::User { .. },
                partial: false,
                ..
            } => Some(("User".to_owned(), m.text())),
            EntryKind::Message {
                message: m @ Message::Assistant { .. },
                partial: false,
                ..
            } => Some(("Assistant".to_owned(), m.text())),
            _ => None,
        })
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    let total = messages.len();
    let mut kept: Vec<(String, String)> = Vec::new();
    let mut chars = 0;
    for (who, text) in messages.into_iter().rev() {
        if kept.len() >= CONTEXT_MAX_MESSAGES {
            break;
        }
        let len = text.chars().count();
        if chars + len > CONTEXT_MAX_CHARS {
            if kept.is_empty() {
                // Even one message is too long: keep its tail so the child sees something.
                let tail: String = text
                    .chars()
                    .rev()
                    .take(CONTEXT_MAX_CHARS)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                kept.push((who, format!("…{tail}")));
            }
            break;
        }
        chars += len;
        kept.push((who, text));
    }
    kept.reverse();
    let omitted = total - kept.len();
    (kept, omitted)
}

/// The child's prompt: the task, then (for `inherit`) the excerpt as clearly labelled text.
pub fn compose_prompt(
    task: &str,
    excerpt: &[(String, String)],
    omitted: usize,
    parent_revision: u64,
) -> String {
    if excerpt.is_empty() {
        return task.to_owned();
    }
    let mut out = String::from(task);
    out.push_str(&format!(
        "\n\n<parent_context revision=\"{parent_revision}\" messages=\"{}\" omitted=\"{omitted}\">\nRecent messages from the conversation that delegated this task, oldest first. They are background only: do the task above.\n",
        excerpt.len()
    ));
    for (who, text) in excerpt {
        out.push_str(&format!("\n{who}: {text}\n"));
    }
    out.push_str("</parent_context>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings_with(subagents: Value) -> Settings {
        let mut s = Settings::load(std::path::Path::new("/nonexistent-config-dir"), None).0;
        s.other.insert("subagents".into(), subagents);
        s
    }

    fn offerable() -> Vec<String> {
        strings(&[
            "read",
            "write",
            "edit",
            "bash",
            "plan_update",
            "plan_read",
            "web_search",
            "web_fetch",
            "web_read",
        ])
    }

    #[test]
    fn builtins_are_labelled_by_what_they_can_actually_do() {
        let profiles = builtin_profiles();
        let by = |n: &str| profiles.iter().find(|p| p.name == n).unwrap();
        assert_eq!(by("worker").access_label(), "full access");
        assert_eq!(by("reviewer").access_label(), "read-only");
        assert_eq!(
            by("scout").access_label(),
            "workspace read-only; web access"
        );
        assert!(by("reviewer").allows("read") && !by("reviewer").allows("bash"));
        assert!(!by("reviewer").allows("web_fetch") && by("scout").allows("web_fetch"));
        assert!(!by("worker").allows("subagent") && !by("worker").allows("ask_user_question"));
    }

    #[test]
    fn settings_profiles_merge_over_builtins_and_are_validated() {
        let s = settings_with(json!({"profiles": {
            "reviewer": {"tools": ["read", "bash"], "instructions": "look"},
            "tester": {"tools": ["read", "bash"], "model": "scripted/scripted", "context": "inherit", "thinking": "high"}
        }, "maxConcurrent": 2, "maxTotal": 0}));
        let profiles = profiles_from_settings(&s, &offerable()).unwrap();
        let reviewer = profiles.iter().find(|p| p.name == "reviewer").unwrap();
        assert_eq!(reviewer.tools, ToolPolicy::Only(strings(&["read", "bash"])));
        assert_eq!(
            reviewer.access_label(),
            "can change the workspace",
            "bash is not read-only"
        );
        assert!(!reviewer.builtin);
        let tester = profiles.iter().find(|p| p.name == "tester").unwrap();
        assert_eq!(tester.context, ContextMode::Inherit);
        assert_eq!(tester.model.as_deref(), Some("scripted/scripted"));
        assert_eq!(
            caps_from_settings(&s),
            Caps {
                max_concurrent: 2,
                max_total: 1
            }
        );
        for (bad, needle) in [
            (
                json!({"profiles": {"x": {"tools": ["subagent"]}}}),
                "never available",
            ),
            (
                json!({"profiles": {"x": {"tools": ["grep"]}}}),
                "unknown tool grep",
            ),
            (
                json!({"profiles": {"x": {"tools": "some"}}}),
                "must be \"all\" or a list",
            ),
            (
                json!({"profiles": {"x": {"context": "shared"}}}),
                "fresh\" or \"inherit",
            ),
            (
                json!({"profiles": {"bad name": {}}}),
                "not a valid profile name",
            ),
            (json!({"profiles": {"x": 3}}), "must be an object"),
            (json!({"profiles": {"x": {"model": 3}}}), "must be a string"),
        ] {
            let err = profiles_from_settings(&settings_with(bad), &offerable()).unwrap_err();
            assert!(err.contains(needle), "{err}");
        }
        assert_eq!(
            caps_from_settings(&settings_with(json!({}))),
            Caps::default()
        );
    }

    #[test]
    fn launch_records_round_trip_and_enforce_their_own_policy() {
        let record = LaunchRecord {
            version: PROFILE_VERSION,
            profile: "reviewer".into(),
            instructions: Some("look".into()),
            tools: Some(strings(REVIEWER_TOOLS)),
            model: Some("scripted/scripted".into()),
            thinking: None,
            context: ContextMode::Fresh,
            parent_revision: None,
            context_messages: 0,
            context_omitted: 0,
            created_at_ms: 1,
        };
        let back = LaunchRecord::from_json(&record.to_json()).unwrap();
        assert_eq!(back, record);
        assert!(back.allows("read") && !back.allows("write") && !back.allows("subagent"));
        let all = LaunchRecord {
            tools: None,
            ..record
        };
        assert!(all.allows("bash") && !all.allows("ask_user_question"));
        assert!(LaunchRecord::from_json("{").is_none());
    }

    fn entry(id: u64, message: Message, retired: bool) -> Entry {
        Entry {
            id,
            at_ms: 1,
            run: None,
            retired,
            kind: EntryKind::Message {
                message,
                mode: pipkin_engine_protocol::InputMode::Prompt,
                request: None,
                partial: false,
            },
        }
    }

    #[test]
    fn context_excerpts_keep_the_newest_messages_within_bounds_and_say_what_was_left_out() {
        let mut entries = Vec::new();
        for i in 0..30u64 {
            entries.push(entry(
                i * 2,
                Message::user_text(format!("question {i}"), 1),
                false,
            ));
            entries.push(entry(
                i * 2 + 1,
                Message::Assistant {
                    content: vec![pipkin_engine_protocol::Block::text(format!("answer {i}"))],
                    api: "x".into(),
                    provider: "x".into(),
                    model: "x".into(),
                    usage: Default::default(),
                    stop_reason: pipkin_engine_protocol::StopReason::Stop,
                    error_message: None,
                    timestamp: 1,
                    extra: Default::default(),
                },
                false,
            ));
        }
        entries.push(entry(99, Message::user_text("retired", 1), true));
        entries.push(entry(100, Message::System(Default::default()), false));
        let (kept, omitted) = context_excerpt(&entries);
        assert_eq!(kept.len(), CONTEXT_MAX_MESSAGES);
        assert_eq!(omitted, 40);
        assert_eq!(kept[0], ("User".to_owned(), "question 20".to_owned()));
        assert_eq!(
            kept.last().unwrap(),
            &("Assistant".to_owned(), "answer 29".to_owned())
        );
        let prompt = compose_prompt("do it", &kept, omitted, 7);
        assert!(prompt.starts_with(
            "do it\n\n<parent_context revision=\"7\" messages=\"20\" omitted=\"40\">"
        ));
        assert!(prompt.ends_with("</parent_context>"));
        assert!(!prompt.contains("retired"));
        assert_eq!(compose_prompt("do it", &[], 0, 7), "do it");
        // A single oversized message keeps its tail.
        let huge = entry(
            1,
            Message::user_text("x".repeat(CONTEXT_MAX_CHARS + 10), 1),
            false,
        );
        let (kept, omitted) = context_excerpt(&[huge]);
        assert_eq!(kept.len(), 1);
        assert_eq!(omitted, 0);
        assert!(kept[0].1.starts_with('…'));
        assert_eq!(kept[0].1.chars().count(), CONTEXT_MAX_CHARS + 1);
    }
}

//! Normalized conversation content: the messages the engine keeps and replays to providers.
//!
//! The JSON shape intentionally matches the reference engine's message model (`role`, typed
//! content blocks, `usage`, `stopReason`) so that imported history deserializes without
//! translation. Every block and message keeps unknown fields in `extra`, so opaque provider
//! metadata (signatures, response ids, encrypted reasoning) survives a round trip byte for byte.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One block of message content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Block {
    Text {
        text: String,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    Thinking {
        thinking: String,
        /// Provider signature needed to replay this block (opaque; never shown).
        #[serde(
            rename = "thinkingSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        signature: Option<String>,
        /// The provider returned this reasoning encrypted/redacted; `thinking` is not readable.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    Image {
        /// Base64 data.
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

impl Block {
    pub fn text(text: impl Into<String>) -> Block {
        Block::Text {
            text: text.into(),
            extra: Map::new(),
        }
    }

    pub fn image(mime_type: impl Into<String>, data: impl Into<String>) -> Block {
        Block::Image {
            data: data.into(),
            mime_type: mime_type.into(),
            extra: Map::new(),
        }
    }

    pub fn tool_call(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Block {
        Block::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            extra: Map::new(),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Block::Text { text, .. } => Some(text),
            _ => None,
        }
    }
}

/// System content preserves both reference wire representations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemContent {
    Text(String),
    Blocks(Vec<Block>),
}

impl Default for SystemContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl SystemContent {
    pub fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// Ordered section patch and tool declarations at one position in model history.
/// Sections set in place, null removes, and a removed/re-added section moves to the end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    #[serde(default)]
    pub content: SystemContent,
    #[serde(
        default,
        skip_serializing_if = "Map::is_empty",
        deserialize_with = "system_sections"
    )]
    pub sections: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_removed: Vec<ToolReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_added: Vec<ToolDeclaration>,
    #[serde(default)]
    pub timestamp: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn system_sections<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Map<String, Value>, D::Error> {
    let sections = Map::<String, Value>::deserialize(d)?;
    if sections.values().any(|v| !v.is_null() && !v.is_string()) {
        return Err(serde::de::Error::custom(
            "system sections must be strings or null",
        ));
    }
    Ok(sections)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolReference {
    pub name: String,
}

/// Token and cost accounting for one assistant message, as the provider reported it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    /// Subset of cache_write with one-hour retention; absent if not reported.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "cacheWrite1h"
    )]
    pub cache_write_1h: Option<u64>,
    /// Subset of output; never add this again to total_tokens or cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: Cost,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        if let Some(value) = other.cache_write_1h {
            self.cache_write_1h = Some(self.cache_write_1h.unwrap_or(0).saturating_add(value));
        }
        if let Some(value) = other.reasoning {
            self.reasoning = Some(self.reasoning.unwrap_or(0).saturating_add(value));
        }
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.cost.input += other.cost.input;
        self.cost.output += other.cost.output;
        self.cost.cache_read += other.cost.cache_read;
        self.cost.cache_write += other.cost.cache_write;
        self.cost.total += other.cost.total;
    }
}

/// Dollar cost by component, computed from model metadata (not a billing guarantee).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub total: f64,
}

/// Why an assistant message ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    #[default]
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// A message in the model context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    System(SystemMessage),
    User {
        content: Vec<Block>,
        /// Unix milliseconds.
        #[serde(default)]
        timestamp: i64,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    Assistant {
        content: Vec<Block>,
        #[serde(default)]
        api: String,
        #[serde(default)]
        provider: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        usage: Usage,
        #[serde(rename = "stopReason", default)]
        stop_reason: StopReason,
        #[serde(
            rename = "errorMessage",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        error_message: Option<String>,
        #[serde(default)]
        timestamp: i64,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    ToolResult {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        content: Vec<Block>,
        #[serde(rename = "isError", default)]
        is_error: bool,
        #[serde(default)]
        timestamp: i64,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

impl Message {
    pub fn user_text(text: impl Into<String>, timestamp: i64) -> Message {
        Message::User {
            content: vec![Block::text(text)],
            timestamp,
            extra: Map::new(),
        }
    }

    pub fn content(&self) -> &[Block] {
        match self {
            Message::System(system) => match &system.content {
                SystemContent::Blocks(blocks) => blocks,
                SystemContent::Text(_) => &[],
            },
            Message::User { content, .. }
            | Message::Assistant { content, .. }
            | Message::ToolResult { content, .. } => content,
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            Message::System(system) => system.timestamp,
            Message::User { timestamp, .. }
            | Message::Assistant { timestamp, .. }
            | Message::ToolResult { timestamp, .. } => *timestamp,
        }
    }

    /// Concatenated text blocks.
    pub fn text(&self) -> String {
        if let Message::System(system) = self {
            return system.content.text();
        }
        let mut out = String::new();
        for block in self.content() {
            if let Block::Text { text, .. } = block {
                out.push_str(text);
            }
        }
        out
    }

    /// The tool calls an assistant message makes, in order.
    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        match self {
            Message::Assistant { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } => Some((id.as_str(), name.as_str(), arguments)),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }
}

/// An image attached to an input, already validated and base64-encoded by the client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    pub mime_type: String,
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_sections_preserve_order_and_reject_invalid_patch_values() {
        let raw =
            r#"{"role":"system","content":"","sections":{"z":"Z","a":null,"b":"B"},"timestamp":1}"#;
        let m: Message = serde_json::from_str(raw).unwrap();
        assert_eq!(serde_json::to_string(&m).unwrap(), raw);
        assert!(
            serde_json::from_str::<Message>(
                r#"{"role":"system","content":"","sections":{"a":42}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn usage_optional_subsets_round_trip_and_accumulate_without_double_counting() {
        let legacy = serde_json::to_value(Usage::default()).unwrap();
        assert!(legacy.get("reasoning").is_none());
        assert!(legacy.get("cacheWrite1h").is_none());
        let mut total: Usage = serde_json::from_value(legacy).unwrap();
        let reported: Usage = serde_json::from_value(json!({
            "input": 10, "output": 20, "cacheWrite": 8, "cacheWrite1h": 3,
            "reasoning": 5, "totalTokens": 38
        }))
        .unwrap();
        total.add(&reported);
        total.add(&Usage::default());
        total.add(&reported);
        assert_eq!(total.reasoning, Some(10));
        assert_eq!(total.cache_write_1h, Some(6));
        assert_eq!(total.total_tokens, 76);
        let encoded = serde_json::to_value(&total).unwrap();
        assert_eq!(encoded["cacheWrite1h"], 6);
        assert_eq!(serde_json::from_value::<Usage>(encoded).unwrap(), total);
        let mut zero = Usage::default();
        zero.add(&Usage {
            reasoning: Some(0),
            cache_write_1h: Some(0),
            ..Usage::default()
        });
        assert_eq!(zero.reasoning, Some(0));
        assert_eq!(zero.cache_write_1h, Some(0));
        let max = Usage {
            reasoning: Some(u64::MAX),
            cache_write_1h: Some(u64::MAX),
            ..Usage::default()
        };
        zero.add(&max);
        zero.add(&reported);
        assert_eq!(zero.reasoning, Some(u64::MAX));
        assert_eq!(zero.cache_write_1h, Some(u64::MAX));
    }

    #[test]
    fn reference_shaped_messages_round_trip_including_unknown_fields() {
        let raw = json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "hm", "thinkingSignature": "sig", "vendorBlob": [1, 2]},
                {"type": "text", "text": "hi", "textSignature": "t1"},
                {"type": "toolCall", "id": "c1", "name": "read", "arguments": {"path": "a"}}
            ],
            "api": "anthropic-messages",
            "provider": "anthropic",
            "model": "m",
            "usage": {"input": 1, "output": 2, "cacheRead": 3, "cacheWrite": 4, "totalTokens": 10,
                      "cost": {"input": 0.1, "output": 0.2, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.3}},
            "stopReason": "toolUse",
            "timestamp": 5,
            "responseId": "resp_1"
        });
        let message: Message = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), raw);
        assert_eq!(message.tool_calls().len(), 1);
    }

    #[test]
    fn tool_results_and_users_round_trip() {
        let raw = json!({"role": "toolResult", "toolCallId": "c1", "toolName": "bash",
            "content": [{"type": "text", "text": "ok"}], "isError": false, "timestamp": 1,
            "details": {"exitCode": 0}});
        let message: Message = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), raw);
        let user = json!({"role": "user", "content": [{"type": "text", "text": "x"},
            {"type": "image", "data": "AA==", "mimeType": "image/png"}], "timestamp": 2});
        let message: Message = serde_json::from_value(user.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), user);
    }
}

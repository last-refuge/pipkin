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
            Message::User { content, .. }
            | Message::Assistant { content, .. }
            | Message::ToolResult { content, .. } => content,
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            Message::User { timestamp, .. }
            | Message::Assistant { timestamp, .. }
            | Message::ToolResult { timestamp, .. } => *timestamp,
        }
    }

    /// Concatenated text blocks.
    pub fn text(&self) -> String {
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

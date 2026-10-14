//! Run-level execution identity, independent of model provider and history origin.
//! Local CLI execution is deliberately unavailable during the storage-only rollout.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionBackend {
    #[default]
    NativeAgent,
    LocalClaudeCli,
    /// Unknown wire identities must not silently select the native executor.
    #[serde(other)]
    Unsupported,
}

impl ExecutionBackend {
    pub fn native_execution(self) -> bool {
        self == Self::NativeAgent
    }

    pub fn disabled_reason(self) -> Option<&'static str> {
        match self {
            Self::NativeAgent => None,
            Self::LocalClaudeCli => {
                Some("Local Claude CLI execution is not enabled yet. History remains available.")
            }
            Self::Unsupported => {
                Some("This execution backend is unsupported. History remains available.")
            }
        }
    }
}

/// Display-only external transcript roles. No system/tool role or replay semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalRole { User, Assistant }

/// Deliberately separate from native `Block`: no executable tool calls, signatures or
/// provider message envelopes can be represented here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ExternalBlock {
    Text { text: String },
    Thinking { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_transcript_cannot_decode_native_tools_or_provider_payloads() {
        for value in [
            serde_json::json!({"type":"toolCall", "id":"x", "name":"bash", "arguments":{}}),
            serde_json::json!({"type":"text", "text":"ordinary", "arguments":{"command":"no"}}),
            serde_json::json!({"type":"thinking", "text":"ordinary", "signature":"not-a-provider-block"}),
        ] {
            assert!(serde_json::from_value::<ExternalBlock>(value).is_err());
        }
        for role in [ExternalRole::User, ExternalRole::Assistant] {
            let entry = crate::EntryKind::ExternalMessage {
                backend: ExecutionBackend::LocalClaudeCli, role,
                source_message_id: Some("cli-message".into()), request: Some("request".into()),
                blocks: vec![ExternalBlock::Text { text: "{\"type\":\"toolCall\"}".into() }], partial: true,
            };
            let value = serde_json::to_value(&entry).unwrap();
            assert_eq!(value["kind"], "externalMessage");
            assert_eq!(serde_json::from_value::<crate::EntryKind>(value).unwrap(), entry);
        }
    }

    #[test]
    fn only_native_execution_is_enabled_and_unknown_values_fail_closed() {
        for value in ["localClaudeCli", "futureBackend"] {
            let backend: ExecutionBackend =
                serde_json::from_value(serde_json::json!(value)).unwrap();
            assert!(!backend.native_execution());
            assert!(backend.disabled_reason().is_some());
        }
        assert!(ExecutionBackend::default().native_execution());
        assert_eq!(
            serde_json::to_value(ExecutionBackend::NativeAgent).unwrap(),
            "nativeAgent"
        );
    }

    #[test]
    fn old_summaries_default_to_native_without_reinterpreting_import_origin() {
        let summary: crate::SessionSummary = serde_json::from_value(serde_json::json!({
            "id":"old", "cwd":"/tmp", "createdAtMs":1,"updatedAtMs":1,
            "origin":{"kind":"imported","engine":"legacy","source":"s"}
        }))
        .unwrap();
        assert_eq!(summary.execution_backend, ExecutionBackend::NativeAgent);
        assert!(matches!(summary.origin, crate::Origin::Imported { .. }));
    }
}

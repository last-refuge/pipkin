//! Run-level execution identity, independent of model provider and history origin.
//! CLI readiness is overlaid by the owning connection; wire identity alone grants nothing.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionBackend {
    #[default]
    NativeAgent,
    LocalClaudeCli,
    ClaudeCliAgent,
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
            Self::ClaudeCliAgent => Some("Connect Claude orchestration to use this conversation."),
            Self::Unsupported => {
                Some("This execution backend is unsupported. History remains available.")
            }
        }
    }
}

/// Display-only external transcript roles. No system/tool role or replay semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalRole {
    User,
    Assistant,
}

/// Deliberately separate from native `Block`: no executable tool calls, signatures or
/// provider message envelopes can be represented here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ExternalBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// CLI-reported observation, not a native tool call, approval or filesystem proof.
    Action {
        source_tool_id: String,
        name: String,
        state: ExternalActionState,
        output: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalActionState {
    Observed,
    ReportedSuccess,
    ReportedFailure,
    Unknown,
}

/// Immutable launch policy selection. Metadata verification never enables execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalCliMode {
    Orchestration,
    Coding,
    #[default]
    ChatOnly,
    RestrictedWrite,
    #[serde(other)]
    Unsupported,
}

/// Fixed CLI model aliases, not provider model IDs or arbitrary command arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalCliModel {
    Haiku,
    Sonnet,
    Opus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LocalCliSetupTarget {
    New {
        project: String,
        model: LocalCliModel,
        /// Fixed requested effort; omission uses the CLI default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
    },
    Existing {
        session: String,
        model: LocalCliModel,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalCliSetupStage {
    AwaitingTrust,
    Checking,
    MetadataVerified,
    Failed,
}

/// Metadata verification is NOT an execution grant or proof of subscription billing.
/// Tickets are ephemeral, connection-scoped and never contain CLI credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalCliSetupView {
    pub setup: String,
    pub mode: LocalCliMode,
    pub target: LocalCliSetupTarget,
    pub selected_executable: String,
    pub executable: String,
    pub project: String,
    /// Legacy wire name: minimum supported version, not the observed executable version.
    pub qualified_version: String,
    pub stage: LocalCliSetupStage,
    pub reason: Option<String>,
    pub execution_enabled: bool,
}

/// Read-only hints for explicitly reapproving a saved CLI conversation after reconnect.
/// No credential/configuration contents or CLI-private session identifier are exposed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalCliSessionView {
    pub summary: crate::SessionSummary,
    pub mode: LocalCliMode,
    pub executable: String,
    pub cli_version: String,
    pub model: Option<LocalCliModel>,
    pub context_valid: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_cli_setup_accepts_only_fixed_models_and_explicit_trust_fields() {
        let inspect = crate::Call::InspectLocalCli {
            mode: LocalCliMode::ChatOnly,
            executable: "/opt/claude".into(),
            target: LocalCliSetupTarget::New {
                thinking: None,
                project: "/project".into(),
                model: LocalCliModel::Sonnet,
            },
        };
        let value = serde_json::to_value(&inspect).unwrap();
        assert_eq!(value["call"], "inspectLocalCli");
        assert!(serde_json::from_value::<crate::Call>(value.clone()).is_ok());
        let mut arbitrary = value;
        arbitrary["target"]["model"] = serde_json::json!("--arbitrary-flags");
        assert!(serde_json::from_value::<crate::Call>(arbitrary).is_err());
        assert!(
            serde_json::from_value::<LocalCliSetupTarget>(serde_json::json!({
                "kind":"new", "project":"/project", "model":"sonnet", "environment":{}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::Call>(serde_json::json!({
                "call":"checkLocalCli", "setup":"ticket"
            }))
            .is_err()
        );
        let check = crate::Call::CheckLocalCli {
            setup: "ticket".into(),
            trust_executable: true,
            trust_project: false,
        };
        assert!(
            serde_json::from_value::<crate::Call>(serde_json::to_value(check).unwrap()).is_ok()
        );
    }

    #[test]
    fn mode_is_explicit_closed_and_legacy_inspect_only_defaults_to_chat() {
        let mut value = serde_json::json!({"call":"inspectLocalCli", "executable":"/cli",
            "target":{"kind":"new", "project":"/project", "model":"sonnet"}});
        assert!(matches!(
            serde_json::from_value::<crate::Call>(value.clone()).unwrap(),
            crate::Call::InspectLocalCli {
                mode: LocalCliMode::ChatOnly,
                ..
            }
        ));
        for (wire, mode) in [
            ("restrictedWrite", LocalCliMode::RestrictedWrite),
            ("futureMode", LocalCliMode::Unsupported),
        ] {
            value["mode"] = serde_json::json!(wire);
            let crate::Call::InspectLocalCli { mode: decoded, .. } =
                serde_json::from_value(value.clone()).unwrap()
            else {
                panic!()
            };
            assert_eq!(decoded, mode);
        }
        value["mode"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<crate::Call>(value).is_err());
    }

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
                backend: ExecutionBackend::LocalClaudeCli,
                role,
                source_message_id: Some("cli-message".into()),
                request: Some("request".into()),
                blocks: vec![ExternalBlock::Text {
                    text: "{\"type\":\"toolCall\"}".into(),
                }],
                partial: true,
            };
            let value = serde_json::to_value(&entry).unwrap();
            assert_eq!(value["kind"], "externalMessage");
            assert_eq!(
                serde_json::from_value::<crate::EntryKind>(value).unwrap(),
                entry
            );
        }
    }

    #[test]
    fn action_reports_are_closed_inert_shapes_without_arguments_or_approvals() {
        let block = ExternalBlock::Action {
            source_tool_id: "tool-id".into(),
            name: "Write".into(),
            state: ExternalActionState::Unknown,
            output: "untrusted report".into(),
        };
        let value = serde_json::to_value(&block).unwrap();
        assert_eq!(
            serde_json::from_value::<ExternalBlock>(value.clone()).unwrap(),
            block
        );
        for extra in [
            "arguments",
            "approval",
            "nonce",
            "updatedInput",
            "permissions",
        ] {
            let mut injected = value.clone();
            injected[extra] = serde_json::json!({});
            assert!(serde_json::from_value::<ExternalBlock>(injected).is_err());
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

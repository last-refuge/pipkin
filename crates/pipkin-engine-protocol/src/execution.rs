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

#[cfg(test)]
mod tests {
    use super::*;

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

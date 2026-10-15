//! External CLI approvals, never native tools or model context.
use serde::{Deserialize, Serialize};

/// External prompt identity. No CLI-private UUID, account marker or owner supplied by clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliApprovalRun {
    pub epoch: u64,
    pub session: String,
    pub generation: u64,
    pub op: String,
    pub prompt: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliApprovalId {
    pub run: LocalCliApprovalRun,
    pub invocation: String,
    pub arguments_sha256: String,
}
impl std::fmt::Debug for LocalCliApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalCliApprovalId([redacted])")
    }
}

/// Full bounded, escaped inert argument display. No preview/truncation may be approved.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliApprovalReview {
    pub id: LocalCliApprovalId,
    pub arguments: String,
    /// Display hint only; the engine enforces monotonic expiry independently.
    pub expires_at_ms: i64,
}
impl std::fmt::Debug for LocalCliApprovalReview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalCliApprovalReview([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalCliApprovalDecision {
    AllowOnce,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalCliApprovalState {
    Pending,
    Granted,
    Consumed,
    Denied,
    Expired,
    Revoked,
}

/// Consumed is NOT proof of delivery or action completion. Never reissue an allow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliApprovalReceipt {
    pub id: LocalCliApprovalId,
    pub state: LocalCliApprovalState,
    pub decision: Option<LocalCliApprovalDecision>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decisions_and_identity_are_closed_and_debug_redacts_review_contents() {
        let id = LocalCliApprovalId {
            run: LocalCliApprovalRun {
                epoch: 1,
                session: "session".into(),
                generation: 1,
                op: "op".into(),
                prompt: 1,
            },
            invocation: "nonce-do-not-log".into(),
            arguments_sha256: "digest".into(),
        };
        let review = LocalCliApprovalReview {
            id: id.clone(),
            arguments: "private file content".into(),
            expires_at_ms: 1,
        };
        assert!(!format!("{review:?}").contains("private"));
        assert!(!format!("{id:?}").contains("nonce"));
        let mut value = serde_json::to_value(&id).unwrap();
        value["owner"] = serde_json::json!(4);
        assert!(serde_json::from_value::<LocalCliApprovalId>(value).is_err());
        for decision in ["allowAlways", "allow", "updatedInput", "unknown"] {
            assert!(
                serde_json::from_value::<LocalCliApprovalDecision>(serde_json::json!(decision))
                    .is_err()
            );
        }
        let mut value = serde_json::to_value(&review).unwrap();
        value["truncated"] = serde_json::json!(true);
        assert!(serde_json::from_value::<LocalCliApprovalReview>(value).is_err());
    }
}

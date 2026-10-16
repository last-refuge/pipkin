//! Run-level admission boundary. Only Prompt/Stop dispatch by backend.
//! An external agent is never an ApiAdapter and never falls back to the native run loop.

use pipkin_engine_protocol::{Call, ErrorCode, ProtoError};

use super::Shared;

impl Shared {
    pub(super) fn require_native_execution(&self, session: &str) -> Result<(), ProtoError> {
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        // Identity, not temporary availability: enabling a future external executor must
        // never make this native-only boundary pass.
        if !row.execution_backend.native_execution() {
            return Err(ProtoError::new(
                ErrorCode::Unsupported,
                row.execution_backend
                    .disabled_reason()
                    .unwrap_or("This operation requires native agent execution."),
            ));
        }
        // A native-looking child cannot bypass a disabled external root/parent.
        for ancestor in row.parent.iter().chain(row.root.iter()) {
            let owner = self.store.session(ancestor)?.ok_or_else(|| {
                ProtoError::new(ErrorCode::NotFound, "Session owner does not exist")
            })?;
            if !owner.execution_backend.native_execution() {
                return Err(ProtoError::new(
                    ErrorCode::Unsupported,
                    owner
                        .execution_backend
                        .disabled_reason()
                        .unwrap_or("This operation requires native agent execution."),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn guard_execution_call(&self, call: &Call) -> Result<(), ProtoError> {
        if let Some(session) = native_execution_session(call) {
            self.require_native_execution(session)?;
        }
        Ok(())
    }
}

/// Exhaustive on purpose: each new RPC must choose whether it may run for an unavailable
/// executor. History/metadata/global configuration stay available; native agent mutations
/// and session-scoped executable customizations do not. Removal is owned cleanup, not replay.
fn native_execution_session(call: &Call) -> Option<&str> {
    use Call::*;
    match call {
        Enqueue { session, .. }
        | CancelQueued { session, .. }
        | SetModel { session, .. }
        | SetThinking { session, .. }
        | CycleThinking { session }
        | Compact { session, .. }
        | AnswerQuestion { session, .. }
        | CancelQuestion { session, .. }
        | UpdatePlan { session, .. }
        | JobStart { session, .. }
        | JobCancel { session, .. }
        | SetGoal { session, .. }
        | GoalControl { session, .. }
        | MigrateGoal { session, .. }
        | SetSubagentsEnabled { session, .. }
        | StopSubagent { session, .. }
        | SteerSubagent { session, .. }
        | LaunchSubagent { session, .. } => Some(session),
        PrepareExtensions { session, .. }
        | McpServers { session }
        | McpUpsertServer { session, .. }
        | McpRemoveServer { session, .. }
        | McpSetDisabledTools { session, .. }
        | McpTestServer { session, .. }
        | McpReconnectServer { session, .. }
        | McpAuthStart { session, .. }
        | McpAuthRevoke { session, .. }
        | McpImportPreview { session, .. }
        | McpImportApply { session, .. } => session.as_deref(),
        Prompt { .. }
        | Stop { .. }
        | CancelRequest { .. }
        | Hello { .. }
        | CatchUp { .. }
        | AckEvents { .. }
        | ListSessions
        | CreateSession { .. }
        // Explicit setup may establish ephemeral trust only under the config opt-in.
        | DiscoverLocalCli { .. }
        | InspectLocalCli { .. }
        | CheckLocalCli { .. }
        | LocalCliSetupStatus { .. }
        | RevokeLocalCliSetup { .. }
        | CreateLocalCliSession { .. }
        | LookupLocalCliCreation { .. }
        | LocalCliSessionStatus { .. }
        | LocalCliApprovals { .. }
        | DecideLocalCliApproval { .. }
        | LookupLocalCliApproval { .. }
        | RenameSession { .. }
        | RemoveSession { .. }
        | OpenSession { .. }
        | ReleaseSession { .. }
        | HistoryPage { .. }
        | Lookup { .. }
        | WaitForPrompt { .. }
        | Models { .. }
        | RefreshModels
        | ToolOutput { .. }
        | DismissNotices { .. }
        | Subagents { .. }
        | Plan { .. }
        | WebSources { .. }
        | Jobs { .. }
        | JobLog { .. }
        | McpAuthAnswer { .. }
        | McpAuthCancel { .. }
        | GoalHistory { .. }
        | SubagentHistory { .. }
        | SubagentResult { .. }
        | Diagnostics { .. }
        | ExportDiagnostics { .. }
        | ExportHealth
        | ExportConfigure { .. }
        | ExportSetCredential { .. }
        | ExportClearCredential
        | ExportPreview { .. }
        | AuthState
        | AuthStart { .. }
        | AuthReuse { .. }
        | AuthAnswer { .. }
        | AuthCancel { .. }
        | AuthRemove { .. }
        | SetApiKey { .. }
        | ImportDiscover { .. }
        | ImportSessions { .. }
        | ImportConfig { .. }
        | ImportCredentials { .. }
        | Shutdown => None,
    }
}

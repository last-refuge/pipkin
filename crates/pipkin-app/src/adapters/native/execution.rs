//! Adapter-side execution policy. Readiness is a server fact, not a capability inferred
//! from installed executables, credentials, provenance, or a selected provider.
use pipkin_engine_protocol::{Call, ExecutionBackend, SessionSummary};

pub(super) const NATIVE_ONLY: &str =
    "This request requires native agent execution. History remains available.";
pub(super) const UNKNOWN: &str =
    "This execution backend is unsupported. History remains available.";

pub(super) fn prompt(
    summary: &SessionSummary,
    attachments: bool,
    model: bool,
) -> Result<(), String> {
    match summary.execution_backend {
        ExecutionBackend::NativeAgent => Ok(()), // Preserve native provider/readiness handling.
        ExecutionBackend::Unsupported => Err(UNKNOWN.into()),
        ExecutionBackend::ClaudeCliAgent => {
            if summary.local_cli_mode != Some(pipkin_engine_protocol::LocalCliMode::Orchestration) {
                return Err(UNKNOWN.into());
            }
            if let Some(reason) = &summary.blocked {
                return Err(reason.clone());
            }
            if model {
                return Err("A different CLI model requires a separate conversation.".into());
            }
            Ok(())
        }
        ExecutionBackend::LocalClaudeCli => {
            if !matches!(
                summary.local_cli_mode,
                Some(
                    pipkin_engine_protocol::LocalCliMode::Coding
                        | pipkin_engine_protocol::LocalCliMode::ChatOnly
                        | pipkin_engine_protocol::LocalCliMode::RestrictedWrite
                )
            ) {
                return Err(UNKNOWN.into());
            }
            if let Some(reason) = &summary.blocked {
                return Err(reason.clone());
            }
            if attachments || model {
                return Err(
                    "Local CLI accepts text only, without attachments or a model override.".into(),
                );
            }
            Ok(())
        }
    }
}

pub(super) fn stop(summary: &SessionSummary, active: bool) -> bool {
    match summary.execution_backend {
        ExecutionBackend::NativeAgent => true,
        ExecutionBackend::LocalClaudeCli | ExecutionBackend::ClaudeCliAgent => {
            active || summary.blocked.is_none()
        }
        ExecutionBackend::Unsupported => false,
    }
}

/// Guard the shared call boundary, including internal queue reconciliation paths.
/// Read-only receipts/history are deliberately not gated. MCP and subagent surfaces
/// remain native-only even when the server eventually reports CLI chat ready.
pub(super) fn guard<'a>(
    call: &Call,
    current: Option<&str>,
    summary: impl Fn(&str) -> Option<&'a SessionSummary>,
) -> Result<(), String> {
    let session = match call {
        Call::Prompt {
            session,
            images,
            model,
            ..
        } => {
            return prompt(
                summary(session).ok_or(UNKNOWN)?,
                !images.is_empty(),
                model.is_some(),
            );
        }
        Call::Enqueue { session, .. }
        | Call::CancelQueued { session, .. }
        | Call::SetModel { session, .. }
        | Call::SetThinking { session, .. }
        | Call::CycleThinking { session }
        | Call::Compact { session, .. }
        | Call::AnswerQuestion { session, .. }
        | Call::CancelQuestion { session, .. }
        | Call::DismissNotices { session }
        | Call::Subagents { session }
        | Call::SubagentHistory { session, .. }
        | Call::SubagentResult { session, .. }
        | Call::SetSubagentsEnabled { session, .. }
        | Call::StopSubagent { session, .. }
        | Call::SteerSubagent { session, .. }
        | Call::LaunchSubagent { session, .. }
        | Call::SetGoal { session, .. }
        | Call::GoalControl { session, .. }
        | Call::MigrateGoal { session, .. }
        | Call::UpdatePlan { session, .. }
        | Call::Plan { session, .. }
        | Call::GoalHistory { session }
        | Call::Jobs { session }
        | Call::JobLog { session, .. }
        | Call::WebSources { session }
        | Call::JobStart { session, .. }
        | Call::JobCancel { session, .. } => Some(session.as_str()),
        Call::PrepareExtensions { session, .. }
        | Call::McpServers { session }
        | Call::McpUpsertServer { session, .. }
        | Call::McpRemoveServer { session, .. }
        | Call::McpSetDisabledTools { session, .. }
        | Call::McpTestServer { session, .. }
        | Call::McpReconnectServer { session, .. }
        | Call::McpAuthStart { session, .. }
        | Call::McpAuthRevoke { session, .. }
        | Call::McpImportPreview { session, .. }
        | Call::McpImportApply { session, .. } => session.as_deref().or(current),
        Call::McpAuthAnswer { .. } | Call::McpAuthCancel { .. } | Call::RefreshModels => current,
        Call::Stop { session } => {
            return if summary(session).is_some_and(|s| stop(s, false)) {
                Ok(())
            } else {
                Err("Stop is unavailable for this execution profile.".into())
            };
        }
        _ => return Ok(()),
    };
    if let Some(session) = session
        && summary(session).is_some_and(|s| {
            s.execution_backend == ExecutionBackend::ClaudeCliAgent
                && s.local_cli_mode == Some(pipkin_engine_protocol::LocalCliMode::Orchestration)
        })
        && matches!(
            call,
            Call::Enqueue { .. }
                | Call::CancelQueued { .. }
                | Call::Compact { .. }
                | Call::SetGoal { .. }
                | Call::GoalControl { .. }
                | Call::AnswerQuestion { .. }
                | Call::CancelQuestion { .. }
                | Call::DismissNotices { .. }
                | Call::Subagents { .. }
                | Call::SubagentHistory { .. }
                | Call::SubagentResult { .. }
                | Call::Plan { .. }
                | Call::GoalHistory { .. }
                | Call::Jobs { .. }
                | Call::JobLog { .. }
                | Call::WebSources { .. }
                | Call::UpdatePlan { .. }
                | Call::JobCancel { .. }
                | Call::SetSubagentsEnabled { .. }
                | Call::StopSubagent { .. }
                | Call::LaunchSubagent { .. }
                | Call::SteerSubagent { .. }
                | Call::McpServers { .. }
                | Call::McpUpsertServer { .. }
                | Call::McpRemoveServer { .. }
                | Call::McpSetDisabledTools { .. }
                | Call::McpTestServer { .. }
                | Call::McpReconnectServer { .. }
                | Call::McpAuthStart { .. }
                | Call::McpAuthRevoke { .. }
                | Call::McpImportPreview { .. }
                | Call::McpImportApply { .. }
                | Call::McpAuthAnswer { .. }
                | Call::McpAuthCancel { .. }
        )
    {
        return Ok(());
    }
    if let Some(session) = session
        && !summary(session).is_some_and(|s| s.execution_backend.native_execution())
    {
        return Err(NATIVE_ONLY.into());
    }
    Ok(())
}

/// An external/unknown identity cannot be promoted to native by a later summary.
/// A conflicting immutable identity fails closed rather than choosing an executor.
pub(super) fn preserve_identity(
    previous: &SessionSummary,
    next: &mut SessionSummary,
    pinned: bool,
) {
    if (pinned || !previous.execution_backend.native_execution())
        && previous.execution_backend != next.execution_backend
    {
        next.execution_backend = ExecutionBackend::Unsupported;
        next.blocked = Some(UNKNOWN.into());
    }
    if matches!(
        previous.execution_backend,
        ExecutionBackend::LocalClaudeCli | ExecutionBackend::ClaudeCliAgent
    ) && previous.local_cli_mode != next.local_cli_mode
    {
        next.execution_backend = ExecutionBackend::Unsupported;
        next.local_cli_mode = Some(pipkin_engine_protocol::LocalCliMode::Unsupported);
        next.blocked = Some(UNKNOWN.into());
    }
}

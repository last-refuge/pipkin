//! Native CLI services: independent original-turn authority + authenticated per-call intent.
//! Never delegate inference/recovery to subagents::Host. Unimplemented services fail closed.
use super::{
    Shared,
    cli_agent_owner::ServiceAuthority,
    cli_bridge_driver::{BoundHost, DriverError, HostFactory, Publication},
};
use crate::{
    provider::BoxFuture,
    store::{Store, StoreError, cli_agent_turns::projection::Update, cli_bridge::Lease},
    tools::{SubagentLaunch, ToolHost, text::BoundedOutput},
    util::now_ms,
};
use pipkin_engine_protocol::{PlanOp, PlanReceipt, PlanRejected, PlanView, SessionChange};
use serde_json::Value;
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(super) fn factory(
    shared: Arc<Shared>,
    authority: ServiceAuthority,
    publication: Arc<Publication>,
    children: Arc<ChildResources>,
) -> Arc<HostFactory> {
    Arc::new(move |lease, claim, name, cancel, deadline| {
        if lease.scope() != authority.scope() || cancel.is_cancelled() || Instant::now() >= deadline
        {
            return Err(DriverError::Setup);
        }
        authority
            .authorize(|| {
                shared
                    .store
                    .read(|c| Store::assert_cli_native_service_in(c, &lease, claim.call_id(), name))
            })
            .map_err(|_| DriverError::Setup)?;
        let host = Arc::new(Host {
            shared: shared.clone(),
            children: children.clone(),
            authority: authority.clone(),
            publication: publication.clone(),
            lease,
            call: claim.call_id().into(),
            name: name.into(),
            cancel,
            deadline,
        });
        let progress_host = host.clone();
        Ok(BoundHost {
            host,
            progress: Arc::new(move |text| progress_host.publish_progress(text)),
        })
    })
}
mod children;
mod goals;
mod jobs;
mod wake;
pub(super) use goals::GoalAuthority;
pub(super) use jobs::JobAuthority;
#[cfg(test)]
pub(super) use jobs::StartPause;
mod questions;
mod services;
pub(super) use children::ChildResources;
pub(super) use questions::Questions;
use services::{Listing, McpResult};

#[derive(Clone)]
struct Host {
    shared: Arc<Shared>,
    children: Arc<ChildResources>,
    authority: ServiceAuthority,
    publication: Arc<Publication>,
    lease: Arc<Lease>,
    call: String,
    name: String,
    cancel: CancellationToken,
    deadline: Instant,
}
impl Host {
    fn live(&self, c: &rusqlite::Connection) -> crate::store::Result<()> {
        if self.cancel.is_cancelled() || Instant::now() >= self.deadline {
            return Err(StoreError::ExecutionConflict);
        }
        Store::assert_cli_native_service_in(c, &self.lease, &self.call, &self.name)
    }
    fn named(&self, name: &str, call: Option<&str>) -> Result<(), String> {
        if self.name != name || call.is_some_and(|id| id != self.call) {
            return Err("Native service call identity mismatch".into());
        }
        Ok(())
    }
    fn check(&self) -> Result<(), String> {
        self.publication
            .commit(self.authority.scope(), || {
                self.authority.authorize(|| {
                    self.shared.store.read(|c| {
                        self.live(c)?;
                        Ok(((), None))
                    })
                })
            })
            .map_err(|e| e.to_string())
    }
    fn transaction<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction) -> crate::store::Result<(T, Option<Update>)>,
    ) -> Result<T, String> {
        // The publication gate is acquired BEFORE registry/storage, as with root admission.
        self.publication
            .commit(self.authority.scope(), || {
                self.authority.authorize(|| {
                    self.shared.store.tx(|tx| {
                        self.live(tx)?;
                        f(tx)
                    })
                })
            })
            .map_err(|e| e.to_string())
    }
}
impl ToolHost for Host {
    fn goal_get<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Option<pipkin_engine_protocol::GoalView>, String>> {
        Box::pin(async move {
            self.named("get_goal", None)?;
            tokio::task::block_in_place(|| {
                self.transaction(|tx| {
                    Ok((
                        Store::goal_current(tx, &self.authority.scope().session)?,
                        None,
                    ))
                })
            })
        })
    }
    fn goal_update<'a>(
        &'a self,
        call: &'a str,
        request: crate::tools::goal::GoalUpdateRequest,
    ) -> BoxFuture<'a, Result<Result<pipkin_engine_protocol::GoalView, String>, String>> {
        Box::pin(async move {
            self.named("update_goal", Some(call))?;
            tokio::task::block_in_place(|| {
                self.shared.goal_update_cli(
                    &self.authority.scope().session,
                    request,
                    &GoalAuthority(self.clone()),
                )
            })
        })
    }
    fn goal_propose<'a>(
        &'a self,
        call: &'a str,
        proposal: crate::tools::goal::GoalProposal,
    ) -> BoxFuture<
        'a,
        Result<
            Result<
                pipkin_engine_protocol::GoalView,
                (Vec<String>, Option<pipkin_engine_protocol::GoalView>),
            >,
            String,
        >,
    > {
        Box::pin(async move {
            self.named("propose_goal_completion", Some(call))?;
            let result = tokio::task::block_in_place(|| {
                self.shared.goal_propose_cli(
                    &self.authority.scope().session,
                    proposal,
                    &GoalAuthority(self.clone()),
                )
            })?;
            match result {
                Ok(goal) if goal.state == pipkin_engine_protocol::GoalState::Verifying => {
                    self.audit_goal(goal).await.map(Ok)
                }
                other => Ok(other),
            }
        })
    }
    fn job_start<'a>(
        &'a self,
        call: &'a str,
        spec: super::jobs::JobSpec,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::JobView, String>> {
        Box::pin(async move {
            self.named("job_start", Some(call))?;
            tokio::task::block_in_place(|| self.check())?;
            self.start_job(spec).await.map_err(|_| {
                "CLI job startup ended without a confirmed reply; do not replay".to_string()
            })?
        })
    }
    fn jobs<'a>(&'a self) -> BoxFuture<'a, Result<pipkin_engine_protocol::JobsView, String>> {
        Box::pin(async move {
            self.service("job_status", None, self.cancel.clone(), async {
                tokio::task::block_in_place(|| {
                    self.shared.jobs_view(&self.authority.scope().session)
                })
                .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn job_log<'a>(
        &'a self,
        id: u64,
        offset: u64,
        limit: Option<u64>,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::JobLogView, String>> {
        Box::pin(async move {
            self.service("job_log", None, self.cancel.clone(), async {
                tokio::task::block_in_place(|| {
                    self.shared
                        .job_log(&self.authority.scope().session, id, offset, limit)
                })
                .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn job_wait<'a>(
        &'a self,
        id: u64,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::JobView, String>> {
        Box::pin(async move {
            self.service("job_wait", None, cancel.clone(), async {
                self.shared
                    .job_wait(&self.authority.scope().session, id, timeout, &cancel)
                    .await
                    .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn job_cancel<'a>(
        &'a self,
        id: u64,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::JobView, String>> {
        Box::pin(async move {
            self.service("job_cancel", None, self.cancel.clone(), async {
                self.shared
                    .job_cancel(&self.authority.scope().session, id)
                    .await
                    .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn subagent_status<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::SubagentsView, String>> {
        Box::pin(async move {
            self.service("subagent_status", None, self.cancel.clone(), async {
                tokio::task::block_in_place(|| {
                    self.shared.subagents_view(&self.authority.scope().session)
                })
                .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn subagent_result<'a>(
        &'a self,
        child: u64,
        offset: usize,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::SubagentResultView, String>> {
        Box::pin(async move {
            self.service("subagent_result", None, self.cancel.clone(), async {
                tokio::task::block_in_place(|| {
                    self.shared
                        .subagent_result(&self.authority.scope().session, child, offset)
                })
                .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn subagent_answer<'a>(&'a self, child: u64) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            self.named("subagent_result", None)?;
            tokio::task::block_in_place(|| {
                self.transaction(|tx| {
                    let task = Store::task_by_child(tx, &self.authority.scope().session, child)?
                        .ok_or(StoreError::ExecutionConflict)?;
                    Ok((task.result, None))
                })
            })
        })
    }
    fn subagent_wait<'a>(
        &'a self,
        child: u64,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<pipkin_engine_protocol::SubagentResultView, String>> {
        Box::pin(async move {
            self.service("subagent_wait", None, cancel.clone(), async {
                self.shared
                    .subagent_wait(&self.authority.scope().session, child, timeout, &cancel)
                    .await
                    .map_err(|e| e.message)
            })
            .await?
        })
    }
    fn web_search<'a>(
        &'a self,
        call: &'a str,
        query: &'a str,
        count: usize,
        cancel: CancellationToken,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(String, Vec<crate::web::SourceRecord>), crate::web::providers::SearchError>,
            String,
        >,
    > {
        Box::pin(async move {
            let retention = cancel.clone();
            let found = self
                .service("web_search", Some(call), cancel.clone(), async {
                    let service = tokio::task::block_in_place(|| {
                        self.shared.web_service(&self.authority.scope().session)
                    });
                    service.search_raw(query, count, cancel).await
                })
                .await?;
            let (provider, found) = match found {
                Ok(found) => found,
                Err(e) => return Ok(Err(e)),
            };
            let records = tokio::task::block_in_place(|| {
                self.transaction(|tx| {
                    if retention.is_cancelled() {
                        return Err(StoreError::ExecutionConflict);
                    }
                    let records = Store::record_search(
                        tx,
                        &self.authority.scope().session,
                        call,
                        &provider,
                        query.trim(),
                        &found,
                        now_ms(),
                    )?;
                    Ok((records, Some(self.web_update(tx)?)))
                })
            })?;
            Ok(Ok((provider, records)))
        })
    }
    fn web_fetch<'a>(
        &'a self,
        call: &'a str,
        url: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<
        'a,
        Result<
            Result<(crate::web::SourceRecord, crate::web::Fetched), crate::web::FetchError>,
            String,
        >,
    > {
        Box::pin(async move {
            let retention = cancel.clone();
            let fetched = self
                .service("web_fetch", Some(call), cancel.clone(), async {
                    let service = tokio::task::block_in_place(|| {
                        self.shared.web_service(&self.authority.scope().session)
                    });
                    crate::web::fetch(&service.net, url, service.allow_private, cancel).await
                })
                .await?;
            let fetched = match fetched {
                Ok(fetched) => fetched,
                Err(e) => return Ok(Err(e)),
            };
            let record = tokio::task::block_in_place(|| {
                self.transaction(|tx| {
                    if retention.is_cancelled() {
                        return Err(StoreError::ExecutionConflict);
                    }
                    let record = Store::record_fetch(
                        tx,
                        &self.authority.scope().session,
                        call,
                        &fetched,
                        now_ms(),
                    )?;
                    Ok((record, Some(self.web_update(tx)?)))
                })
            })?;
            Ok(Ok((record, fetched)))
        })
    }
    fn web_source<'a>(
        &'a self,
        id: u64,
    ) -> BoxFuture<'a, Result<(Option<crate::web::SourceRecord>, Option<String>), String>> {
        Box::pin(async move {
            self.named("web_read", None)?;
            tokio::task::block_in_place(|| {
                self.transaction(|tx| {
                    let session = &self.authority.scope().session;
                    Ok((
                        (
                            Store::web_source(tx, session, id)?,
                            Store::web_source_text(tx, session, id)?,
                        ),
                        None,
                    ))
                })
            })
        })
    }
    fn mcp_search<'a>(
        &'a self,
        query: &'a str,
        server: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Listing<crate::mcp::ToolInfo>>> {
        Box::pin(
            self.service("mcp_search", None, self.cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared.mcp.search(query, server).await
            }),
        )
    }
    fn mcp_describe<'a>(
        &'a self,
        qualified: &'a str,
    ) -> BoxFuture<'a, McpResult<crate::mcp::ToolInfo>> {
        Box::pin(
            self.service("mcp_describe", None, self.cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared.mcp.describe(qualified).await
            }),
        )
    }
    fn mcp_call<'a>(
        &'a self,
        call: &'a str,
        qualified: &'a str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, McpResult<crate::mcp::CallOutcome>> {
        Box::pin(
            self.service("mcp_call", Some(call), cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared
                    .mcp
                    .call_scoped(
                        &self.authority.scope().session,
                        qualified,
                        arguments,
                        cancel,
                        Some(self.mcp_elicitor()),
                    )
                    .await
            }),
        )
    }
    fn mcp_resources<'a>(
        &'a self,
        server: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Listing<crate::mcp::ResourceInfo>>> {
        Box::pin(
            self.service("mcp_resources", None, self.cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared.mcp_resources(server).await
            }),
        )
    }
    fn mcp_read<'a>(
        &'a self,
        server: &'a str,
        uri: &'a str,
    ) -> BoxFuture<'a, McpResult<Vec<crate::mcp::ResourcePart>>> {
        Box::pin(
            self.service("mcp_read", None, self.cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared.mcp.read_resource(server, uri).await
            }),
        )
    }
    fn mcp_prompts<'a>(
        &'a self,
        server: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Listing<crate::mcp::PromptInfo>>> {
        Box::pin(
            self.service("mcp_prompts", None, self.cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared.mcp_prompts(server).await
            }),
        )
    }
    fn mcp_prompt<'a>(
        &'a self,
        server: &'a str,
        prompt: &'a str,
        arguments: serde_json::Map<String, Value>,
    ) -> BoxFuture<'a, McpResult<crate::mcp::PromptOutcome>> {
        Box::pin(
            self.service("mcp_prompt", None, self.cancel.clone(), async move {
                if let Err(e) = tokio::task::block_in_place(|| self.refresh_mcp()) {
                    return Err(crate::mcp::McpError::InvalidArguments(e));
                }
                self.shared.mcp.get_prompt(server, prompt, arguments).await
            }),
        )
    }
    fn ask<'a>(
        &'a self,
        call: &'a str,
        question: crate::extensions::questions::Question,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, crate::tools::ask::AskOutcome> {
        Box::pin(self.ask_scoped(call, question, cancel))
    }
    fn checkpoint_output(&self, call: &str, output: &BoundedOutput) -> Result<(), String> {
        if call != self.call {
            return Err("Native checkpoint call identity mismatch".into());
        }
        self.transaction(|tx| {
            Store::save_tool_progress(tx, &self.authority.scope().session, call, output)?;
            Ok(((), None))
        })
    }
    fn subagent<'a>(
        &'a self,
        call: &'a str,
        launch: SubagentLaunch,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(String, Value), String>> {
        Box::pin(async move {
            self.named("subagent", Some(call))?;
            self.start_child(launch, cancel).await.map_err(|_| {
                "CLI child actor ended without a confirmed reply; do not replay".to_string()
            })?
        })
    }
    fn plan_update<'a>(
        &'a self,
        call: &'a str,
        ops: Vec<PlanOp>,
        expected: Option<u64>,
    ) -> BoxFuture<'a, Result<Result<PlanReceipt, PlanRejected>, String>> {
        Box::pin(async move {
            self.named("plan_update", Some(call))?;
            tokio::task::block_in_place(|| {
                self.transaction(|tx| {
                    let scope = self.authority.scope();
                    let (outcome, applied) = Store::apply_plan(
                        tx,
                        &scope.session,
                        &format!("call:{call}"),
                        &ops,
                        expected,
                        now_ms(),
                    )?;
                    let update = if applied {
                        let plan = Store::plan_view(tx, &scope.session, false)?;
                        let revision = Store::bump(tx, &scope.session, now_ms())?;
                        Some(Update {
                            scope: scope.clone(),
                            revision,
                            changes: vec![SessionChange::PlanChanged { plan }],
                        })
                    } else {
                        None
                    };
                    Ok((outcome, update))
                })
            })
        })
    }
    fn plan_read<'a>(&'a self, archived: bool) -> BoxFuture<'a, Result<PlanView, String>> {
        Box::pin(async move {
            // PlanUpdateTool reads the committed view to format its result. This read is
            // part of that call's service bundle, not authority to mutate through plan_read.
            if !matches!(self.name.as_str(), "plan_read" | "plan_update") {
                return Err("Native plan read identity mismatch".into());
            }
            tokio::task::block_in_place(|| {
                self.publication
                    .commit(self.authority.scope(), || {
                        self.authority.authorize(|| {
                            self.shared.store.read(|c| {
                                self.live(c)?;
                                Ok((
                                    Store::plan_view(c, &self.authority.scope().session, archived)?,
                                    None,
                                ))
                            })
                        })
                    })
                    .map_err(|e: StoreError| e.to_string())
            })
        })
    }
}

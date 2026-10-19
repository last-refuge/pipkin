//! Scoped native tool executor over the independently authenticated channel. Not activated by
//! any CLI launcher/session creator. This service never requests a model or resumes a receipt.
//! Native services/catalog are supplied by the trusted owner; no native-only guard is widened.
use super::cli_bridge_channel::{Connected, Parts};
use crate::{
    execution::claude_cli::orchestration_protocol::{self as protocol, CallKey, Incoming, Scope},
    store::{
        Store,
        cli_bridge::{Admission, Claim, Lease, Settlement},
    },
    tools::{self, MutationLocks, Tool, ToolCtx, ToolHost},
    util::now_ms,
};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinSet,
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverError {
    Setup,
    Storage,
    Channel,
    Cancelled,
    Protocol,
    Worker,
    Busy,
}
mod close;
mod publication;
#[cfg(test)]
mod publication_tests;
pub use close::{CloseHandle, CloseRequest, Closed, close_channel};
pub use publication::Publication;
/// Owner-provided native resources. No API adapter/model resolver exists in this driver.
pub type Progress = dyn Fn(&Scope, &str, &str) + Send + Sync;
pub type HostFactory = dyn Fn(Arc<Lease>, &Claim, &str, CancellationToken, Instant) -> Result<BoundHost, DriverError>
    + Send
    + Sync;
pub struct BoundHost {
    pub host: Arc<dyn ToolHost>,
    pub progress: Arc<dyn Fn(&str) + Send + Sync>,
}
pub(in crate::runtime) type Settle = dyn Fn(
        &Lease,
        &Claim,
        &tools::ToolResult,
    ) -> crate::store::Result<(
        (Settlement, Value),
        Option<crate::store::cli_agent_turns::projection::Update>,
    )> + Send
    + Sync;
pub struct Services {
    pub(in crate::runtime) settle: Option<Arc<Settle>>,
    pub(in crate::runtime) child_resources: Option<Arc<super::cli_agent_host::ChildResources>>,
    #[cfg(test)]
    pub(crate) child_peers:
        Option<tokio::sync::mpsc::UnboundedSender<super::cli_bridge_channel::Launch>>,
    #[cfg(test)]
    pub(in crate::runtime) job_start_pause: Option<Arc<super::cli_agent_host::StartPause>>,
    pub locks: Arc<MutationLocks>,
    pub processes: Arc<Semaphore>,
    pub host: Option<Arc<dyn ToolHost>>,
    /// Per-call service authority, minted only after authenticated native intent admission.
    pub host_factory: Option<Arc<HostFactory>>,
    pub shell: Option<PathBuf>,
    pub progress: Arc<Progress>,
    /// Optional owner-captured commit/delivery lane; never execution authority itself.
    pub publication: Option<Arc<Publication>>,
}
impl Default for Services {
    fn default() -> Self {
        Self {
            settle: None,
            child_resources: None,
            #[cfg(test)]
            child_peers: None,
            #[cfg(test)]
            job_start_pause: None,
            locks: Arc::new(MutationLocks::default()),
            processes: Arc::new(Semaphore::new(4)),
            host: None,
            host_factory: None,
            shell: None,
            progress: Arc::new(|_, _, _| {}),
            publication: None,
        }
    }
}
struct Checkpoints {
    store: Arc<Store>,
    lease: Arc<Lease>,
}
impl ToolHost for Checkpoints {
    fn subagent<'a>(
        &'a self,
        _: &'a str,
        _: tools::SubagentLaunch,
        _: CancellationToken,
    ) -> crate::provider::BoxFuture<'a, Result<(String, Value), String>> {
        Box::pin(async { Err("No orchestration child service is configured".into()) })
    }
    fn checkpoint_output(
        &self,
        call: &str,
        output: &tools::text::BoundedOutput,
    ) -> Result<(), String> {
        self.store
            .cli_native_progress(&self.lease, call, output)
            .map_err(|e| e.to_string())
    }
}
struct ReaderGuard {
    task: tokio::task::JoinHandle<()>,
    cancel: CancellationToken,
}
impl Drop for ReaderGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
const MAX_ACTIVE_TOOLS: usize = 4;
const MAX_PENDING_BYTES: usize = 2 * protocol::MAX_FRAME_BYTES;
const CANCEL_GRACE: Duration = Duration::from_secs(5);
struct Active {
    key: CallKey,
    cancel: CancellationToken,
    cancelled_at: Option<Instant>,
    sequential: bool,
    bytes: usize,
}
struct Work {
    key: CallKey,
    name: String,
    args: Value,
    bytes: usize,
}
struct Finished {
    key: CallKey,
    claim: Claim,
    result: tools::ToolResult,
    cancel: CancellationToken,
}

/// Bounded parallel native execution with FIFO sequential barriers. A dedicated one-frame
/// reader avoids cancellation-unsafe reads. Pending arguments share a byte budget; four
/// workers bound completed results and one outgoing frame applies backpressure. Caller owns process/CLI lifecycle and must retire
/// its captured identity if THIS future is dropped. Every normal/error exit below fences the
/// connection; no EOF/helper exit is treated as successful root-model completion.
pub async fn serve(
    store: Arc<Store>,
    connected: Connected,
    offered: Vec<Arc<dyn Tool>>,
    services: Services,
    owner_cancel: CancellationToken,
    deadline: Instant,
) -> Result<(), DriverError> {
    serve_inner(
        store,
        connected,
        offered,
        services,
        owner_cancel,
        deadline,
        None,
    )
    .await
}
/// The trusted root owner may request clean closure after observing its terminal boundary.
/// Closure still requires protocol quiescence, helper acknowledgment and durable receipts.
/// Neither this return value nor the close acknowledgment establishes root-model success.
pub async fn serve_controlled(
    store: Arc<Store>,
    connected: Connected,
    offered: Vec<Arc<dyn Tool>>,
    services: Services,
    owner_cancel: CancellationToken,
    deadline: Instant,
    close: CloseRequest,
) -> Result<(), DriverError> {
    serve_inner(
        store,
        connected,
        offered,
        services,
        owner_cancel,
        deadline,
        Some(close),
    )
    .await
}
async fn serve_inner(
    store: Arc<Store>,
    connected: Connected,
    offered: Vec<Arc<dyn Tool>>,
    services: Services,
    owner_cancel: CancellationToken,
    deadline: Instant,
    mut close: Option<CloseRequest>,
) -> Result<(), DriverError> {
    let deadline = deadline.min(connected.deadline());
    let catalog_hash = connected.native_catalog();
    let Parts {
        mut reader,
        mut writer,
        lease,
    } = connected.into_parts();
    let lease = Arc::new(lease);
    let link = reader.revocation();
    let (send, mut receive) = mpsc::channel(1);
    let work_cancel = owner_cancel.child_token();
    let read_cancel = work_cancel.clone();
    let read_task = tokio::spawn(async move {
        while let Ok(frame) = reader.read(deadline).await {
            if send.send(frame).await.is_err() {
                break;
            }
        }
        reader.revoke();
        read_cancel.cancel();
    });
    let guard = ReaderGuard {
        task: read_task,
        cancel: work_cancel.clone(),
    };
    let mut workers = JoinSet::new();
    let mut close_ack = None;
    let mut result = async {
        if close.as_ref().is_some_and(|c|c.scope!=*lease.scope()) {return Err(DriverError::Setup);}
        let row = store.cli_native_context(&lease).map_err(|_| DriverError::Setup)?;
        if !matches!(row.model.as_deref(), Some("haiku" | "sonnet" | "opus"))
            || !std::path::Path::new(&row.cwd).is_absolute()
        {
            return Err(DriverError::Setup);
        }
        let model = format!("claude-cli/{}", row.model.as_deref().ok_or(DriverError::Setup)?);
        let specs: Vec<_> = offered.iter().map(|t| t.spec()).collect();
        let mut session = protocol::Session::new(lease.scope().clone(), &specs)
            .map_err(|_| DriverError::Setup)?;
        if Some(session.catalog_hash()) != catalog_hash {
            return Err(DriverError::Setup);
        }
        let tools: HashMap<_, _> = offered.into_iter().zip(specs)
            .map(|(tool, spec)| (spec.name.clone(), (tool, spec))).collect();
        let host = services.host.clone().unwrap_or_else(|| Arc::new(Checkpoints {
            store: store.clone(), lease: lease.clone(),
        }));
        let mut queued = VecDeque::<Work>::new();
        let mut active = Vec::<Active>::new();
        let mut pending_bytes = 0usize;
        loop {
            if owner_cancel.is_cancelled() || link.is_cancelled() || Instant::now() >= deadline {
                return Err(DriverError::Cancelled);
            }
            while !close.as_ref().is_some_and(|c|c.requested.is_cancelled())
                && workers.len() < MAX_ACTIVE_TOOLS
                && !active.iter().any(|a| a.sequential)
                && queued.front().is_some_and(|w| active.is_empty() || tools.get(&w.name).is_some_and(|(t,_)| !t.sequential()))
            {
                if owner_cancel.is_cancelled() || link.is_cancelled() || Instant::now() >= deadline {return Err(DriverError::Cancelled);}
                let work = queued.pop_front().expect("checked queue");
                if work.key.scope() != lease.scope() { return Err(DriverError::Protocol); }
                let (tool, spec) = tools.get(&work.name).ok_or(DriverError::Protocol)?;
                if tool.spec() != *spec { return Err(DriverError::Protocol); }
                let admission = admit_work(&store, &lease, &services, &work)?;
                match admission {
                    Admission::New(claim) => {
                        // Recheck revocation after SQLite before any native effect can start.
                        if owner_cancel.is_cancelled() || link.is_cancelled() || Instant::now() >= deadline {
                            return Err(DriverError::Cancelled);
                        }
                        if close.as_ref().is_some_and(|c|c.requested.is_cancelled()) {return Err(DriverError::Busy);}
                        let scope = lease.scope().clone();
                        let call = claim.call_id().to_owned();
                        let progress = services.progress.clone();
                        let cancel = work_cancel.child_token();
                        let (host, bound_progress)=if let Some(factory)=&services.host_factory {
                            let bound = factory(lease.clone(),&claim,&work.name,cancel.clone(),deadline)?;
                            (bound.host, Some(bound.progress))
                        } else {(host.clone(), None)};
                        let ctx = ToolCtx {
                            cwd: PathBuf::from(&row.cwd), session: scope.session.clone(),
                            call_id: call.clone(), cancel: cancel.clone(),
                            progress: bound_progress.unwrap_or_else(|| Arc::new(move |text| progress(&scope, &call, text))),
                            env: super::run::tool_env(&row.id, claim.call_id(), Some(&model), row.thinking.as_deref()),
                            shell: services.shell.clone(), locks: services.locks.clone(),
                            processes: services.processes.clone(), host: Some(host.clone()),
                        };
                        active.push(Active { key: work.key.clone(), cancel: cancel.clone(), cancelled_at: None, sequential: tool.sequential(), bytes: work.bytes });
                        let tool = tool.clone();
                        workers.spawn(async move {
                            let result = tools::run(tool.as_ref(), work.args, &ctx).await;
                            Finished { key: work.key, claim, result, cancel }
                        });
                    }
                    Admission::Completed(value) => {
                        pending_bytes -= work.bytes;
                        let response = session.complete(&work.key, value).map_err(|_| DriverError::Protocol)?;
                        send_reply(&mut writer, response, &owner_cancel, &link, deadline).await?;
                    }
                    _ => return Err(DriverError::Protocol),
                }
            }
            let cancel_deadline = active.iter().filter_map(|a| a.cancelled_at.map(|t| t + CANCEL_GRACE)).min().unwrap_or(deadline);
            tokio::select! {
                biased;
                _ = owner_cancel.cancelled() => return Err(DriverError::Cancelled),
                _ = link.cancelled() => return Err(DriverError::Channel),
                _ = tokio::time::sleep_until(deadline.min(cancel_deadline)) => return Err(DriverError::Cancelled),
                requested = close::next(&mut close) => {
                    close_ack=Some(requested?);
                    if !session.is_quiescent() || !workers.is_empty() || !queued.is_empty() {return Err(DriverError::Busy);}
                    close::finish(&store,&lease,(&mut writer,&mut receive),&owner_cancel,&link,deadline).await?;
                    return Ok(());
                }
                finished = workers.join_next(), if !workers.is_empty() => {
                    let finished = finished.ok_or(DriverError::Worker)?.map_err(|_| DriverError::Worker)?;
                    let index = active.iter().position(|a| a.key == finished.key).ok_or(DriverError::Worker)?;
                    let completed = active.remove(index);
                    pending_bytes -= completed.bytes;
                    // Requested per-call cancellation may settle its actual native result.
                    // Unrequested cancellation (e.g. failed progress persistence) still fences.
                    if finished.cancel.is_cancelled() && completed.cancelled_at.is_none() { return Err(DriverError::Cancelled); }
                    let reply = settle_work(&store, &lease, &services, &finished.claim, &finished.result)?;
                    let response = session.complete(&finished.key, reply).map_err(|_| DriverError::Protocol)?;
                    send_reply(&mut writer, response, &owner_cancel, &link, deadline).await?;
                }
                frame = receive.recv() => {
                    let frame = frame.ok_or(DriverError::Channel)?;
                    if close::input_closed(&frame) {
                        // Only the authenticated helper can emit this reserved control.
                        // All delivered tool receipts must be settled before MCP shutdown.
                        // Keep the owner's close receiver alive until its terminal record:
                        // helper EOF alone must never complete or resume the model turn.
                        if close.is_none() || !session.is_quiescent() || !workers.is_empty() || !queued.is_empty() {
                            return Err(DriverError::Busy);
                        }
                        match receive.try_recv() {
                            Err(mpsc::error::TryRecvError::Empty) => {},
                            _ => return Err(DriverError::Busy),
                        }
                        close::commit_close(&store,&lease,&mut writer,&owner_cancel,&link,deadline).await?;
                        close_ack=Some(tokio::select! {biased;
                            _=owner_cancel.cancelled()=>return Err(DriverError::Cancelled),
                            _=tokio::time::sleep_until(deadline)=>return Err(DriverError::Cancelled),
                            requested=close::next(&mut close)=>requested?,
                        });
                        return Ok(());
                    }
                    match session.receive(&frame) {
                        Incoming::Response(value) => send_reply(&mut writer, value, &owner_cancel, &link, deadline).await?,
                        Incoming::Call { key, name, arguments } => {
                            if queued.len() >= protocol::MAX_PENDING { return Err(DriverError::Protocol); }
                            let bytes = frame.len();
                            if bytes > MAX_PENDING_BYTES.saturating_sub(pending_bytes) { return Err(DriverError::Protocol); }
                            pending_bytes += bytes;
                            queued.push_back(Work { key, name, args: arguments, bytes });
                        }
                        Incoming::Replay(key) => {
                            let value = store.cli_native_receipt(&lease, &key.request_id()).map_err(|_| DriverError::Storage)?;
                            let response = session.replay(&key, value).map_err(|_| DriverError::Protocol)?;
                            send_reply(&mut writer, response, &owner_cancel, &link, deadline).await?;
                        }
                        Incoming::Pending(_) | Incoming::Ignore => {},
                        Incoming::Cancel(key) => {
                            if let Some(index) = queued.iter().position(|w| w.key == key) {
                                let work = queued.remove(index).expect("located work");
                                let reply = match admit_work(&store, &lease, &services, &work)? {
                                    Admission::New(claim) => settle_work(&store, &lease, &services, &claim, &tools::aborted(&work.name, ""))?,
                                    Admission::Completed(reply) => reply,
                                    _ => return Err(DriverError::Protocol),
                                };
                                pending_bytes -= work.bytes;
                                let response = session.complete(&key, reply).map_err(|_| DriverError::Protocol)?;
                                send_reply(&mut writer, response, &owner_cancel, &link, deadline).await?;
                            }
                            else if let Some(work) = active.iter_mut().find(|w| w.key == key) {
                                work.cancelled_at.get_or_insert_with(Instant::now);
                                work.cancel.cancel();
                            } else {return Err(DriverError::Protocol);}
                        }
                        Incoming::Fence(_) => return Err(DriverError::Protocol),
                    }
                }
            }
        }
    }.await;
    // Stop effects/IO FIRST, before any potentially slow durable cleanup. Native tools get a
    // bounded cooperative cleanup period, then their futures are aborted, never replayed.
    work_cancel.cancel();
    writer.revoke();
    drop(guard);
    if timeout(Duration::from_secs(5), async {
        while workers.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
    let db = store.clone();
    let owner = lease.clone();
    let retired =
        tokio::task::spawn_blocking(move || db.fence_cli_tool_bridge(&owner, now_ms())).await;
    if !matches!(retired, Ok(Ok(()))) {
        result = Err(DriverError::Storage);
    }
    if result.is_ok() && (owner_cancel.is_cancelled() || Instant::now() >= deadline) {
        result = Err(DriverError::Cancelled);
    }
    if let Some(ack) = close_ack {
        let _ = ack.send(result.map(|()| Closed {
            scope: lease.scope().clone(),
        }));
    }
    result
}
fn admit_work(
    store: &Store,
    lease: &Lease,
    services: &Services,
    work: &Work,
) -> Result<Admission, DriverError> {
    if let Some(publication) = &services.publication {
        publication.commit(lease.scope(), || {
            store.admit_cli_native_call_published(
                lease,
                &work.key.request_id(),
                &work.name,
                &work.args,
                now_ms(),
            )
        })
    } else {
        store.admit_cli_native_call(
            lease,
            &work.key.request_id(),
            &work.name,
            &work.args,
            now_ms(),
        )
    }
    .map_err(|_| DriverError::Storage)
}
fn settle_work(
    store: &Store,
    lease: &Lease,
    services: &Services,
    claim: &Claim,
    result: &tools::ToolResult,
) -> Result<Value, DriverError> {
    let (state, reply) = if let Some(publication) = &services.publication {
        publication.commit(lease.scope(), || {
            if let Some(settle) = &services.settle {
                settle(lease, claim, result)
            } else {
                store.settle_cli_native_call_published(lease, claim, result, now_ms())
            }
        })
    } else {
        store.settle_cli_native_call(lease, claim, result, now_ms())
    }
    .map_err(|_| DriverError::Storage)?;
    if !matches!(state, Settlement::Committed | Settlement::AlreadyCommitted) {
        return Err(DriverError::Storage);
    }
    Ok(reply)
}
async fn send_reply<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut crate::execution::claude_cli::orchestration_wire::Writer<W>,
    value: Value,
    cancel: &CancellationToken,
    link: &CancellationToken,
    deadline: Instant,
) -> Result<(), DriverError> {
    let payload = serde_json::to_vec(&value).map_err(|_| DriverError::Protocol)?;
    tokio::select! {biased;_=cancel.cancelled()=>Err(DriverError::Cancelled),_=link.cancelled()=>Err(DriverError::Channel),
    result=writer.write(&payload,deadline)=>result.map_err(|_|DriverError::Channel)}
}

#[cfg(test)]
mod tests;

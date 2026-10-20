//! Owned native CLI root coordinator. Not exposed by RPC/setup/model selection yet.
//! Caller owes independent orchestration approval; Coding grants are never accepted here.
//! One durable launch CAS, one process, one bridge lifetime; no retry or API fallback.
#![allow(dead_code)] // Staged owner route; existing public CLI hosting stays Coding-only.
use super::{
    cli_bridge_channel::{self, Server},
    cli_bridge_driver::{self, Services},
};
use crate::{
    execution::claude_cli::{
        orchestration_observer::{Observer, Outcome},
        orchestration_policy::Assets,
        process,
    },
    store::{
        Store,
        cli_agent_turns::{
            self, Lease,
            projection::{Projection, Update},
        },
    },
    tools::Tool,
    util::now_ms,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{Instant, timeout_at},
};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Preparation,
    Storage,
    Channel,
    Process,
    Projection,
    Publication,
    Cancelled,
}
/// Publication is bounded and scoped. A downstream runtime must publish/reconcile committed
/// revisions rather than route by selected conversation. It must continuously drain this lane.
pub(crate) struct Launch {
    pub ownership: super::cli_agent_owner::Turn,
    pub root: Lease,
    pub server: Server,
    pub assets: Assets,
    pub tools: Vec<Arc<dyn Tool>>,
    pub services: Services,
    pub cancel: CancellationToken,
    pub publication: mpsc::Sender<Update>,
}
struct Lifetime {
    process: CancellationToken,
    bridge: CancellationToken,
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.process.cancel();
        self.bridge.cancel();
    }
}
fn committed<T>(
    publisher: &Option<Arc<cli_bridge_driver::Publication>>,
    scope: &crate::execution::claude_cli::orchestration_protocol::Scope,
    f: impl FnOnce() -> Result<(Update, T), Error>,
) -> Result<(Option<Update>, T), Error> {
    if let Some(publisher) = publisher {
        let value = publisher
            .commit(scope, || {
                let (update, value) =
                    f().map_err(|_| crate::store::StoreError::ExecutionConflict)?;
                Ok((value, Some(update)))
            })
            .map_err(|_| Error::Publication)?;
        Ok((None, value))
    } else {
        let (update, value) = f()?;
        Ok((Some(update), value))
    }
}
struct Observed {
    decoder: Option<Observer>,
    projection: Projection,
    running: bool,
}
/// No reconstruction from receipts: consumes both the original root lease and prepared listener.
/// Any refusal after intent is conservatively Unknown. Drops revoke memory/process work; normal
/// exits also await native cleanup before retiring SQL. Restart owns interrupted durable cleanup.
pub(crate) async fn run(
    store: Arc<Store>,
    launch: Launch,
) -> Result<cli_agent_turns::Receipt, Error> {
    let Launch {
        ownership,
        root,
        server,
        assets,
        tools,
        services,
        cancel,
        publication,
    } = launch;
    if !ownership.matches(&root) {
        return Err(Error::Preparation);
    }
    let root = Arc::new(root);
    let identity = server.identity().clone();
    let deadline = server.deadline();
    let lifetime = Lifetime {
        process: ownership.cancellation().child_token(),
        bridge: CancellationToken::new(),
    };
    let shutdown = ownership.shutdown_token();
    let publisher = services.publication.clone();
    let result = {
        let result = async {
            if lifetime.process.is_cancelled() || Instant::now() >= deadline {
                return Err(Error::Cancelled);
            }
            if let Some(policy) = ownership.child_policy() {
                if !assets.matches_system(policy.system()) {
                    return Err(Error::Preparation);
                }
                ownership
                    .validate_inputs(
                        policy.system(),
                        &tools.iter().map(|t| t.spec()).collect::<Vec<_>>(),
                    )
                    .map_err(|_| Error::Preparation)?;
            }
            let check = ownership.prepare();
            tokio::pin!(check);
            let prepared = tokio::select! { biased;
                _ = tokio::time::sleep_until(deadline) => {
                    ownership.cancellation().cancel();
                    let _ = check.await;
                    return Err(Error::Cancelled);
                },
                prepared = &mut check => prepared.map_err(|_| Error::Preparation)?,
            };
            let db = store.clone();
            let lease = root.clone();
            let context = tokio::task::spawn_blocking(move || db.cli_agent_launch_context(&lease))
                .await
                .map_err(|_| Error::Storage)?
                .map_err(|_| Error::Preparation)?;
            if &identity.scope != context.scope() {
                return Err(Error::Preparation);
            }
            let specs = tools.iter().map(|t| t.spec()).collect::<Vec<_>>();
            let observer = Observer::new(
                &context,
                &specs,
                server.catalog().ok_or(Error::Preparation)?,
            )
            .map_err(|_| Error::Preparation)?;
            prepared
                .launch_admitted_orchestration_spec(&context, &assets)
                .map_err(|_| Error::Preparation)?;
            let db = store.clone();
            let lease = root.clone();
            let bridge = identity.id.clone();
            tokio::task::spawn_blocking(move || {
                db.begin_cli_agent_launch(&lease, &bridge, now_ms())
            })
            .await
            .map_err(|_| Error::Storage)?
            .map_err(|_| Error::Storage)?;
            // Revalidate executable/config/assets/freshness AFTER the durable intent. Failed
            // validation cannot turn that intent into permission to repeat a spawn.
            let mut spec = prepared
                .launch_admitted_orchestration_spec(&context, &assets)
                .map_err(|_| Error::Preparation)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            spec.timeout = Some(
                spec.timeout
                    .map_or(remaining, |timeout| timeout.min(remaining)),
            );
            let (close, request) = cli_bridge_driver::close_channel(context.scope().clone());
            let close = Arc::new(tokio::sync::Mutex::new(close));
            let observed = Arc::new(Mutex::new(Observed {
                decoder: Some(observer),
                projection: Projection::new(root.clone()),
                running: false,
            }));
            let scope = context.scope().clone();
            let observer_scope = scope.clone();
            let state = observed.clone();
            let db = store.clone();
            let lease = root.clone();
            let output = publication.clone();
            let closing = close.clone();
            let observer_publisher = publisher.clone();
            let observer: process::AsyncStdoutObserver = Box::new(move |bytes| {
                let state = state.clone();
                let db = db.clone();
                let lease = lease.clone();
                let scope = observer_scope.clone();
                let output = output.clone();
                let closing = closing.clone();
                let publisher = observer_publisher.clone();
                Box::pin(async move {
                    let closing_scope = scope.clone();
                    let (update,terminal)=tokio::task::spawn_blocking(move||committed(&publisher,&scope,||{
                        let mut state=state.lock().map_err(|_|Error::Projection)?;
                        if !state.running {db.mark_cli_agent_running(&lease,now_ms()).map_err(|_|Error::Storage)?;state.running=true;}
                        let mut events=Vec::new();let decoder=state.decoder.as_mut().ok_or(Error::Projection)?;
                        decoder.feed(&scope,&bytes,|_,event|{events.push(event);Ok::<_,()>(())}).map_err(|_|Error::Projection)?;
                        let terminal=events.iter().any(|e|matches!(e,crate::execution::claude_cli::orchestration_observer::Observation::Terminal {..}));
                        let update=state.projection.apply(&db,&scope,events,now_ms()).map_err(|_|Error::Storage)?;
                        Ok((update,terminal))
                    })).await.map_err(|_|"projection unavailable")?.map_err(|_|"projection refused")?;
                    let scope = closing_scope;
                    if let Some(update) = update {
                        output
                            .send(update)
                            .await
                            .map_err(|_| "publication unavailable")?;
                    }
                    if terminal {
                        closing
                            .lock()
                            .await
                            .finish(&scope)
                            .await
                            .map_err(|_| "native close refused")?;
                    }
                    Ok(())
                })
            });
            let revoke = lifetime.bridge.clone();
            let before_signal: process::BeforeSignal = Box::new(move || {
                revoke.cancel();
                true
            });
            let process = process::run_observed_async(
                spec,
                lifetime.process.clone(),
                Some(before_signal),
                observer,
            );
            let bridge = async {
                let connected = tokio::select! {biased;_=lifetime.bridge.cancelled()=>return Err(Error::Cancelled),result=server.accept()=>result.map_err(|_|Error::Channel)?};
                cli_bridge_driver::serve_controlled(
                    store.clone(),
                    connected,
                    tools,
                    services,
                    lifetime.bridge.clone(),
                    deadline,
                    request,
                )
                .await
                .map_err(|error| {
                    #[cfg(test)]
                    eprintln!("Native tool driver refused: {error:?}");
                    let _ = error;
                    Error::Channel
                })
            };
            tokio::pin!(process);
            tokio::pin!(bridge);
            let (report, bridge_ok) = tokio::select! {biased;
                result=&mut bridge=>{
                    if result.is_err(){lifetime.process.cancel();}
                    (process.await,result.is_ok())
                },
                report=&mut process=>{
                    lifetime.bridge.cancel();
                    let ok=timeout_at(Instant::now()+Duration::from_secs(6),&mut bridge).await.is_ok_and(|r|r.is_ok());
                    (report,ok)
                }
            };
            let report = report.map_err(|_| Error::Process)?;
            if !report.clean_exit()
                || !bridge_ok
                || lifetime.process.is_cancelled()
                || Instant::now() >= deadline
            {
                #[cfg(test)]
                {
                    let state = observed.lock().unwrap();
                    let decoder = state.decoder.as_ref();
                    eprintln!(
                        "Native CLI lifecycle refused: {report:?}; bridge_ok={bridge_ok}; diagnostic={:?}; parse={:?}; record={:?}",
                        decoder.and_then(Observer::diagnostic),
                        decoder.and_then(Observer::parse_failure),
                        decoder.and_then(Observer::failed_record),
                    );
                }
                return Err(Error::Process);
            }
            let state = observed.clone();
            let db = store.clone();
            let finishing_publisher = publisher.clone();
            let finishing_scope = scope.clone();
            let (update, (terminal, incomplete, diagnostic)) =
                tokio::task::spawn_blocking(move || {
                    committed(&finishing_publisher, &finishing_scope, || {
                        let mut state = state.lock().map_err(|_| Error::Projection)?;
                        let decoder = state.decoder.take().ok_or(Error::Projection)?;
                        let mut tail = Vec::new();
                        let diagnostic = decoder.diagnostic();
                        let summary = decoder
                            .finish(|_, event| {
                                tail.push(event);
                                Ok::<_, ()>(())
                            })
                            .map_err(|_| Error::Projection)?;
                        // A terminal must already have closed the bridge while the root was supervised.
                        // An unterminated final line cannot retroactively produce that close proof.
                        if !tail.is_empty() || summary.outcome != Some(Outcome::Success) {
                            return Err(Error::Projection);
                        }
                        let terminal = state.projection.terminal().ok_or(Error::Projection)?;
                        let update = state
                            .projection
                            .finish(&db, now_ms(), summary.presentation_incomplete)
                            .map_err(|_| Error::Storage)?;
                        Ok((
                            update,
                            (terminal, summary.presentation_incomplete, diagnostic),
                        ))
                    })
                })
                .await
                .map_err(|_| Error::Projection)??;
            if let Some(update) = update {
                tokio::select! {biased;_=lifetime.process.cancelled()=>return Err(Error::Cancelled),_=tokio::time::sleep_until(deadline)=>return Err(Error::Cancelled),result=publication.send(update)=>result.map_err(|_|Error::Publication)?};
            }
            if lifetime.process.is_cancelled() || Instant::now() >= deadline {
                return Err(Error::Cancelled);
            }
            let evidence = cli_agent_turns::Completion {
                cli_session_id: context.binding().create.identity.cli_session_id.clone(),
                terminal,
                supervisor: crate::store::ExternalSettlementEvidence {
                    projection_finished: true,
                    input_delivered: report.stdin_delivered,
                    root_reaped: report.root_reaped,
                    pipes_drained: report.pipes_drained,
                    exit_code: report.exit_code,
                    cleanup_timed_out: report.cleanup_timed_out,
                    presentation_incomplete: incomplete,
                    stream_diagnostic: diagnostic,
                    ..Default::default()
                },
            };
            let db = store.clone();
            let lease = root.clone();
            let cancellation = lifetime.process.clone();
            tokio::task::spawn_blocking(move || {
                if cancellation.is_cancelled() || Instant::now() >= deadline {
                    return Err(crate::store::StoreError::ExecutionConflict);
                }
                db.complete_cli_agent_turn(&lease, &evidence, now_ms())
            })
            .await
            .map_err(|_| Error::Storage)?
            .map_err(|_| Error::Storage)
            .and_then(|s| {
                if s == cli_agent_turns::Settlement::Committed {
                    Ok(())
                } else {
                    Err(Error::Storage)
                }
            })?;
            store
                .cli_agent_turn(&scope.session, &root.receipt().request)
                .map_err(|_| Error::Storage)?
                .ok_or(Error::Storage)
        };
        tokio::pin!(result);
        tokio::select! { biased;
            _ = cancel.cancelled() => { ownership.cancellation().cancel(); result.await },
            _ = shutdown.cancelled() => { ownership.cancellation().cancel(); result.await },
            result = &mut result => result,
        }
    };
    // Assets stay alive through process/group reaping and native cleanup, including errors.
    lifetime.process.cancel();
    lifetime.bridge.cancel();
    if result.is_err() {
        let _ = cli_bridge_channel::retire(store.clone(), identity).await;
        let db = store.clone();
        let lease = root.clone();
        tokio::task::spawn_blocking(move || db.abandon_cli_agent_turn(&lease, now_ms()))
            .await
            .map_err(|_| Error::Storage)?
            .map_err(|_| Error::Storage)?;
    }
    drop(assets);
    result
}
#[cfg(all(test, target_os = "linux"))]
mod tests;

//! Owner-only quiescence handshake. A closed tool channel is NOT a completed model run.
use super::*;
use crate::execution::claude_cli::{orchestration_control::Control, orchestration_wire::Writer};
use tokio::{io::AsyncWrite, sync::oneshot};
type Ack = oneshot::Sender<Result<Closed, DriverError>>;
pub struct CloseHandle {
    scope: Scope,
    sender: Option<oneshot::Sender<Ack>>,
    requested: CancellationToken,
}
pub struct CloseRequest {
    pub(super) scope: Scope,
    pub(super) receiver: oneshot::Receiver<Ack>,
    pub(super) requested: CancellationToken,
}
#[derive(Debug)]
pub struct Closed {
    pub(super) scope: Scope,
}
impl Closed {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
}
/// Create on the native owner path only. Nothing here is serializable or exposed over MCP.
pub fn close_channel(scope: Scope) -> (CloseHandle, CloseRequest) {
    let (sender, receiver) = oneshot::channel();
    let requested = CancellationToken::new();
    (
        CloseHandle {
            scope: scope.clone(),
            sender: Some(sender),
            requested: requested.clone(),
        },
        CloseRequest {
            scope,
            receiver,
            requested,
        },
    )
}
impl Drop for CloseHandle {
    fn drop(&mut self) {
        // Revoke dispatch synchronously, before the driver next polls the oneshot.
        // Drop performs no storage IO and cannot manufacture a clean-close acknowledgment.
        self.requested.cancel();
    }
}
impl CloseHandle {
    /// A stale scope does not consume/revoke the legitimate owner's close capability.
    /// After sending, cancellation/lost acknowledgement is uncertain, never a retry grant.
    pub async fn finish(&mut self, scope: &Scope) -> Result<Closed, DriverError> {
        if *scope != self.scope {
            return Err(DriverError::Protocol);
        }
        let sender = self.sender.take().ok_or(DriverError::Protocol)?;
        self.requested.cancel();
        let (send, receive) = oneshot::channel();
        sender.send(send).map_err(|_| DriverError::Channel)?;
        receive.await.map_err(|_| DriverError::Channel)?
    }
}
pub(super) async fn next(request: &mut Option<CloseRequest>) -> Result<Ack, DriverError> {
    match request {
        Some(request) => (&mut request.receiver)
            .await
            .map_err(|_| DriverError::Channel),
        None => std::future::pending().await,
    }
}
pub(super) async fn finish<W: AsyncWrite + Unpin>(
    store: &Arc<Store>,
    lease: &Arc<Lease>,
    io: (&mut Writer<W>, &mut mpsc::Receiver<Vec<u8>>),
    cancel: &CancellationToken,
    link: &CancellationToken,
    deadline: Instant,
) -> Result<(), DriverError> {
    let deadline = deadline.min(Instant::now() + Duration::from_secs(10));
    let (writer, receive) = io;
    match receive.try_recv() {
        Err(mpsc::error::TryRecvError::Empty) => {}
        Ok(frame) if input_closed(&frame) => {
            return commit_close(store, lease, writer, cancel, link, deadline).await;
        }
        _ => return Err(DriverError::Busy),
    }
    send_reply(writer, Control::Quiesce.value(), cancel, link, deadline).await?;
    let frame = tokio::select! {biased;
        _=cancel.cancelled()=>return Err(DriverError::Cancelled),
        _=link.cancelled()=>return Err(DriverError::Channel),
        _=tokio::time::sleep_until(deadline)=>return Err(DriverError::Cancelled),
        frame=receive.recv()=>frame.ok_or(DriverError::Channel)?,
    };
    let value: Value = serde_json::from_slice(&frame).map_err(|_| DriverError::Protocol)?;
    if !Control::Quiesced.matches(&value) && !Control::InputClosed.matches(&value) {
        return Err(DriverError::Protocol);
    }
    commit_close(store, lease, writer, cancel, link, deadline).await
}
pub(super) fn input_closed(frame: &[u8]) -> bool {
    serde_json::from_slice::<Value>(frame).is_ok_and(|v| Control::InputClosed.matches(&v))
}
pub(super) async fn commit_close<W: AsyncWrite + Unpin>(
    store: &Arc<Store>,
    lease: &Arc<Lease>,
    writer: &mut Writer<W>,
    cancel: &CancellationToken,
    link: &CancellationToken,
    deadline: Instant,
) -> Result<(), DriverError> {
    // The helper has stopped its MCP input loop. No later requests can reach admission.
    let db = store.clone();
    let owner = lease.clone();
    let committed = tokio::task::spawn_blocking(move || db.close_cli_tool_bridge(&owner, now_ms()));
    let closed = tokio::select! {biased;
        _=cancel.cancelled()=>return Err(DriverError::Cancelled),
        _=link.cancelled()=>return Err(DriverError::Channel),
        _=tokio::time::sleep_until(deadline)=>return Err(DriverError::Cancelled),
        result=committed=>result.map_err(|_|DriverError::Storage)?.map_err(|_|DriverError::Storage)?,
    };
    if !closed {
        return Err(DriverError::Storage);
    }
    // The durable close precedes the final frame. Lost delivery still cannot imply root success.
    send_reply(writer, Control::Closed.value(), cancel, link, deadline).await
}

//! Narrow private command-hook IPC, not launched by the chat-only actor.
//! The socket only submits requests; owner-scoped RPC decisions delegate to Control.
//! No tool execution, administration commands or retries on this socket.
#![cfg(unix)]
#![allow(dead_code)] // Qualification observer APIs and platform-gated paths.

use super::external_permissions::{Broker, Error, Review};
use crate::execution::claude_cli::{
    permission::{Decision, Ticket},
    permission_helper::{self, HelperLaunch},
    permission_wire::{self, ReplyFrame, RequestFrame},
};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::mpsc,
    task::JoinSet,
    time::{Instant, timeout_at},
};
use tokio_util::{
    sync::CancellationToken,
    task::{TaskTracker, task_tracker::TaskTrackerToken},
};

const CONNECTIONS: usize = 16;
const TOTAL_CONNECTIONS: usize = 128;
const HANDSHAKE: Duration = Duration::from_secs(2);

struct Slot {
    stream: Arc<UnixStream>,
    allow: Vec<u8>,
    deny: Vec<u8>,
    done: CancellationToken,
}
fn packet(frame: &RequestFrame, decision: permission_wire::Decision) -> Result<Vec<u8>, Error> {
    let reply = ReplyFrame::for_request(frame, decision).map_err(|_| Error::Request)?;
    let bytes = serde_json::to_vec(&reply).map_err(|_| Error::Request)?;
    if bytes.len() > 1024 {
        return Err(Error::Request);
    }
    let mut packet = Vec::with_capacity(bytes.len() + 4);
    packet.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    packet.extend_from_slice(&bytes);
    Ok(packet)
}

pub(super) struct Control {
    broker: Arc<Broker>,
    slots: Mutex<BTreeMap<[u8; 32], Arc<Slot>>>,
}
impl Control {
    #[cfg(test)]
    pub fn cancel_after_commit_for_test(&self) {
        self.broker.cancel_after_commit_for_test();
    }
    pub fn scope(&self) -> &crate::execution::claude_cli::permission::RunScope {
        self.broker.scope()
    }
    pub fn prompt(&self) -> u64 {
        self.broker.prompt()
    }
    pub fn cancel(&self) {
        self.broker.cancel();
    }
    pub fn live(&self) -> bool {
        self.broker.permits_launch()
    }
    pub fn reviews(&self, owner: u64) -> Result<Vec<Review>, Error> {
        // Never hold transport and broker locks together.
        let nonces: Vec<_> = self
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect();
        Ok(self
            .broker
            .reviews(owner)?
            .into_iter()
            .filter(|r| nonces.contains(&r.ticket.nonce))
            .collect())
    }
    pub fn lookup(
        &self,
        owner: u64,
        ticket: &Ticket,
    ) -> Result<Option<crate::store::ExternalPermissionReceipt>, Error> {
        self.broker.lookup(owner, ticket)
    }
    pub fn decide(&self, owner: u64, ticket: &Ticket, decision: Decision) -> Result<(), Error> {
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ticket.nonce)
            .cloned()
            .ok_or(Error::Decision)?;
        self.broker.decide(owner, ticket, decision, |bytes| {
            let (expected, packet) = match decision {
                Decision::AllowOnce => (permission_wire::Decision::AllowOnce, &slot.allow),
                Decision::Deny => (permission_wire::Decision::Deny, &slot.deny),
            };
            if bytes != permission_helper::decision_json(expected) {
                return Err(());
            }
            // One bounded nonblocking syscall while the broker's fence is held.
            // Partial/WouldBlock/lost delivery never resumes or reissues an allow.
            // Retire the pending-disconnect watcher BEFORE bytes can escape: a
            // successful peer may read and close before this callback returns.
            // Delivery failure is still fenced by Broker::decide, never retried.
            slot.done.cancel();
            match slot.stream.try_write(packet) {
                Ok(n) if n == packet.len() => Ok(()),
                _ => Err(()),
            }
        })?;
        slot.done.cancel();
        Ok(())
    }
    pub fn fence(&self) -> bool {
        self.broker.fence()
    }
}

struct BindingAttempt(Option<Arc<Broker>>);
impl Drop for BindingAttempt {
    fn drop(&mut self) {
        if let Some(broker) = &self.0 {
            broker.fence();
        }
    }
}

pub(super) struct Server {
    listener: UnixListener,
    control: Arc<Control>,
    tracker: TaskTracker,
    // Only our newly-created private directory is removed, never a caller directory.
    _directory: tempfile::TempDir,
    _completion: TaskTrackerToken,
}
impl Server {
    /// Native engine background/setup work only: this performs filesystem I/O.
    /// If offloaded, the actor must acquire/retain a tracker token BEFORE offloading
    /// the bind itself, and recheck its lease before any CLI spawn after the result.
    /// Parent must be an existing trusted directory outside the captured project.
    /// Both ancestry and peer UID are checked; this is not same-UID containment.
    pub fn bind(
        parent: &Path,
        broker: Arc<Broker>,
    ) -> Result<(Self, Arc<Control>, HelperLaunch), Error> {
        broker.claim_endpoint()?;
        let mut attempt = BindingAttempt(Some(broker.clone()));
        let tracker = broker.tracker()?;
        tokio::runtime::Handle::try_current().map_err(|_| Error::Unavailable)?;
        let completion = tracker.token(); // registration before any asynchronous launch
        let parent = parent.canonicalize().map_err(|_| Error::Unavailable)?;
        let project = broker
            .project()
            .canonicalize()
            .map_err(|_| Error::Unavailable)?;
        if project != broker.project() || parent.starts_with(&project) {
            return Err(Error::Scope);
        }
        let directory = tempfile::Builder::new()
            .prefix(".ph")
            .tempdir_in(parent)
            .map_err(|_| Error::Unavailable)?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Unavailable)?;
        let path = directory.path().join("s");
        let listener =
            std::os::unix::net::UnixListener::bind(&path).map_err(|_| Error::Unavailable)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::Unavailable)?;
        permission_helper::check_endpoint(&path).map_err(|_| Error::Unavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| Error::Unavailable)?;
        let listener = UnixListener::from_std(listener).map_err(|_| Error::Unavailable)?;
        let launch =
            HelperLaunch::new(&path, broker.run_nonce()).map_err(|_| Error::Unavailable)?;
        let control = Arc::new(Control {
            broker,
            slots: Mutex::new(BTreeMap::new()),
        });
        attempt.0 = None;
        Ok((
            Self {
                listener,
                control: control.clone(),
                tracker,
                _directory: directory,
                _completion: completion,
            },
            control,
            launch,
        ))
    }

    /// Caller owns this future through cleanup; dropping even an unpolled server fences.
    /// Each connection also retains the engine's tracker through its own cleanup/drop.
    #[cfg(test)]
    pub async fn run(self, reviews: mpsc::Sender<Review>) {
        self.run_optional(Some(reviews)).await;
    }

    /// None is deliberate pull-only delivery through the registered Control. No
    /// background receiver, event forwarding, or extra authority lifetime is needed.
    pub async fn run_optional(self, reviews: Option<mpsc::Sender<Review>>) {
        let mut tasks = JoinSet::new();
        let mut total = 0;
        loop {
            tokio::select! {
                biased;
                _ = self.control.broker.retired() => break,
                result = tasks.join_next(), if !tasks.is_empty() => {
                    if !matches!(result, Some(Ok(Ok(())))) { break; }
                },
                accepted = self.listener.accept() => {
                    let Ok((stream, _)) = accepted else { break; };
                    total += 1;
                    if total > TOTAL_CONNECTIONS || tasks.len() >= CONNECTIONS { break; }
                    let control = self.control.clone();
                    let reviews = reviews.clone();
                    // The token is acquired before spawn, including unpolled/aborted work.
                    let token = self.tracker.token();
                    tasks.spawn(async move {
                        let _completion = token;
                        connection(control, stream, reviews).await
                    });
                }
            }
        }
        self.control.fence();
        tasks.shutdown().await;
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.control.fence();
    }
}

struct PendingConnection {
    control: Arc<Control>,
    nonce: [u8; 32],
    slot: Arc<Slot>,
    completed: bool,
}
impl Drop for PendingConnection {
    fn drop(&mut self) {
        {
            let mut slots = self.control.slots.lock().unwrap_or_else(|e| e.into_inner());
            if slots
                .get(&self.nonce)
                .is_some_and(|s| Arc::ptr_eq(s, &self.slot))
            {
                slots.remove(&self.nonce);
            }
        }
        if !self.completed && !self.slot.done.is_cancelled() {
            self.control.fence();
        }
    }
}

async fn connection(
    control: Arc<Control>,
    mut stream: UnixStream,
    reviews: Option<mpsc::Sender<Review>>,
) -> Result<(), Error> {
    // SAFETY: geteuid has no preconditions.
    if stream.peer_cred().map_err(|_| Error::Request)?.uid() != unsafe { libc::geteuid() } {
        return Ok(());
    }
    let deadline = Instant::now() + HANDSHAKE;
    // Unauthenticated malformed/idle/cross-run peers gain nothing and are closed.
    let Ok(frame) = permission_wire::read_request(&mut stream, deadline).await else {
        return Ok(());
    };
    if frame.run_nonce != control.broker.run_nonce() {
        return Ok(());
    }
    timeout_at(deadline, stream.writable())
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
    if Instant::now() >= deadline {
        return Err(Error::Unavailable);
    }
    let allow = packet(&frame, permission_wire::Decision::AllowOnce)?;
    let deny = packet(&frame, permission_wire::Decision::Deny)?;
    let review = control.broker.admit(&frame)?;
    let expires = review.expires;
    let nonce = review.ticket.nonce;
    let slot = Arc::new(Slot {
        stream: Arc::new(stream),
        allow,
        deny,
        done: CancellationToken::new(),
    });
    let mut pending = PendingConnection {
        control: control.clone(),
        nonce,
        slot: slot.clone(),
        completed: false,
    };
    {
        let mut slots = control.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.contains_key(&nonce) {
            return Err(Error::Request);
        }
        slots.insert(nonce, slot.clone());
    }
    if let Some(reviews) = reviews {
        reviews.try_send(review).map_err(|_| Error::Unavailable)?;
    }
    loop {
        tokio::select! {
            biased;
            _ = slot.done.cancelled() => { pending.completed = true; return Ok(()); },
            _ = control.broker.retired() => return Err(Error::Unavailable),
            _ = tokio::time::sleep_until(expires.into()) => return Err(Error::Unavailable),
            ready = slot.stream.readable() => {
                ready.map_err(|_| Error::Unavailable)?;
                let mut extra = [0];
                match slot.stream.try_read(&mut extra) {
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                    // EOF, extra frames/data, or read failure retire pending authority.
                    _ if slot.done.is_cancelled() => { pending.completed = true; return Ok(()); },
                    _ => return Err(Error::Request),
                }
            }
        }
    }
}

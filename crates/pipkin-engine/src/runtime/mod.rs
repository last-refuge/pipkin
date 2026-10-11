//! The engine runtime: one tokio runtime, the store, the catalog/auth layer, per-session
//! execution, and client connections.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use tokio::sync::mpsc;

use crate::auth::Auth;
use crate::config::EngineConfig;
use crate::host::Connection;
use crate::store::Store;

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shared engine state.
pub struct Shared {
    pub config: EngineConfig,
    pub store: Arc<Store>,
    pub auth: Arc<Auth>,
    pub http: reqwest::Client,
    pub epoch: u64,
    seq: AtomicU64,
    clients: Mutex<HashMap<u64, std::sync::mpsc::Sender<ServerFrame>>>,
}

impl Shared {
    /// Publish a committed event to every connected client.
    pub fn publish(&self, event: Event) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let frame = ServerFrame::Event {
            epoch: self.epoch,
            seq,
            event,
        };
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.retain(|_, tx| tx.send(frame.clone()).is_ok());
    }

    /// Send to one client only (volatile, client-scoped events like sign-in state).
    pub fn publish_to(&self, client: u64, event: Event) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let frame = ServerFrame::Event {
            epoch: self.epoch,
            seq,
            event,
        };
        if let Some(tx) = self
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&client)
        {
            let _ = tx.send(frame);
        }
    }
}

pub struct EngineHandle {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    shared: Arc<Shared>,
    next_client: AtomicU64,
}

impl EngineHandle {
    pub fn start(config: EngineConfig) -> Result<EngineHandle, String> {
        let store = Store::open(&config.data_dir).map_err(|e| e.to_string())?;
        let epoch = store.next_epoch().map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .max_blocking_threads(32)
            .thread_name("pipkin-engine")
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the engine runtime: {e}"))?;
        let auth = Arc::new(Auth::new(&config));
        let shared = Arc::new(Shared {
            config,
            store: Arc::new(store),
            auth,
            http: crate::provider::http::client(),
            epoch,
            seq: AtomicU64::new(0),
            clients: Mutex::new(HashMap::new()),
        });
        Ok(EngineHandle {
            runtime: Mutex::new(Some(runtime)),
            shared,
            next_client: AtomicU64::new(1),
        })
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// A new client connection.
    pub fn connect(&self, _name: &str) -> Connection {
        let client = self.next_client.fetch_add(1, Ordering::SeqCst);
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<ClientFrame>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<ServerFrame>();
        self.shared
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(client, out_tx.clone());
        let shared = self.shared.clone();
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            rt.spawn(async move {
                while let Some(frame) = in_rx.recv().await {
                    let shared = shared.clone();
                    let out = out_tx.clone();
                    tokio::spawn(async move {
                        let result = dispatch(&shared, client, frame.call).await;
                        let _ = out.send(ServerFrame::Reply {
                            id: frame.id,
                            result,
                        });
                    });
                }
                shared
                    .clients
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&client);
                shared.auth.client_gone(client);
            });
        }
        Connection {
            tx: in_tx,
            rx: out_rx,
            client,
        }
    }

    /// Orderly stop: settle what can be settled, then stop the runtime within `timeout`.
    pub fn shutdown(&self, timeout: Duration) {
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            rt.shutdown_timeout(timeout);
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(5));
    }
}

async fn dispatch(shared: &Arc<Shared>, _client: u64, call: Call) -> Result<Reply, ProtoError> {
    match call {
        Call::Hello { protocol, .. } => {
            if protocol != PROTOCOL_VERSION {
                return Err(ProtoError::new(
                    ErrorCode::Incompatible,
                    format!("the engine speaks protocol {PROTOCOL_VERSION}, the client {protocol}"),
                ));
            }
            Ok(Reply::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                engine_version: ENGINE_VERSION.into(),
                engine_revision: option_env!("PIPKIN_REVISION").unwrap_or("unknown").into(),
                schema: crate::store::SCHEMA_VERSION,
                epoch: shared.epoch,
                capabilities: vec![],
            }))
        }
        _ => Err(ProtoError::new(
            ErrorCode::Unsupported,
            "not implemented yet in this engine build",
        )),
    }
}

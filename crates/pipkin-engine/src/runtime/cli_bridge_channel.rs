//! Inactive host-side authenticated channel + durable one-use connection admission.
//! No listener is started by normal runtime startup, and no native tool is dispatched here.
//! Caller must supply independently approved binding/scope, supervise cancellation/cleanup,
//! and join native tool records before executing effects. Never reconstruct this from receipts.
use crate::{
    execution::claude_cli::orchestration_wire::{self as wire, Reader, Rejected, Secret, Writer},
    store::{
        Store,
        cli_bridge::{BridgeSpec, Lease, ORCHESTRATION_POLICY},
    },
    util::now_ms,
};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{ReadHalf, WriteHalf},
    net::{TcpListener, TcpStream},
    time::{Instant, timeout_at},
};

const MAX_BAD_PEERS: usize = 8;
const MAX_LIFETIME: Duration = Duration::from_secs(30 * 60);
fn scope_digest(spec: &BridgeSpec) -> [u8; 32] {
    Sha256::digest(
        serde_json::to_vec(&serde_json::json!([
            ORCHESTRATION_POLICY,
            spec.id,
            spec.scope.session,
            spec.scope.generation,
            spec.scope.operation,
            spec.scope.run,
            spec.owner,
            spec.binding_hash,
            spec.capability_hash
        ]))
        .expect("fixed JSON identity"),
    )
    .into()
}
/// Sensitive helper-only launch data; no shell string or credential/configuration access.
#[derive(Clone)]
pub struct Launch {
    endpoint: SocketAddrV4,
    secret: Secret,
    scope: [u8; 32],
}
impl fmt::Debug for Launch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Launch([redacted])")
    }
}
impl Launch {
    /// Sensitive: hand directly to the dedicated helper environment, never logs or argv.
    pub fn environment_value(&self) -> String {
        let scope: String = self
            .scope
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        serde_json::json!({"endpoint": self.endpoint.to_string(), "secret": self.secret.to_hex(), "scope": scope}).to_string()
    }
    /// Closed helper configuration. No hostname resolution, arbitrary address, file or URL.
    pub fn from_environment_value(value: &str) -> Result<Self, Rejected> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            endpoint: String,
            secret: String,
            scope: String,
        }
        if value.len() > 1024 {
            return Err(Rejected);
        }
        let config: Config = serde_json::from_str(value).map_err(|_| Rejected)?;
        let endpoint: SocketAddrV4 = config.endpoint.parse().map_err(|_| Rejected)?;
        if *endpoint.ip() != Ipv4Addr::LOCALHOST || endpoint.port() == 0 {
            return Err(Rejected);
        }
        let scope =
            *crate::execution::claude_cli::permission_wire::Binding::from_hex(&config.scope)
                .map_err(|_| Rejected)?
                .as_bytes();
        Ok(Self {
            endpoint,
            secret: Secret::from_hex(&config.secret)?,
            scope,
        })
    }
    /// Binary helper transport, not a Claude invocation. No reconnect on any failure.
    pub async fn connect(&self, deadline: Instant) -> Result<wire::Channel<TcpStream>, Rejected> {
        if Instant::now() >= deadline {
            return Err(Rejected);
        }
        let stream = timeout_at(deadline, TcpStream::connect(self.endpoint))
            .await
            .map_err(|_| Rejected)?
            .map_err(|_| Rejected)?;
        wire::connect(stream, self.secret.clone(), self.scope, deadline).await
    }
}
/// Private native launch capability tied to the committed binding, not ambient configuration.
/// Only a native-prepared Server can mint it. It authorizes neither a prompt nor a retry.
pub struct NativeLaunch {
    launch: Launch,
    scope: crate::execution::claude_cli::orchestration_protocol::Scope,
    binding: [u8; 32],
    deadline: Instant,
}
impl NativeLaunch {
    pub fn environment_value(&self) -> String {
        self.launch.environment_value()
    }
    pub fn matches(&self, binding: &crate::store::cli_agent::Binding) -> bool {
        binding.create.session == self.scope.session
            && binding.verified_digest() == Some(self.binding)
    }
    pub fn matches_scope(
        &self,
        scope: &crate::execution::claude_cli::orchestration_protocol::Scope,
    ) -> bool {
        self.scope == *scope
    }
    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// Owns a newly prepared identity. bind requires NEW durable preparation, never lookup.
pub struct Server {
    listener: TcpListener,
    store: Arc<Store>,
    spec: BridgeSpec,
    launch: Launch,
    deadline: Instant,
    native_catalog: Option<[u8; 32]>,
}
/// Only Server can construct this authenticated, scope-bound ownership handoff.
pub struct Connected {
    parts: Parts,
    deadline: Instant,
    native_catalog: Option<[u8; 32]>,
}
pub struct Parts {
    pub reader: Reader<ReadHalf<TcpStream>>,
    pub writer: Writer<WriteHalf<TcpStream>>,
    pub lease: Lease,
}
impl Connected {
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn into_parts(self) -> Parts {
        self.parts
    }
    pub(crate) fn native_catalog(&self) -> Option<[u8; 32]> {
        self.native_catalog
    }
}
/// Commit the catalog alongside the independent executable/project/login binding identity.
/// The capability_hash column is the channel-secret digest, NOT a tool catalog digest.
pub fn native_binding_digest(binding: [u8; 32], catalog: [u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"pipkin-native-binding-catalog-v1");
    hash.update(binding);
    hash.update(catalog);
    hash.finalize().into()
}
impl Server {
    pub async fn bind(
        store: Arc<Store>,
        spec: BridgeSpec,
        deadline: Instant,
    ) -> Result<Self, Rejected> {
        Self::bind_scoped(store, spec, None, deadline).await
    }
    pub async fn bind_native(
        store: Arc<Store>,
        spec: BridgeSpec,
        catalog: [u8; 32],
        deadline: Instant,
    ) -> Result<Self, Rejected> {
        Self::bind_scoped(store, spec, Some(catalog), deadline).await
    }
    async fn bind_scoped(
        store: Arc<Store>,
        mut spec: BridgeSpec,
        native_catalog: Option<[u8; 32]>,
        deadline: Instant,
    ) -> Result<Self, Rejected> {
        let deadline = deadline.min(Instant::now() + MAX_LIFETIME);
        if Instant::now() >= deadline {
            return Err(Rejected);
        }
        let secret = Secret::fresh()?;
        spec.capability_hash = secret.digest();
        if let Some(catalog) = native_catalog {
            let binding = store
                .cli_agent_binding(&spec.scope.session)
                .map_err(|_| Rejected)?
                .ok_or(Rejected)?;
            if binding.digest != spec.binding_hash {
                return Err(Rejected);
            }
            spec.binding_hash = native_binding_digest(spec.binding_hash, catalog);
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Rejected)?;
        let SocketAddr::V4(endpoint) = listener.local_addr().map_err(|_| Rejected)? else {
            return Err(Rejected);
        };
        let launch = Launch {
            endpoint,
            secret,
            scope: scope_digest(&spec),
        };
        let db = store.clone();
        let identity = spec.clone();
        // Await the writer even if the deadline expires: the owner must not retire its
        // root while a detached preparation can still commit behind that retirement.
        // No caller may cancel/drop this bind as a substitute for owned cleanup.
        let fresh =
            tokio::task::spawn_blocking(move || db.prepare_cli_tool_bridge(&identity, now_ms()))
                .await
                .map_err(|_| Rejected)?
                .map_err(|_| Rejected)?;
        if !fresh || Instant::now() >= deadline {
            return Err(Rejected);
        }
        Ok(Self {
            listener,
            store,
            spec,
            launch,
            deadline,
            native_catalog,
        })
    }
    pub fn launch(&self) -> Launch {
        self.launch.clone()
    }
    pub fn native_launch(&self) -> Result<NativeLaunch, Rejected> {
        let catalog = self.native_catalog.ok_or(Rejected)?;
        let binding = self
            .store
            .cli_agent_binding(&self.spec.scope.session)
            .map_err(|_| Rejected)?
            .ok_or(Rejected)?;
        if Instant::now() >= self.deadline
            || native_binding_digest(binding.digest, catalog) != self.spec.binding_hash
        {
            return Err(Rejected);
        }
        Ok(NativeLaunch {
            launch: self.launch(),
            scope: self.spec.scope.clone(),
            binding: binding.digest,
            deadline: self.deadline,
        })
    }
    /// Captured immutable identity for the owner's explicit retirement path, including when
    /// accept is cancelled. Drop closes the listener; it does NOT perform blocking SQLite I/O.
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(crate) fn catalog(&self) -> Option<[u8; 32]> {
        self.native_catalog
    }
    pub fn identity(&self) -> &BridgeSpec {
        &self.spec
    }
    /// Consumes the listener: there is only one connection lifetime, even if its reply is lost.
    pub async fn accept(self) -> Result<Connected, Rejected> {
        let result = self.accept_inner().await;
        if result.is_err() {
            let _ = retire(self.store.clone(), self.spec.clone()).await;
        }
        result
    }
    async fn accept_inner(&self) -> Result<Connected, Rejected> {
        for _ in 0..MAX_BAD_PEERS {
            if Instant::now() >= self.deadline {
                return Err(Rejected);
            }
            let (stream, peer) = timeout_at(self.deadline, self.listener.accept())
                .await
                .map_err(|_| Rejected)?
                .map_err(|_| Rejected)?;
            if peer.ip() != Ipv4Addr::LOCALHOST {
                continue;
            }
            let pending = match wire::authenticate(
                stream,
                self.launch.secret.clone(),
                self.launch.scope,
                self.deadline,
            )
            .await
            {
                Ok(pending) => pending,
                Err(_) => continue,
            };
            let db = self.store.clone();
            let spec = self.spec.clone();
            let lease = timeout_at(
                self.deadline,
                tokio::task::spawn_blocking(move || db.claim_cli_tool_bridge(&spec, now_ms())),
            )
            .await
            .map_err(|_| Rejected)?
            .map_err(|_| Rejected)?
            .map_err(|_| Rejected)?
            .ok_or(Rejected)?;
            // The helper cannot send accepted frames until it verifies this commit proof.
            let (reader, writer) = pending.acknowledge(self.deadline).await?.split();
            return Ok(Connected {
                parts: Parts {
                    reader,
                    writer,
                    lease,
                },
                deadline: self.deadline,
                native_catalog: self.native_catalog,
            });
        }
        Err(Rejected)
    }
}
/// Owner cleanup, not an authentication endpoint. Cancelling the future does not cancel the
/// queued SQLite retirement; it must not be awaited before cancelling processes/in-memory work.
/// Storage failure is surfaced. Restart still retires any unconfirmed prepared/connected rows.
pub async fn retire(store: Arc<Store>, spec: BridgeSpec) -> Result<(), Rejected> {
    tokio::task::spawn_blocking(move || store.retire_cli_tool_bridge(&spec, now_ms()))
        .await
        .map_err(|_| Rejected)?
        .map_err(|_| Rejected)
}

#[cfg(test)]
mod tests;

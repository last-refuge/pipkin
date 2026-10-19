//! Private authenticated channel, NOT CLI stdout and not execution authority.
//! Mutual challenge proofs bind a fresh host secret to captured scope. Every frame is
//! direction/transcript/sequence authenticated. No secret is sent on the socket. Native
//! admission must commit between authenticate() and acknowledge(); no reconnect/retry.
//! This is loopback IPC, not encrypted transport or a sandbox against the same UID/root.
use super::{orchestration_protocol, permission_wire::Binding};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, timeout_at},
};
use tokio_util::sync::CancellationToken;

type Hmac256 = Hmac<Sha256>;
const MAGIC: &[u8; 8] = b"PKCB0001";
const HANDSHAKE_LIMIT: Duration = Duration::from_secs(2);
const FRAME_LIMIT: Duration = Duration::from_secs(10);
pub const REQUEST_BYTES: usize = orchestration_protocol::MAX_FRAME_BYTES;
pub const RESPONSE_BYTES: usize = orchestration_protocol::MAX_RESULT_BYTES + 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejected;
impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CLI bridge channel rejected")
    }
}
impl std::error::Error for Rejected {}

/// Sensitive host-generated secret, supplied only through the private helper environment.
#[derive(Clone)]
pub struct Secret(Binding);
impl Secret {
    pub fn fresh() -> Result<Self, Rejected> {
        Binding::fresh().map(Self).map_err(|_| Rejected)
    }
    pub fn from_hex(value: &str) -> Result<Self, Rejected> {
        Binding::from_hex(value).map(Self).map_err(|_| Rejected)
    }
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
    /// Nonsecret commitment for the durable bridge identity, never an authentication proof.
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }
    fn mac(&self, label: &[u8], parts: &[&[u8]]) -> Hmac256 {
        let mut mac =
            Hmac256::new_from_slice(self.0.as_bytes()).expect("HMAC accepts a 32-byte key");
        mac.update(MAGIC);
        mac.update(&(label.len() as u32).to_be_bytes());
        mac.update(label);
        for part in parts {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part);
        }
        mac
    }
    fn sign(&self, label: &[u8], parts: &[&[u8]]) -> [u8; 32] {
        self.mac(label, parts).finalize().into_bytes().into()
    }
    fn verify(&self, label: &[u8], parts: &[&[u8]], tag: &[u8]) -> Result<(), Rejected> {
        self.mac(label, parts)
            .verify_slice(tag)
            .map_err(|_| Rejected)
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

async fn bounded<T>(
    deadline: Instant,
    future: impl std::future::Future<Output = Result<T, Rejected>>,
) -> Result<T, Rejected> {
    if Instant::now() >= deadline {
        return Err(Rejected);
    }
    let value = timeout_at(deadline, future).await.map_err(|_| Rejected)??;
    if Instant::now() >= deadline {
        return Err(Rejected);
    }
    Ok(value)
}
fn nonce() -> Result<[u8; 32], Rejected> {
    Ok(*Binding::fresh().map_err(|_| Rejected)?.as_bytes())
}
fn transcript(scope: &[u8; 32], client: &[u8; 32], server: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(MAGIC);
    hash.update(scope);
    hash.update(client);
    hash.update(server);
    hash.finalize().into()
}

/// Private until durable host admission commits. Dropping it closes the unacknowledged socket.
pub(crate) struct Pending<S> {
    stream: S,
    secret: Secret,
    transcript: [u8; 32],
    expires: Instant,
}
pub(crate) async fn authenticate<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    secret: Secret,
    scope: [u8; 32],
    deadline: Instant,
) -> Result<Pending<S>, Rejected> {
    bounded(deadline.min(Instant::now() + HANDSHAKE_LIMIT), async {
        let mut hello = [0; 40];
        stream.read_exact(&mut hello).await.map_err(|_| Rejected)?;
        if &hello[..8] != MAGIC {
            return Err(Rejected);
        }
        let client: [u8; 32] = hello[8..].try_into().map_err(|_| Rejected)?;
        let server = nonce()?;
        let transcript = transcript(&scope, &client, &server);
        stream.write_all(&server).await.map_err(|_| Rejected)?;
        stream
            .write_all(&secret.sign(b"server", &[&transcript]))
            .await
            .map_err(|_| Rejected)?;
        stream.flush().await.map_err(|_| Rejected)?;
        let mut proof = [0; 32];
        stream.read_exact(&mut proof).await.map_err(|_| Rejected)?;
        secret.verify(b"client", &[&transcript], &proof)?;
        Ok(Pending {
            stream,
            secret,
            transcript,
            expires: deadline,
        })
    })
    .await
}
impl<S: AsyncRead + AsyncWrite + Unpin> Pending<S> {
    /// Only the host adapter calls this, AFTER its single-use claim commits.
    pub(crate) async fn acknowledge(mut self, deadline: Instant) -> Result<Channel<S>, Rejected> {
        bounded(
            deadline
                .min(self.expires)
                .min(Instant::now() + HANDSHAKE_LIMIT),
            async {
                self.stream
                    .write_all(&self.secret.sign(b"committed", &[&self.transcript]))
                    .await
                    .map_err(|_| Rejected)?;
                self.stream.flush().await.map_err(|_| Rejected)?;
                Ok(Channel::new(
                    self.stream,
                    self.secret,
                    self.transcript,
                    true,
                    self.expires,
                ))
            },
        )
        .await
    }
}
pub async fn connect<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    secret: Secret,
    scope: [u8; 32],
    deadline: Instant,
) -> Result<Channel<S>, Rejected> {
    bounded(deadline.min(Instant::now() + HANDSHAKE_LIMIT), async {
        let client = nonce()?;
        stream.write_all(MAGIC).await.map_err(|_| Rejected)?;
        stream.write_all(&client).await.map_err(|_| Rejected)?;
        stream.flush().await.map_err(|_| Rejected)?;
        let mut server = [0; 32];
        let mut proof = [0; 32];
        stream.read_exact(&mut server).await.map_err(|_| Rejected)?;
        stream.read_exact(&mut proof).await.map_err(|_| Rejected)?;
        let transcript = transcript(&scope, &client, &server);
        secret.verify(b"server", &[&transcript], &proof)?;
        stream
            .write_all(&secret.sign(b"client", &[&transcript]))
            .await
            .map_err(|_| Rejected)?;
        stream.flush().await.map_err(|_| Rejected)?;
        stream.read_exact(&mut proof).await.map_err(|_| Rejected)?;
        secret.verify(b"committed", &[&transcript], &proof)?;
        Ok(Channel::new(stream, secret, transcript, false, deadline))
    })
    .await
}

struct Direction {
    secret: Arc<Secret>,
    transcript: [u8; 32],
    label: &'static [u8],
    limit: usize,
    next: u64,
    expires: Instant,
    revoked: CancellationToken,
}
/// Armed across every await: dropping a partially consumed operation revokes BOTH halves.
struct Attempt {
    revoked: CancellationToken,
    armed: bool,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if self.armed {
            self.revoked.cancel();
        }
    }
}
impl Direction {
    fn begin(&self) -> Result<Attempt, Rejected> {
        if self.revoked.is_cancelled() || self.next == u64::MAX {
            self.revoked.cancel();
            return Err(Rejected);
        }
        Ok(Attempt {
            revoked: self.revoked.clone(),
            armed: true,
        })
    }
    fn tag(&self, payload: &[u8]) -> [u8; 32] {
        self.secret.sign(
            self.label,
            &[&self.transcript, &self.next.to_be_bytes(), payload],
        )
    }
}
/// No unbounded output queue. Each half holds at most one bounded frame; the caller owns
/// scheduling/backpressure. Dropping either half revokes the other immediately.
pub struct Reader<R> {
    io: R,
    direction: Direction,
}
pub struct Writer<W> {
    io: W,
    direction: Direction,
}
pub struct Channel<S> {
    stream: S,
    secret: Secret,
    transcript: [u8; 32],
    host: bool,
    expires: Instant,
}
impl<S> Channel<S> {
    fn new(stream: S, secret: Secret, transcript: [u8; 32], host: bool, expires: Instant) -> Self {
        Self {
            stream,
            secret,
            transcript,
            host,
            expires,
        }
    }
}
impl<S: AsyncRead + AsyncWrite> Channel<S> {
    pub fn split(
        self,
    ) -> (
        Reader<tokio::io::ReadHalf<S>>,
        Writer<tokio::io::WriteHalf<S>>,
    ) {
        let (read, write) = tokio::io::split(self.stream);
        let secret = Arc::new(self.secret);
        let revoked = CancellationToken::new();
        let direction = |request| Direction {
            secret: secret.clone(),
            transcript: self.transcript,
            label: if request { b"request" } else { b"response" },
            limit: if request {
                REQUEST_BYTES
            } else {
                RESPONSE_BYTES
            },
            next: 0,
            expires: self.expires,
            revoked: revoked.clone(),
        };
        (
            Reader {
                io: read,
                direction: direction(self.host),
            },
            Writer {
                io: write,
                direction: direction(!self.host),
            },
        )
    }
}
impl<R: AsyncRead + Unpin> Reader<R> {
    /// Admission/cancellation signal for the owning driver, not native tool completion.
    pub fn revocation(&self) -> CancellationToken {
        self.direction.revoked.clone()
    }
    pub async fn read(&mut self, deadline: Instant) -> Result<Vec<u8>, Rejected> {
        let mut attempt = self.direction.begin()?;
        let deadline = deadline.min(self.direction.expires);
        let work = async {
            let mut length = [0; 4];
            bounded(deadline, async {
                self.io
                    .read_exact(&mut length[..1])
                    .await
                    .map_err(|_| Rejected)?;
                Ok(())
            })
            .await?;
            bounded(deadline.min(Instant::now() + FRAME_LIMIT), async {
                self.io
                    .read_exact(&mut length[1..])
                    .await
                    .map_err(|_| Rejected)?;
                let len = u32::from_be_bytes(length) as usize;
                if len == 0 || len > self.direction.limit {
                    return Err(Rejected);
                }
                let mut sequence = [0; 8];
                let mut tag = [0; 32];
                self.io
                    .read_exact(&mut sequence)
                    .await
                    .map_err(|_| Rejected)?;
                if u64::from_be_bytes(sequence) != self.direction.next {
                    return Err(Rejected);
                }
                self.io.read_exact(&mut tag).await.map_err(|_| Rejected)?;
                let mut payload = vec![0; len];
                self.io
                    .read_exact(&mut payload)
                    .await
                    .map_err(|_| Rejected)?;
                self.direction.secret.verify(
                    self.direction.label,
                    &[&self.direction.transcript, &sequence, &payload],
                    &tag,
                )?;
                Ok(payload)
            })
            .await
        };
        let payload = tokio::select! { biased; _=attempt.revoked.cancelled()=>Err(Rejected), result=work=>result }?;
        self.direction.next += 1;
        attempt.armed = false;
        Ok(payload)
    }
    pub fn revoke(&self) {
        self.direction.revoked.cancel();
    }
}
impl<W: AsyncWrite + Unpin> Writer<W> {
    pub async fn write(&mut self, payload: &[u8], deadline: Instant) -> Result<(), Rejected> {
        let mut attempt = self.direction.begin()?;
        if payload.is_empty() || payload.len() > self.direction.limit {
            return Err(Rejected);
        }
        let work = bounded(
            deadline
                .min(self.direction.expires)
                .min(Instant::now() + FRAME_LIMIT),
            async {
                self.io
                    .write_all(&(payload.len() as u32).to_be_bytes())
                    .await
                    .map_err(|_| Rejected)?;
                self.io
                    .write_all(&self.direction.next.to_be_bytes())
                    .await
                    .map_err(|_| Rejected)?;
                self.io
                    .write_all(&self.direction.tag(payload))
                    .await
                    .map_err(|_| Rejected)?;
                self.io.write_all(payload).await.map_err(|_| Rejected)?;
                self.io.flush().await.map_err(|_| Rejected)?;
                Ok(())
            },
        );
        tokio::select! { biased; _=attempt.revoked.cancelled()=>Err(Rejected), result=work=>result }?;
        self.direction.next += 1;
        attempt.armed = false;
        Ok(())
    }
    pub fn revoke(&self) {
        self.direction.revoked.cancel();
    }
}
impl<R> Drop for Reader<R> {
    fn drop(&mut self) {
        self.direction.revoked.cancel();
    }
}
impl<W> Drop for Writer<W> {
    fn drop(&mut self) {
        self.direction.revoked.cancel();
    }
}

#[cfg(test)]
mod tests;

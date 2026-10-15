//! Bounded command-hook IPC only; no listener, permission parser, or approval authority.
//! Frames are a big-endian u32 byte length followed by one closed JSON object.
//! The host must authenticate its peer and expected run nonce, strictly parse
//! `payload.as_bytes()` before considering approval, and consume each invocation
//! once through its own gate. These types do not authorize anything. A matching
//! reply is not tool-completion evidence.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, timeout_at},
};

pub const WIRE_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
pub const MAX_STDIN_BYTES: usize = 64 * 1024;

/// Deliberately contains no underlying I/O, JSON, argument, or token diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireError;

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("permission IPC rejected")
    }
}
impl std::error::Error for WireError {}

/// A 256-bit binding, encoded as exactly 64 hexadecimal characters on the wire.
/// Used for ephemeral run/invocation nonces and the exact-payload SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Binding([u8; 32]);

impl Binding {
    /// Host-owned randomness, never a CLI credential or an identifier from CLI input.
    pub fn fresh() -> Result<Self, WireError> {
        let mut bytes = [0; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| WireError)?;
        Ok(Self(bytes))
    }

    pub fn from_hex(value: &str) -> Result<Self, WireError> {
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(WireError);
        }
        let mut bytes = [0; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| WireError)?;
        }
        Ok(Self(bytes))
    }

    /// Sensitive: only use for the child's environment or the private IPC frame.
    pub fn to_hex(self) -> String {
        const HEX: &[u8] = b"0123456789abcdef";
        let mut value = String::with_capacity(64);
        for byte in self.0 {
            value.push(HEX[(byte >> 4) as usize] as char);
            value.push(HEX[(byte & 15) as usize] as char);
        }
        value
    }

    pub fn payload_digest(payload: &[u8]) -> Self {
        Self(Sha256::digest(payload).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Binding([redacted])")
    }
}
impl Serialize for Binding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}
impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(|_| de::Error::custom("invalid permission binding"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestFrame {
    pub version: u32,
    pub run_nonce: Binding,
    pub invocation: Binding,
    pub payload_sha256: Binding,
    /// Original UTF-8 input, including whitespace and duplicate JSON keys.
    /// Do not parse/reserialize before handing these bytes to the strict host parser.
    pub payload: String,
}

impl RequestFrame {
    /// Each helper invocation creates fresh host-generated randomness here.
    pub fn new(run_nonce: Binding, payload: String) -> Result<Self, WireError> {
        if payload.len() > MAX_STDIN_BYTES {
            return Err(WireError);
        }
        Ok(Self {
            version: WIRE_VERSION,
            run_nonce,
            invocation: Binding::fresh()?,
            payload_sha256: Binding::payload_digest(payload.as_bytes()),
            payload,
        })
    }

    pub fn validate(&self) -> Result<(), WireError> {
        if self.version != WIRE_VERSION
            || self.payload.len() > MAX_STDIN_BYTES
            || self.payload_sha256 != Binding::payload_digest(self.payload.as_bytes())
        {
            return Err(WireError);
        }
        Ok(())
    }

    pub fn decision_from(&self, reply: &ReplyFrame) -> Result<Decision, WireError> {
        self.validate()?;
        if reply.version != WIRE_VERSION
            || reply.run_nonce != self.run_nonce
            || reply.invocation != self.invocation
            || reply.payload_sha256 != self.payload_sha256
        {
            return Err(WireError);
        }
        Ok(reply.decision)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Decision {
    AllowOnce,
    Deny,
}

impl<'de> Deserialize<'de> for Decision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "AllowOnce" => Ok(Self::AllowOnce),
            "Deny" => Ok(Self::Deny),
            _ => Err(de::Error::custom("invalid permission decision")),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyFrame {
    pub version: u32,
    pub run_nonce: Binding,
    pub invocation: Binding,
    pub payload_sha256: Binding,
    pub decision: Decision,
}

impl ReplyFrame {
    /// This constructor is not authorization. The host must parse, gate, and consume
    /// the request before constructing AllowOnce; invalid or stale requests deny.
    pub fn for_request(request: &RequestFrame, decision: Decision) -> Result<Self, WireError> {
        request.validate()?;
        Ok(Self {
            version: WIRE_VERSION,
            run_nonce: request.run_nonce,
            invocation: request.invocation,
            payload_sha256: request.payload_sha256,
            decision,
        })
    }
}

impl fmt::Debug for RequestFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RequestFrame([redacted])")
    }
}
impl fmt::Debug for ReplyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplyFrame([redacted])")
    }
}

// Require an object: serde's derived struct deserializer otherwise accepts arrays.
fn decode<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a closed permission frame")
        }
        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = deserializer
        .deserialize_map(Object(std::marker::PhantomData))
        .map_err(|_| WireError)?;
    deserializer.end().map_err(|_| WireError)?;
    Ok(value)
}

async fn read_frame<R: AsyncRead + Unpin, T: de::DeserializeOwned>(
    reader: &mut R,
    deadline: Instant,
) -> Result<T, WireError> {
    if Instant::now() >= deadline {
        return Err(WireError);
    }
    timeout_at(deadline, async {
        let len = reader.read_u32().await.map_err(|_| WireError)? as usize;
        if len == 0 || len > MAX_FRAME_BYTES {
            return Err(WireError);
        }
        let mut bytes = vec![0; len];
        reader.read_exact(&mut bytes).await.map_err(|_| WireError)?;
        let value = decode(&bytes)?;
        if Instant::now() >= deadline {
            return Err(WireError);
        }
        Ok(value)
    })
    .await
    .map_err(|_| WireError)?
}

async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
    deadline: Instant,
) -> Result<(), WireError> {
    let bytes = serde_json::to_vec(value).map_err(|_| WireError)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES || Instant::now() >= deadline {
        return Err(WireError);
    }
    timeout_at(deadline, async {
        writer
            .write_u32(bytes.len() as u32)
            .await
            .map_err(|_| WireError)?;
        writer.write_all(&bytes).await.map_err(|_| WireError)?;
        writer.flush().await.map_err(|_| WireError)?;
        if Instant::now() >= deadline {
            return Err(WireError);
        }
        Ok(())
    })
    .await
    .map_err(|_| WireError)?
}

pub async fn read_request<R: AsyncRead + Unpin>(
    reader: &mut R,
    deadline: Instant,
) -> Result<RequestFrame, WireError> {
    let request: RequestFrame = read_frame(reader, deadline).await?;
    request.validate()?;
    if Instant::now() >= deadline {
        return Err(WireError);
    }
    Ok(request)
}

pub async fn write_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &RequestFrame,
    deadline: Instant,
) -> Result<(), WireError> {
    request.validate()?;
    write_frame(writer, request, deadline).await
}

pub async fn read_reply<R: AsyncRead + Unpin>(
    reader: &mut R,
    request: &RequestFrame,
    deadline: Instant,
) -> Result<Decision, WireError> {
    let reply: ReplyFrame = read_frame(reader, deadline).await?;
    let decision = request.decision_from(&reply)?;
    if Instant::now() >= deadline {
        return Err(WireError);
    }
    Ok(decision)
}

pub async fn write_reply<W: AsyncWrite + Unpin>(
    writer: &mut W,
    reply: &ReplyFrame,
    deadline: Instant,
) -> Result<(), WireError> {
    if reply.version != WIRE_VERSION {
        return Err(WireError);
    }
    write_frame(writer, reply, deadline).await
}

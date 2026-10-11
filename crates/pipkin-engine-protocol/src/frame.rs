//! Length-prefixed JSON frames: a 4-byte big-endian length, then that many bytes of UTF-8 JSON.
//!
//! JSON keeps the private channel inspectable; the codec is isolated here so a binary encoding
//! can replace it after measurement without touching the message types.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::limits::MAX_FRAME_BYTES;

#[derive(Debug)]
pub enum FrameError {
    /// The peer closed the channel cleanly between frames.
    Closed,
    TooLarge(usize),
    Io(io::Error),
    Decode(serde_json::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Closed => f.write_str("channel closed"),
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes exceeds the limit"),
            FrameError::Io(e) => write!(f, "i/o: {e}"),
            FrameError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Encode one frame (header + body). Refuses a body over the limit.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(value).map_err(FrameError::Decode)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn write<T: Serialize>(w: &mut impl Write, value: &T) -> Result<(), FrameError> {
    let bytes = encode(value)?;
    w.write_all(&bytes).map_err(FrameError::Io)?;
    w.flush().map_err(FrameError::Io)
}

/// Read one frame. A clean end of stream before any header byte is `Closed`.
pub fn read<T: DeserializeOwned>(r: &mut impl Read) -> Result<T, FrameError> {
    let mut header = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut header[got..]) {
            Ok(0) if got == 0 => return Err(FrameError::Closed),
            Ok(0) => {
                return Err(FrameError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated frame header",
                )));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(FrameError::Io)?;
    serde_json::from_slice(&body).map_err(FrameError::Decode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Call, ClientFrame, Lane};

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let frame = ClientFrame {
            id: 7,
            lane: Lane::Control,
            call: Call::Stop {
                session: "s".into(),
            },
        };
        let mut buf = Vec::new();
        write(&mut buf, &frame).unwrap();
        write(&mut buf, &frame).unwrap();
        let mut r = &buf[..];
        assert_eq!(read::<ClientFrame>(&mut r).unwrap(), frame);
        assert_eq!(read::<ClientFrame>(&mut r).unwrap(), frame);
        assert!(matches!(read::<ClientFrame>(&mut r), Err(FrameError::Closed)));
    }

    #[test]
    fn oversized_and_truncated_frames_are_refused() {
        let mut buf = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(b"{}");
        assert!(matches!(
            read::<ClientFrame>(&mut &buf[..]),
            Err(FrameError::TooLarge(_))
        ));
        let truncated = [0u8, 0];
        assert!(matches!(
            read::<ClientFrame>(&mut &truncated[..]),
            Err(FrameError::Io(_))
        ));
        let mut short = 10u32.to_be_bytes().to_vec();
        short.extend_from_slice(b"{}");
        assert!(matches!(
            read::<ClientFrame>(&mut &short[..]),
            Err(FrameError::Io(_))
        ));
    }

    #[test]
    fn garbage_is_a_decode_error_not_a_panic() {
        let mut buf = 3u32.to_be_bytes().to_vec();
        buf.extend_from_slice(b"xyz");
        assert!(matches!(
            read::<ClientFrame>(&mut &buf[..]),
            Err(FrameError::Decode(_))
        ));
    }
}

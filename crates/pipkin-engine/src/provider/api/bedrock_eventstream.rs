//! AWS binary event-stream framing (`application/vnd.amazon.eventstream`), as used by Bedrock
//! `ConverseStream` responses.
//!
//! Each message is: a 12-byte prelude (`total_length: u32 BE`, `headers_length: u32 BE`,
//! `prelude_crc: u32 BE` = CRC32 of the first 8 bytes), the headers, the payload, and a
//! trailing `message_crc: u32 BE` = CRC32 of everything before it. A header is a 1-byte name
//! length, the name, a 1-byte type tag and a typed value. The decoder buffers bytes until a
//! whole message is present, so any fragmentation of the network reads decodes identically;
//! a checksum mismatch or malformed frame is a hard error (the stream cannot be resynchronized).

/// Largest message accepted (the SDK codec's limit is 16 MiB).
const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;
const PRELUDE_LEN: usize = 12;
const MIN_MESSAGE_LEN: usize = PRELUDE_LEN + 4;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (IEEE 802.3, the zlib/`crc32` polynomial).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in bytes {
        c = CRC_TABLE[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderValue {
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Bytes(Vec<u8>),
    String(String),
    /// Milliseconds since the Unix epoch.
    Timestamp(i64),
    Uuid([u8; 16]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventMessage {
    pub headers: Vec<(String, HeaderValue)>,
    pub payload: Vec<u8>,
}

impl EventMessage {
    /// A string header's value.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find_map(|(k, v)| match v {
            HeaderValue::String(s) if k == name => Some(s.as_str()),
            _ => None,
        })
    }
}

/// Incremental decoder over arbitrarily fragmented bytes.
#[derive(Default)]
pub struct EventStreamDecoder {
    buf: Vec<u8>,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl EventStreamDecoder {
    pub fn new() -> EventStreamDecoder {
        EventStreamDecoder::default()
    }

    /// Feed bytes; returns every complete message. An error is fatal for the stream.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<EventMessage>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut start = 0;
        while self.buf.len() - start >= PRELUDE_LEN {
            let b = &self.buf[start..];
            let total = be32(&b[0..4]) as usize;
            let headers_len = be32(&b[4..8]) as usize;
            let prelude_crc = be32(&b[8..12]);
            let computed = crc32(&b[0..8]);
            if prelude_crc != computed {
                return Err(format!(
                    "The prelude checksum specified in the message ({prelude_crc}) does not match the calculated CRC32 checksum ({computed})"
                ));
            }
            if !(MIN_MESSAGE_LEN..=MAX_MESSAGE_LEN).contains(&total) {
                return Err(format!("Invalid event stream message length: {total}"));
            }
            if headers_len > total - MIN_MESSAGE_LEN {
                return Err(format!(
                    "Invalid event stream headers length: {headers_len}"
                ));
            }
            if b.len() < total {
                break;
            }
            let message = &b[..total];
            let expected = be32(&message[total - 4..]);
            let computed = crc32(&message[..total - 4]);
            if expected != computed {
                return Err(format!(
                    "The message checksum ({computed}) did not match the expected value of {expected}"
                ));
            }
            let headers = decode_headers(&message[PRELUDE_LEN..PRELUDE_LEN + headers_len])?;
            let payload = message[PRELUDE_LEN + headers_len..total - 4].to_vec();
            out.push(EventMessage { headers, payload });
            start += total;
        }
        self.buf.drain(..start);
        Ok(out)
    }

    /// The body ended: leftover bytes mean a truncated message.
    pub fn finish(&self) -> Result<(), String> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Event stream ended inside a message ({} bytes left over)",
                self.buf.len()
            ))
        }
    }
}

fn decode_headers(mut b: &[u8]) -> Result<Vec<(String, HeaderValue)>, String> {
    fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
        if b.len() < n {
            return Err("Malformed event stream header".into());
        }
        let (head, tail) = b.split_at(n);
        *b = tail;
        Ok(head)
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        let name_len = take(&mut b, 1)?[0] as usize;
        let name = String::from_utf8_lossy(take(&mut b, name_len)?).into_owned();
        let tag = take(&mut b, 1)?[0];
        let value = match tag {
            0 => HeaderValue::Bool(true),
            1 => HeaderValue::Bool(false),
            2 => HeaderValue::Byte(take(&mut b, 1)?[0] as i8),
            3 => {
                let v = take(&mut b, 2)?;
                HeaderValue::Short(i16::from_be_bytes([v[0], v[1]]))
            }
            4 => HeaderValue::Int(be32(take(&mut b, 4)?) as i32),
            5 | 8 => {
                let v = take(&mut b, 8)?;
                let n = i64::from_be_bytes(v.try_into().map_err(|_| "Malformed header")?);
                if tag == 5 {
                    HeaderValue::Long(n)
                } else {
                    HeaderValue::Timestamp(n)
                }
            }
            6 | 7 => {
                let len = take(&mut b, 2)?;
                let len = u16::from_be_bytes([len[0], len[1]]) as usize;
                let v = take(&mut b, len)?;
                if tag == 6 {
                    HeaderValue::Bytes(v.to_vec())
                } else {
                    HeaderValue::String(String::from_utf8_lossy(v).into_owned())
                }
            }
            9 => {
                let v = take(&mut b, 16)?;
                HeaderValue::Uuid(v.try_into().map_err(|_| "Malformed header")?)
            }
            other => {
                return Err(format!(
                    "Unrecognized event stream header type tag: {other}"
                ));
            }
        };
        out.push((name, value));
    }
    Ok(out)
}

/// Encode a message (string headers only) — the inverse of the decoder, for tests and mocks.
#[cfg(any(test, feature = "test-support"))]
pub fn encode(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut h = Vec::new();
    for (k, v) in headers {
        h.push(k.len() as u8);
        h.extend_from_slice(k.as_bytes());
        h.push(7);
        h.extend_from_slice(&(v.len() as u16).to_be_bytes());
        h.extend_from_slice(v.as_bytes());
    }
    let total = (PRELUDE_LEN + h.len() + payload.len() + 4) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&(h.len() as u32).to_be_bytes());
    let prelude_crc = crc32(&out);
    out.extend_from_slice(&prelude_crc.to_be_bytes());
    out.extend_from_slice(&h);
    out.extend_from_slice(payload);
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

/// An `event` message with a JSON payload, as Bedrock sends them.
#[cfg(any(test, feature = "test-support"))]
pub fn encode_event(event_type: &str, payload: &serde_json::Value) -> Vec<u8> {
    encode(
        &[
            (":event-type", event_type),
            (":content-type", "application/json"),
            (":message-type", "event"),
        ],
        payload.to_string().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    /// Vectors from the AWS event-stream codec test suite (`int32_header`/`empty_message`).
    #[test]
    fn decodes_published_vectors() {
        let empty = [
            0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x05, 0xc2, 0x48, 0xeb, 0x7d, 0x98,
            0xc8, 0xff,
        ];
        let msgs = EventStreamDecoder::new().push(&empty).unwrap();
        assert_eq!(
            msgs,
            vec![EventMessage {
                headers: vec![],
                payload: vec![]
            }]
        );
        let int32 = [
            0x00, 0x00, 0x00, 0x2d, 0x00, 0x00, 0x00, 0x10, 0x41, 0xc4, 0x24, 0xb8, 0x0a, 0x65,
            0x76, 0x65, 0x6e, 0x74, 0x2d, 0x74, 0x79, 0x70, 0x65, 0x04, 0x00, 0x00, 0xa0, 0x0c,
            0x7b, 0x27, 0x66, 0x6f, 0x6f, 0x27, 0x3a, 0x27, 0x62, 0x61, 0x72, 0x27, 0x7d, 0x36,
            0xf4, 0x80, 0xa0,
        ];
        let msgs = EventStreamDecoder::new().push(&int32).unwrap();
        assert_eq!(
            msgs[0].headers,
            vec![("event-type".into(), HeaderValue::Int(40972))]
        );
        assert_eq!(msgs[0].payload, b"{'foo':'bar'}");
    }

    #[test]
    fn every_fragmentation_decodes_the_same() {
        let mut stream = encode_event("messageStart", &serde_json::json!({"role": "assistant"}));
        stream.extend(encode_event(
            "contentBlockDelta",
            &serde_json::json!({"contentBlockIndex": 0, "delta": {"text": "h\u{e9}llo \u{1f600}"}}),
        ));
        stream.extend(encode(&[], b""));
        for split_a in 1..stream.len() {
            for split_b in [split_a, (split_a + 7).min(stream.len()), stream.len()] {
                let mut d = EventStreamDecoder::new();
                let mut out = d.push(&stream[..split_a]).unwrap();
                out.extend(d.push(&stream[split_a..split_b]).unwrap());
                out.extend(d.push(&stream[split_b..]).unwrap());
                d.finish().unwrap();
                assert_eq!(out.len(), 3, "split {split_a}/{split_b}");
                assert_eq!(out[0].header(":event-type"), Some("messageStart"));
                assert_eq!(out[1].header(":message-type"), Some("event"));
                let v: serde_json::Value = serde_json::from_slice(&out[1].payload).unwrap();
                assert_eq!(v["delta"]["text"], "h\u{e9}llo \u{1f600}");
            }
        }
        let mut d = EventStreamDecoder::new();
        let mut n = 0;
        for b in &stream {
            n += d.push(&[*b]).unwrap().len();
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn checksum_failures_and_truncation_are_errors() {
        let good = encode_event(
            "messageStop",
            &serde_json::json!({"stopReason": "end_turn"}),
        );
        let mut bad_payload = good.clone();
        let mid = bad_payload.len() - 6;
        bad_payload[mid] ^= 0x01;
        let err = EventStreamDecoder::new().push(&bad_payload).unwrap_err();
        assert!(err.contains("message checksum"), "{err}");

        let mut bad_prelude = good.clone();
        bad_prelude[3] ^= 0x01;
        let err = EventStreamDecoder::new().push(&bad_prelude).unwrap_err();
        assert!(err.contains("prelude checksum"), "{err}");

        let mut d = EventStreamDecoder::new();
        assert!(d.push(&good[..good.len() - 1]).unwrap().is_empty());
        assert!(d.finish().is_err());
    }
}

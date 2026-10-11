//! Server-sent events decoding over arbitrarily fragmented bytes.
//!
//! Bytes are buffered until a full line is available, so a UTF-8 sequence or a JSON payload
//! split across network reads is never decoded early. Events end at a blank line; `data:`
//! lines are joined with `\n`; comments (`:`) are ignored; `\r\n`, `\n` and `\r` all end lines.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
    has_field: bool,
    /// Bytes buffered without a line end; refuse unbounded growth.
    max_line: usize,
}

impl SseDecoder {
    pub fn new() -> SseDecoder {
        SseDecoder {
            max_line: 64 * 1024 * 1024,
            ..SseDecoder::default()
        }
    }

    /// Feed bytes; returns complete events. Errors only on a line longer than the bound.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let Some(pos) = self.buf.iter().position(|b| *b == b'\n' || *b == b'\r') else {
                break;
            };
            // A lone '\r' at the end may be followed by '\n' in the next read.
            if self.buf[pos] == b'\r' && pos + 1 == self.buf.len() {
                break;
            }
            let line: Vec<u8> = self.buf.drain(..pos).collect();
            let first = self.buf.remove(0);
            if first == b'\r' && self.buf.first() == Some(&b'\n') {
                self.buf.remove(0);
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            if let Some(event) = self.line(&line) {
                out.push(event);
            }
        }
        if self.buf.len() > self.max_line {
            return Err("server-sent event line exceeds the size limit".into());
        }
        Ok(out)
    }

    /// The stream ended: flush a final event that had no trailing blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            if let Some(e) = self.line(&line) {
                return Some(e);
            }
        }
        self.dispatch()
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if !self.has_field {
            return None;
        }
        self.has_field = false;
        let event = SseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            id: self.id.take(),
        };
        Some(event)
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => {
                self.event = Some(value.to_owned());
                self.has_field = true;
            }
            "data" => {
                self.data.push(value.to_owned());
                self.has_field = true;
            }
            "id" => {
                self.id = Some(value.to_owned());
                self.has_field = true;
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_survive_any_fragmentation_including_split_utf8() {
        let raw = "event: a\ndata: {\"t\":\"h\u{e9}llo \u{1f600}\"}\n\n: comment\r\ndata: x\r\ndata: y\r\n\r\ndata: last";
        let bytes = raw.as_bytes();
        for split in 1..bytes.len() {
            let mut d = SseDecoder::new();
            let mut events = d.push(&bytes[..split]).unwrap();
            events.extend(d.push(&bytes[split..]).unwrap());
            events.extend(d.finish());
            assert_eq!(events.len(), 3, "split {split}");
            assert_eq!(events[0].event.as_deref(), Some("a"));
            assert_eq!(events[0].data, "{\"t\":\"h\u{e9}llo \u{1f600}\"}");
            assert_eq!(events[1].data, "x\ny");
            assert_eq!(events[2].data, "last");
        }
    }

    #[test]
    fn byte_at_a_time_matches() {
        let raw = b"data: 1\n\ndata: 2\r\r";
        let mut d = SseDecoder::new();
        let mut out = vec![];
        for b in raw {
            out.extend(d.push(&[*b]).unwrap());
        }
        out.extend(d.finish());
        assert_eq!(out.iter().map(|e| e.data.as_str()).collect::<Vec<_>>(), ["1", "2"]);
    }
}

//! Text helpers shared by the tools: JavaScript-compatible number formatting and parsing,
//! streaming lossy UTF-8 decoding, output bounding (`boundOutput`/`OutputBuffer` of the
//! reference `harness/output.ts`) and head truncation (`truncate.ts`).

use serde_json::{Value, json};

use super::{OutputLimits, Retain};

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

const NEWLINE: u8 = b'\n';

// ---------------------------------------------------------------------------------------------
// JavaScript numbers

/// `String(value)` / template-literal formatting of a JavaScript number.
pub fn js_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Shortest round-trip digits and the decimal exponent, e.g. "1.2345e-7".
    let formatted = format!("{:e}", value.abs());
    let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let k = digits.len() as i64;
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign_e = if e >= 0 { "+" } else { "-" };
        if k == 1 {
            format!("{digits}e{sign_e}{}", e.abs())
        } else {
            format!("{}.{}e{sign_e}{}", &digits[..1], &digits[1..], e.abs())
        }
    };
    format!("{sign}{body}")
}

/// A JSON number formatted like JavaScript would print it.
pub fn js_json_number(value: &serde_json::Number) -> String {
    if let Some(i) = value.as_i64() {
        return i.to_string();
    }
    if let Some(u) = value.as_u64() {
        return u.to_string();
    }
    js_number(value.as_f64().unwrap_or(f64::NAN))
}

fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `String.prototype.trimEnd`.
pub fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_whitespace)
}

/// `Number(text)` for a string: `None` when the result is `NaN`.
pub fn js_parse_number(text: &str) -> Option<f64> {
    let t = text.trim_matches(is_js_whitespace);
    if t.is_empty() {
        return Some(0.0);
    }
    let radix = |prefix: &str, radix: u32| -> Option<Option<f64>> {
        let rest = t
            .strip_prefix(prefix)
            .or_else(|| t.strip_prefix(&prefix.to_uppercase()))?;
        if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
            return Some(None);
        }
        let mut value = 0f64;
        for c in rest.chars() {
            value = value * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64;
        }
        Some(Some(value))
    };
    for (prefix, base) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(result) = radix(prefix, base) {
            return result;
        }
    }
    let (sign, unsigned) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..]),
        b'-' => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    if unsigned == "Infinity" {
        return Some(sign * f64::INFINITY);
    }
    // StrDecimalLiteral: digits [. digits] [e[+-]digits] | . digits [e...]
    let bytes = unsigned.as_bytes();
    let mut i = 0;
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - start;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    if i != bytes.len() {
        return None;
    }
    let normalized = if unsigned.starts_with('.') {
        format!("0{unsigned}")
    } else {
        unsigned.replace(".e", ".0e").replace(".E", ".0E")
    };
    let normalized = normalized.strip_suffix('.').unwrap_or(&normalized);
    normalized.parse::<f64>().ok().map(|v| sign * v)
}

/// `Math.trunc` as `Array.prototype.slice` converts an index: `NaN` is 0.
pub fn slice_index(value: f64) -> f64 {
    if value.is_nan() { 0.0 } else { value.trunc() }
}

/// `Number.isSafeInteger`.
pub fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0
}

/// `Math.max(a, b)` with JavaScript's NaN propagation.
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Math.min(a, b)` with JavaScript's NaN propagation.
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `formatSize`: `<n>B`, `<x.y>KB`, `<x.y>MB`.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------------------------------------
// Lossy UTF-8 decoding

/// Streaming lossy UTF-8 decoding equal to decoding the whole input at once with the WHATWG
/// decoder (one U+FFFD per maximal invalid subpart). It never strips a byte-order mark.
#[derive(Default)]
pub struct LossyDecoder {
    carry: Vec<u8>,
}

impl LossyDecoder {
    /// Decode `bytes`, holding back an incomplete trailing sequence unless `last`.
    pub fn feed(&mut self, bytes: &[u8], last: bool, sink: &mut dyn FnMut(&str)) {
        let joined;
        let buf: &[u8] = if self.carry.is_empty() {
            bytes
        } else {
            let mut b = std::mem::take(&mut self.carry);
            b.extend_from_slice(bytes);
            joined = b;
            &joined
        };
        let mut pos = 0;
        while pos < buf.len() {
            match std::str::from_utf8(&buf[pos..]) {
                Ok(text) => {
                    sink(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if valid > 0 {
                        // The prefix up to `valid_up_to` is valid UTF-8.
                        sink(std::str::from_utf8(&buf[pos..pos + valid]).unwrap_or_default());
                    }
                    match error.error_len() {
                        Some(len) => {
                            sink("\u{fffd}");
                            pos += valid + len;
                        }
                        None => {
                            if last {
                                sink("\u{fffd}");
                            } else {
                                self.carry = buf[pos + valid..].to_vec();
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    pub fn decode(&mut self, bytes: &[u8]) -> String {
        let mut out = String::new();
        self.feed(bytes, false, &mut |s| out.push_str(s));
        out
    }

    /// End of input: an incomplete sequence becomes one U+FFFD.
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        self.feed(&[], true, &mut |s| out.push_str(s));
        out
    }

    /// Decoded UTF-8 bytes of `bytes`, without building the text.
    pub fn count(&mut self, bytes: &[u8]) -> usize {
        let mut n = 0;
        self.feed(bytes, false, &mut |s| n += s.len());
        n
    }

    pub fn count_finish(&mut self) -> usize {
        let mut n = 0;
        self.feed(&[], true, &mut |s| n += s.len());
        n
    }
}

/// Decodes one process stream chunk by chunk; a U+FEFF at the very start is dropped
/// (`env/decode.ts` `StreamDecoder`).
#[derive(Default)]
pub struct StreamDecoder {
    inner: LossyDecoder,
    started: bool,
}

impl StreamDecoder {
    pub fn decode(&mut self, bytes: &[u8], last: bool) -> String {
        let mut text = String::new();
        self.inner.feed(bytes, last, &mut |s| text.push_str(s));
        if self.started || text.is_empty() {
            return text;
        }
        self.started = true;
        match text.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_owned(),
            None => text,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Output bounding (`harness/output.ts`)

/// Remove control characters that break display and transcripts; tabs and newlines stay.
pub fn sanitize_output(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{fff9}'..='\u{fffb}'))
        .collect()
}

/// An exact slice of the input within the limits, and what it left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputSlice {
    pub text: String,
    pub bytes: usize,
    pub dropped_bytes: usize,
    pub dropped_lines: usize,
}

/// Bound `text` to whole lines within the limits; a single line longer than `max_bytes` is cut
/// at the byte limit on a character boundary.
pub fn bound_output(text: &str, limits: OutputLimits) -> OutputSlice {
    let bytes = text.as_bytes();
    let (from, to) = match limits.retain {
        Retain::Head => head_range(bytes, limits),
        Retain::Tail => tail_range(bytes, limits),
    };
    let kept = &text[from..to];
    OutputSlice {
        text: kept.to_owned(),
        bytes: kept.len(),
        dropped_bytes: bytes.len() - kept.len(),
        dropped_lines: line_count(bytes) - line_count(kept.as_bytes()),
    }
}

fn head_range(bytes: &[u8], limits: OutputLimits) -> (usize, usize) {
    if limits.max_lines == 0 || limits.max_bytes == 0 {
        return (0, 0);
    }
    let mut end = bytes.len();
    let mut lines = 0;
    for (index, _) in bytes.iter().enumerate().filter(|(_, b)| **b == NEWLINE) {
        lines += 1;
        if lines == limits.max_lines {
            end = index + 1;
            break;
        }
    }
    if end > limits.max_bytes {
        let window = &bytes[..limits.max_bytes.min(bytes.len())];
        end = match memrchr(window) {
            Some(newline) => newline + 1,
            None => character_end(bytes, limits.max_bytes),
        };
    }
    (0, end)
}

fn tail_range(bytes: &[u8], limits: OutputLimits) -> (usize, usize) {
    let len = bytes.len();
    if limits.max_lines == 0 || limits.max_bytes == 0 {
        return (len, len);
    }
    // A trailing newline ends the last line rather than starting another.
    let last: isize = if len > 0 && bytes[len - 1] == NEWLINE {
        len as isize - 2
    } else {
        len as isize - 1
    };
    let mut start = 0usize;
    let mut lines = 1usize;
    let mut index = if last < 0 {
        None
    } else {
        memrchr(&bytes[..=last as usize])
    };
    while let Some(i) = index {
        if lines == limits.max_lines {
            start = i + 1;
            break;
        }
        lines += 1;
        index = if i == 0 { None } else { memrchr(&bytes[..i]) };
    }
    if len - start > limits.max_bytes {
        let from = len - limits.max_bytes;
        // `bytes.indexOf(NEWLINE, from - 1)`; a negative start searches from 0.
        let search = from.saturating_sub(1);
        let newline = bytes[search..]
            .iter()
            .position(|b| *b == NEWLINE)
            .map(|p| p + search);
        start = match newline {
            Some(n) if n + 1 < len => n + 1,
            _ => character_start(bytes, from),
        };
    }
    (start, len)
}

fn memrchr(bytes: &[u8]) -> Option<usize> {
    bytes.iter().rposition(|b| *b == NEWLINE)
}

/// The last character boundary at or before `index` (an index past the end is a boundary).
pub fn character_end(bytes: &[u8], index: usize) -> usize {
    let mut end = index;
    while end > 0 && (bytes.get(end).copied().unwrap_or(0) & 0xc0) == 0x80 {
        end -= 1;
    }
    end
}

fn character_start(bytes: &[u8], index: usize) -> usize {
    let mut start = index;
    while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
        start += 1;
    }
    start
}

fn line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|b| **b == NEWLINE).count();
    newlines + usize::from(bytes[bytes.len() - 1] != NEWLINE)
}

fn count_newlines(text: &str) -> usize {
    text.bytes().filter(|b| *b == NEWLINE).count()
}

/// Retained output and what the limits dropped from the whole stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundedOutput {
    pub text: String,
    pub dropped_bytes: u64,
    pub dropped_lines: u64,
}

struct Chunk {
    text: String,
    bytes: usize,
    newlines: usize,
}

/// Bounded running output of one tool call (`OutputBuffer`): head retention stops storing once
/// the window is full; tail retention keeps a suffix just longer than the window. Totals of the
/// whole stream are kept so dropped counts stay exact.
pub struct OutputBuffer {
    limits: OutputLimits,
    chunks: Vec<Chunk>,
    stored_bytes: usize,
    stored_newlines: usize,
    full: bool,
    total_bytes: u64,
    total_newlines: u64,
    ends_with_newline: bool,
}

impl OutputBuffer {
    pub fn new(limits: OutputLimits) -> OutputBuffer {
        OutputBuffer {
            limits,
            chunks: Vec::new(),
            stored_bytes: 0,
            stored_newlines: 0,
            full: false,
            total_bytes: 0,
            total_newlines: 0,
            ends_with_newline: true,
        }
    }

    /// Bytes currently held; bounded by the limits plus one chunk.
    pub fn stored_bytes(&self) -> usize {
        self.stored_bytes
    }

    /// Total bytes accepted so far.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn push(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let bytes = text.len();
        let newlines = count_newlines(text);
        self.total_bytes += bytes as u64;
        self.total_newlines += newlines as u64;
        self.ends_with_newline = text.ends_with('\n');
        if self.full {
            return true;
        }
        self.chunks.push(Chunk {
            text: text.to_owned(),
            bytes,
            newlines,
        });
        self.stored_bytes += bytes;
        self.stored_newlines += newlines;
        if self.limits.retain == Retain::Head {
            self.full = self.stored_bytes > self.limits.max_bytes
                || self.stored_newlines >= self.limits.max_lines;
            return true;
        }
        let mut drop = 0;
        while self.chunks.len() - drop > 1 {
            let first = &self.chunks[drop];
            let bytes_after = self.stored_bytes - first.bytes;
            let newlines_after = self.stored_newlines - first.newlines;
            if bytes_after <= self.limits.max_bytes.saturating_add(1)
                && newlines_after <= self.limits.max_lines.saturating_add(1)
            {
                break;
            }
            self.stored_bytes = bytes_after;
            self.stored_newlines = newlines_after;
            drop += 1;
        }
        if drop > 0 {
            self.chunks.drain(..drop);
        }
        true
    }

    /// Retained, sanitized output and what the limits dropped from the whole stream.
    pub fn snapshot(&mut self) -> BoundedOutput {
        let stored = if self.chunks.len() == 1 {
            self.chunks[0].text.clone()
        } else {
            self.chunks.iter().map(|c| c.text.as_str()).collect()
        };
        let kept = bound_output(&stored, self.limits);
        let stored_lines = lines(
            self.stored_newlines as u64,
            stored.is_empty() || stored.ends_with('\n'),
        );
        let kept_lines = stored_lines - kept.dropped_lines as u64;
        if self.limits.retain == Retain::Tail || self.chunks.len() > 1 {
            let text = if self.limits.retain == Retain::Tail {
                tail_margin(&stored, self.limits).to_owned()
            } else {
                stored
            };
            let bytes = text.len();
            let newlines = count_newlines(&text);
            self.chunks = if text.is_empty() {
                vec![]
            } else {
                vec![Chunk {
                    text,
                    bytes,
                    newlines,
                }]
            };
            self.stored_bytes = bytes;
            self.stored_newlines = self.chunks.first().map_or(0, |c| c.newlines);
        }
        BoundedOutput {
            text: sanitize_output(&kept.text),
            dropped_bytes: self.total_bytes - kept.bytes as u64,
            dropped_lines: lines(self.total_newlines, self.ends_with_newline) - kept_lines,
        }
    }
}

/// The shortest suffix of `text` with more than `max_bytes` bytes or more than `max_lines`
/// newlines, or all of it.
fn tail_margin(text: &str, limits: OutputLimits) -> &str {
    let bytes = text.as_bytes();
    let byte_start = if bytes.len() > limits.max_bytes {
        character_end(bytes, bytes.len() - limits.max_bytes - 1)
    } else {
        0
    };
    let mut line_start = 0;
    let mut newlines = 0usize;
    let mut index = memrchr(bytes);
    while let Some(i) = index {
        newlines += 1;
        if newlines > limits.max_lines {
            line_start = i;
            break;
        }
        if i == 0 {
            break;
        }
        index = memrchr(&bytes[..i]);
    }
    let start = byte_start.max(line_start);
    &text[start..]
}

fn lines(newlines: u64, terminated: bool) -> u64 {
    newlines + u64::from(!terminated)
}

// ---------------------------------------------------------------------------------------------
// Head truncation (`truncate.ts`)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Truncation {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl Truncation {
    /// The `TruncationResult` minus `content`, as the reference stores it in `details`.
    pub fn to_json(&self) -> Value {
        json!({
            "truncated": self.truncated,
            "truncatedBy": match self.truncated_by {
                None => Value::Null,
                Some(TruncatedBy::Lines) => json!("lines"),
                Some(TruncatedBy::Bytes) => json!("bytes"),
            },
            "totalLines": self.total_lines,
            "totalBytes": self.total_bytes,
            "outputLines": self.output_lines,
            "outputBytes": self.output_bytes,
            "lastLinePartial": self.last_line_partial,
            "firstLineExceedsLimit": self.first_line_exceeds_limit,
            "maxLines": self.max_lines,
            "maxBytes": self.max_bytes,
        })
    }
}

fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return vec![];
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// `truncateHead` with the default limits.
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> Truncation {
    let totals = (split_lines_for_counting(content).len(), content.len());
    truncate_head_of(content, totals, max_lines, max_bytes)
}

/// `truncateHeadOf`: `truncateHead` of a text known by a prefix and its totals (lines, bytes).
pub fn truncate_head_of(
    prefix: &str,
    (total_lines, total_bytes): (usize, usize),
    max_lines: usize,
    max_bytes: usize,
) -> Truncation {
    let lines = split_lines_for_counting(prefix);
    let base = Truncation {
        content: String::new(),
        truncated: true,
        truncated_by: Some(TruncatedBy::Bytes),
        total_lines,
        total_bytes,
        output_lines: 0,
        output_bytes: 0,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    };
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return Truncation {
            content: prefix.to_owned(),
            truncated: false,
            truncated_by: None,
            output_lines: total_lines,
            output_bytes: total_bytes,
            ..base
        };
    }
    let first_line_bytes = lines.first().map_or(0, |l| l.len());
    if first_line_bytes > max_bytes {
        return Truncation {
            first_line_exceeds_limit: true,
            ..base
        };
    }
    let mut out: Vec<&str> = Vec::new();
    let mut out_bytes = 0;
    let mut by_bytes = false;
    for (i, line) in lines.iter().enumerate().take(max_lines) {
        let line_bytes = line.len() + usize::from(i > 0);
        if out_bytes + line_bytes > max_bytes {
            by_bytes = true;
            break;
        }
        out.push(line);
        out_bytes += line_bytes;
    }
    let truncated_by = if by_bytes || out.len() >= total_lines {
        TruncatedBy::Bytes
    } else {
        TruncatedBy::Lines
    };
    let content = out.join("\n");
    Truncation {
        output_lines: out.len(),
        output_bytes: content.len(),
        content,
        truncated_by: Some(truncated_by),
        ..base
    }
}

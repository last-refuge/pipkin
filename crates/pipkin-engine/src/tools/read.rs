//! `read` (`tools/read.ts`, `tools/image.ts`, `env/line-scan.ts`).
//!
//! The result equals decoding the whole file (lossy UTF-8, a leading BOM dropped), splitting it
//! on `\n`, selecting lines with JavaScript `slice` semantics and bounding them with
//! `truncateHead`, while reading only one scan of the file plus the shown head.

use std::fs::File;
use std::path::Path;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::files::read_at;
use super::path::{node_error, resolve_read_tool_path};
use super::text::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, LossyDecoder, TruncatedBy, character_end, format_size,
    is_safe_integer, js_max, js_min, js_number, slice_index, truncate_head_of,
};
use super::{Diagnostic, Severity, Tool, ToolCtx, ToolError, ToolOutput, spec_from_json};
use crate::provider::{BoxFuture, ToolSpec};
use pipkin_engine_protocol::Block;

const SCHEMA: &str = r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},"limit":{"type":"number","description":"Maximum number of lines to read"}}}"#;

const DESCRIPTION: &str = "Read the contents of a text file. Output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.";

const READ_CHUNK: usize = 64 * 1024;

pub struct ReadTool;

impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        spec_from_json("read", DESCRIPTION, SCHEMA)
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or_default().to_owned();
            let offset = args.get("offset").and_then(Value::as_f64);
            let limit = args.get("limit").and_then(Value::as_f64);
            let cwd = ctx.cwd.clone();
            let cancel = ctx.cancel.clone();
            tokio::task::spawn_blocking(move || read_file(&cwd, &path, offset, limit, &cancel))
                .await
                .map_err(|e| ToolError::new(e.to_string()))?
        })
    }
}

fn aborted() -> ToolError {
    ToolError::new("aborted")
}

/// The blocking body of `read`.
pub fn read_file(
    cwd: &Path,
    path: &str,
    offset: Option<f64>,
    limit: Option<f64>,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    if cancel.is_cancelled() {
        return Err(aborted());
    }
    let absolute = resolve_read_tool_path(cwd, path).map_err(ToolError::new)?;
    let file = open_reader(&absolute)?;
    let io = |e: std::io::Error| ToolError::new(node_error(&e, "read", &absolute));
    // A concurrent writer can change the file between the scan and the reads; retry once.
    for attempt in 0.. {
        let before = file.metadata().map_err(io)?;
        let result = read_text(&file, before.len(), path, offset, limit, cancel)?;
        let after = file.metadata().map_err(io)?;
        if after.len() == before.len() && after.modified().ok() == before.modified().ok() {
            return Ok(result);
        }
        if attempt == 1 {
            return Err(ToolError::new(format!("{path} changed while it was read")));
        }
    }
    unreachable!("the loop returns on its second attempt")
}

fn open_reader(path: &Path) -> Result<File, ToolError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Nonblocking, so opening a FIFO does not wait for a writer.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| ToolError::new(node_error(&e, "open", path)))?;
    let meta = file
        .metadata()
        .map_err(|e| ToolError::new(node_error(&e, "fstat", path)))?;
    if !meta.is_file() {
        return Err(ToolError::new(if meta.is_dir() {
            "EISDIR: illegal operation on a directory, read"
        } else {
            "Not a regular file"
        }));
    }
    Ok(file)
}

fn read_text(
    file: &File,
    size: u64,
    path: &str,
    offset: Option<f64>,
    limit: Option<f64>,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let io = |e: std::io::Error| ToolError::new(e.to_string());
    if let Some(mime) = detect_image_of(file, size).map_err(io)? {
        return Ok(ToolOutput {
            content: Some(vec![]),
            is_error: true,
            details: None,
            diagnostics: vec![Diagnostic::new(
                Severity::Error,
                "unsupported_image",
                format!("{path} is an image ({mime}); reading images is not supported"),
            )],
        });
    }

    // `offset ? Math.max(0, offset - 1) : 0`: 0 and NaN are falsy.
    let start_line = match offset {
        Some(o) if o != 0.0 && !o.is_nan() => js_max(0.0, o - 1.0),
        _ => 0.0,
    };
    let start_line_display = start_line + 1.0;
    let slice_start = slice_index(start_line);
    let scan_start = if is_safe_integer(slice_start) {
        slice_start
    } else {
        0.0
    };
    let requested_end = limit.map(|l| js_max(scan_start + 1.0, slice_index(start_line + l)));
    let scan_end = requested_end.filter(|e| is_safe_integer(*e));
    let scan_of = |end: Option<f64>| -> Result<LineScan, ToolError> {
        scan_lines(file, scan_start as u64, end.map(|e| e as u64), cancel)
    };
    let mut scan = scan_of(scan_end)?;
    let total = scan.newlines as f64 + 1.0;
    if start_line >= total {
        return Err(ToolError::new(format!(
            "Offset {} is beyond end of file ({} lines total)",
            js_number(offset.unwrap_or(f64::NAN)),
            js_number(total)
        )));
    }

    let mut user_limited_lines = None;
    let mut selected_line_count = total - slice_start;
    if let Some(limit) = limit {
        let end_line = js_min(start_line + limit, total);
        user_limited_lines = Some(end_line - start_line);
        let relative_end = slice_index(end_line);
        let slice_end = if relative_end < 0.0 {
            (total + relative_end).max(0.0)
        } else {
            relative_end
        };
        selected_line_count = (slice_end - slice_start).max(0.0);
        if selected_line_count > 0.0 && relative_end < 0.0 {
            scan = scan_of(Some(slice_end))?;
        }
    }
    let empty = selected_line_count == 0.0;
    let ends_with_newline =
        !empty && scan.last_line_start == scan.end && scan.last_line_start > scan.start;
    let total_lines = if empty || scan.selected_bytes == 0 {
        0
    } else {
        selected_line_count as usize - usize::from(ends_with_newline)
    };
    let total_bytes = if empty { 0 } else { scan.selected_bytes };
    let mut first = [0u8; 3];
    let first_len = read_at(file, 0, &mut first).map_err(io)?;
    let bom = first_len == 3 && first == [0xef, 0xbb, 0xbf];
    let head = if empty {
        String::new()
    } else {
        read_head(file, scan.start, scan.end, bom, cancel)?
    };

    let truncation = truncate_head_of(
        &head,
        (total_lines, total_bytes as usize),
        DEFAULT_MAX_LINES,
        DEFAULT_MAX_BYTES,
    );
    let mut diagnostics = Vec::new();
    let mut output_text = truncation.content.clone();
    let mut details = None;
    if truncation.first_line_exceeds_limit {
        // Like `allLines[startLine]`, a fractional start line names no line.
        let integral = start_line.fract() == 0.0;
        let line: &[u8] = if integral {
            head.split('\n').next().unwrap_or_default().as_bytes()
        } else {
            &[]
        };
        let line_size = if integral { scan.first_line_bytes } else { 0 };
        let end = character_end(line, DEFAULT_MAX_BYTES);
        output_text = String::from_utf8_lossy(&line[..end.min(line.len())]).into_owned();
        let display = js_number(start_line_display);
        diagnostics.push(Diagnostic::new(
            Severity::Warn,
            "truncated",
            format!(
                "Line {display} is {}, exceeds the {} limit; showing its first {}. Use bash: sed -n '{display}p' {path} | tail -c +{}",
                format_size(line_size as usize),
                format_size(DEFAULT_MAX_BYTES),
                format_size(end),
                end + 1
            ),
        ));
        let mut t = truncation.to_json();
        t["outputBytes"] = json!(end);
        t["outputLines"] = json!(1);
        details = Some(json!({ "truncation": t }));
    } else if truncation.truncated {
        let end_line_display = start_line_display + truncation.output_lines as f64 - 1.0;
        let limit_text = if truncation.truncated_by == Some(TruncatedBy::Lines) {
            String::new()
        } else {
            format!(" ({} limit)", format_size(DEFAULT_MAX_BYTES))
        };
        diagnostics.push(Diagnostic::new(
            Severity::Info,
            "truncated",
            format!(
                "Showing lines {}-{} of {}{limit_text}. Use offset={} to continue.",
                js_number(start_line_display),
                js_number(end_line_display),
                js_number(total),
                js_number(end_line_display + 1.0)
            ),
        ));
        details = Some(json!({ "truncation": truncation.to_json() }));
    } else if let Some(limited) = user_limited_lines
        && start_line + limited < total
    {
        let remaining = total - (start_line + limited);
        let next = start_line + limited + 1.0;
        diagnostics.push(Diagnostic::new(
            Severity::Info,
            "",
            format!(
                "{} more lines in file. Use offset={} to continue.",
                js_number(remaining),
                js_number(next)
            ),
        ));
    }

    Ok(ToolOutput {
        content: Some(if output_text.is_empty() {
            vec![]
        } else {
            vec![Block::text(output_text)]
        }),
        is_error: false,
        details,
        diagnostics,
    })
}

/// The decoded start of bytes `[start, end)`: all of it, or enough for `truncate_head_of`
/// (more than `DEFAULT_MAX_BYTES + 1` bytes, or `DEFAULT_MAX_LINES` newlines).
fn read_head(
    file: &File,
    start: u64,
    end: u64,
    skip_bom: bool,
    cancel: &CancellationToken,
) -> Result<String, ToolError> {
    let mut decoder = LossyDecoder::default();
    let mut text = String::new();
    let mut newlines = 0;
    let mut position = if skip_bom && start == 0 { 3 } else { start };
    let mut buf = vec![0u8; READ_CHUNK];
    while position < end {
        if cancel.is_cancelled() {
            return Err(aborted());
        }
        let want = READ_CHUNK.min((end - position) as usize);
        let n =
            read_at(file, position, &mut buf[..want]).map_err(|e| ToolError::new(e.to_string()))?;
        if n == 0 {
            break;
        }
        position += n as u64;
        let decoded = decoder.decode(&buf[..n]);
        newlines += decoded.bytes().filter(|b| *b == b'\n').count();
        text.push_str(&decoded);
        if newlines >= DEFAULT_MAX_LINES || text.len() > DEFAULT_MAX_BYTES + 1 {
            return Ok(text);
        }
    }
    text.push_str(&decoder.finish());
    Ok(text)
}

// ---------------------------------------------------------------------------------------------
// Line scan (`env/line-scan.ts`)

/// Byte positions and decoded sizes of a line selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineScan {
    pub newlines: u64,
    pub start: u64,
    pub end: u64,
    pub first_line_end: u64,
    pub last_line_start: u64,
    pub selected_bytes: u64,
    pub first_line_bytes: u64,
}

/// Computes a [`LineScan`] from a file's bytes fed in order, in bounded memory.
pub struct LineScanner {
    start_line: u64,
    end_line: Option<u64>,
    position: u64,
    newlines: u64,
    line_start: u64,
    start: Option<u64>,
    end: Option<u64>,
    first_line_end: Option<u64>,
    last_line_start: Option<u64>,
    selected_bytes: u64,
    first_line_bytes: u64,
    selection: Option<LossyDecoder>,
    first_line: Option<LossyDecoder>,
    head: Option<Vec<u8>>,
    bom: bool,
}

impl LineScanner {
    /// `end_line` must be greater than `start_line`; `None` scans to the end.
    pub fn new(start_line: u64, end_line: Option<u64>) -> LineScanner {
        let mut scanner = LineScanner {
            start_line,
            end_line,
            position: 0,
            newlines: 0,
            line_start: 0,
            start: None,
            end: None,
            first_line_end: None,
            last_line_start: None,
            selected_bytes: 0,
            first_line_bytes: 0,
            selection: None,
            first_line: None,
            head: Some(Vec::new()),
            bom: false,
        };
        if start_line == 0 {
            scanner.begin(0);
        }
        scanner
    }

    fn is_last_selected(&self, line: u64) -> bool {
        self.end_line
            .is_some_and(|end| end.checked_sub(1) == Some(line))
    }

    pub fn push(&mut self, mut chunk: &[u8]) {
        if let Some(head) = &mut self.head {
            let take = (3 - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
            if head.len() < 3 {
                return;
            }
            self.release_head();
            chunk = &chunk[take..];
        }
        self.process(chunk);
    }

    fn release_head(&mut self) {
        let head = self.head.take().unwrap_or_default();
        self.bom = head.starts_with(&[0xef, 0xbb, 0xbf]);
        self.process(&head);
    }

    fn process(&mut self, chunk: &[u8]) {
        let base = self.position;
        let mut from = 0;
        for index in chunk
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i)
        {
            self.feed(chunk, base, from, index);
            let line = self.newlines;
            let position = base + index as u64;
            if line == self.start_line {
                self.end_first_line(position);
            }
            if self.is_last_selected(line) {
                self.end_selection(position);
            }
            self.feed(chunk, base, index, index + 1);
            from = index + 1;
            self.newlines += 1;
            self.line_start = position + 1;
            if self.newlines == self.start_line {
                self.begin(self.line_start);
            }
            if self.is_last_selected(self.newlines) {
                self.last_line_start = Some(self.line_start);
            }
        }
        self.feed(chunk, base, from, chunk.len());
        self.position += chunk.len() as u64;
    }

    pub fn finish(mut self) -> LineScan {
        if self.head.is_some() {
            self.release_head();
        }
        let size = self.position;
        let Some(start) = self.start else {
            return LineScan {
                newlines: self.newlines,
                start: size,
                end: size,
                first_line_end: size,
                last_line_start: size,
                selected_bytes: 0,
                first_line_bytes: 0,
            };
        };
        if self.first_line_end.is_none() {
            self.end_first_line(size);
        }
        if self.end.is_none() {
            self.end_selection(size);
        }
        LineScan {
            newlines: self.newlines,
            start,
            end: self.end.unwrap_or(size),
            first_line_end: self.first_line_end.unwrap_or(size),
            last_line_start: self.last_line_start.unwrap_or(self.line_start),
            selected_bytes: self.selected_bytes,
            first_line_bytes: self.first_line_bytes,
        }
    }

    fn begin(&mut self, start: u64) {
        self.start = Some(start);
        if self.is_last_selected(self.start_line) {
            self.last_line_start = Some(start);
        }
        self.selection = Some(LossyDecoder::default());
        self.first_line = Some(LossyDecoder::default());
    }

    fn end_first_line(&mut self, position: u64) {
        self.first_line_end = Some(position);
        if let Some(mut decoder) = self.first_line.take() {
            self.first_line_bytes += decoder.count_finish() as u64;
        }
    }

    fn end_selection(&mut self, position: u64) {
        self.end = Some(position);
        if let Some(mut decoder) = self.selection.take() {
            self.selected_bytes += decoder.count_finish() as u64;
        }
    }

    fn feed(&mut self, chunk: &[u8], base: u64, mut from: usize, to: usize) {
        // Decoding the whole file drops a leading byte-order mark.
        if self.bom && base + (from as u64) < 3 {
            from = to.min((3 - base) as usize);
        }
        if to <= from {
            return;
        }
        let bytes = &chunk[from..to];
        if let Some(decoder) = &mut self.selection {
            self.selected_bytes += decoder.count(bytes) as u64;
        }
        if let Some(decoder) = &mut self.first_line {
            self.first_line_bytes += decoder.count(bytes) as u64;
        }
    }
}

fn scan_lines(
    file: &File,
    start_line: u64,
    end_line: Option<u64>,
    cancel: &CancellationToken,
) -> Result<LineScan, ToolError> {
    let mut scanner = LineScanner::new(start_line, end_line);
    let mut buf = vec![0u8; READ_CHUNK];
    let mut position = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(aborted());
        }
        let n = read_at(file, position, &mut buf).map_err(|e| ToolError::new(e.to_string()))?;
        if n == 0 {
            return Ok(scanner.finish());
        }
        scanner.push(&buf[..n]);
        position += n as u64;
    }
}

// ---------------------------------------------------------------------------------------------
// Image sniffing (`tools/image.ts`)

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const HEADER_BYTES: usize = 32;
const BLOCK_BYTES: usize = 64 * 1024;

fn ascii_at(buffer: &[u8], offset: usize, text: &str) -> bool {
    buffer.get(offset..offset + text.len()) == Some(text.as_bytes())
}

fn u32_be(b: &[u8], o: usize) -> u64 {
    let at = |i: usize| b.get(o + i).copied().unwrap_or(0) as u64;
    (at(0) << 24) | (at(1) << 16) | (at(2) << 8) | at(3)
}

fn u32_le(b: &[u8], o: usize) -> u64 {
    let at = |i: usize| b.get(o + i).copied().unwrap_or(0) as u64;
    at(0) | (at(1) << 8) | (at(2) << 16) | (at(3) << 24)
}

fn u16_le(b: &[u8], o: usize) -> u64 {
    let at = |i: usize| b.get(o + i).copied().unwrap_or(0) as u64;
    at(0) | (at(1) << 8)
}

fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16 && u32_be(buffer, 8) == 13 && ascii_at(buffer, 12, "IHDR")
}

fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }
    let declared = u32_le(buffer, 2);
    let pixel_offset = u32_le(buffer, 10);
    let dib = u32_le(buffer, 14);
    if declared != 0 && declared < 26 {
        return false;
    }
    if pixel_offset < 14 + dib {
        return false;
    }
    if declared != 0 && pixel_offset >= declared {
        return false;
    }
    let (planes, bits) = if dib == 12 {
        (u16_le(buffer, 22), u16_le(buffer, 24))
    } else if (40..=124).contains(&dib) {
        if buffer.len() < 30 {
            return false;
        }
        (u16_le(buffer, 26), u16_le(buffer, 28))
    } else {
        return false;
    };
    planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits)
}

/// `detectSupportedImageMimeType` of a header (PNG animation is judged by the chunks present).
pub fn detect_image(buffer: &[u8]) -> Option<&'static str> {
    if buffer.starts_with(&[0xff, 0xd8, 0xff]) {
        return if buffer.get(3) == Some(&0xf7) {
            None
        } else {
            Some("image/jpeg")
        };
    }
    if buffer.starts_with(&PNG_SIGNATURE) {
        return (is_png(buffer)
            && !is_animated_png(buffer.len() as u64, &mut |o, l| {
                Ok(buffer
                    .get(o as usize..(o as usize + l).min(buffer.len()))
                    .unwrap_or_default()
                    .to_vec())
            })
            .unwrap_or(false))
        .then_some("image/png");
    }
    if ascii_at(buffer, 0, "GIF87a") || ascii_at(buffer, 0, "GIF89a") {
        return Some("image/gif");
    }
    if ascii_at(buffer, 0, "RIFF") && ascii_at(buffer, 8, "WEBP") {
        return Some("image/webp");
    }
    if ascii_at(buffer, 0, "BM") && is_bmp(buffer) {
        return Some("image/bmp");
    }
    None
}

type ReadFn<'a> = dyn FnMut(u64, usize) -> std::io::Result<Vec<u8>> + 'a;

fn is_animated_png(size: u64, read: &mut ReadFn<'_>) -> std::io::Result<bool> {
    let mut block: Vec<u8> = Vec::new();
    let mut block_start = 0u64;
    let mut offset = PNG_SIGNATURE.len() as u64;
    while offset + 8 <= size {
        if offset < block_start || offset + 8 > block_start + block.len() as u64 {
            block_start = offset;
            block = read(offset, BLOCK_BYTES)?;
        }
        let at = (offset - block_start) as usize;
        let header = &block[at..(at + 8).min(block.len())];
        let length = u32_be(header, 0);
        if ascii_at(header, 4, "acTL") {
            return Ok(true);
        }
        if ascii_at(header, 4, "IDAT") {
            return Ok(false);
        }
        let next = offset + 8 + length + 4;
        if next <= offset || next > size {
            return Ok(false);
        }
        offset = next;
    }
    Ok(false)
}

fn detect_image_of(file: &File, size: u64) -> std::io::Result<Option<&'static str>> {
    let mut header = vec![0u8; HEADER_BYTES];
    let n = read_at(file, 0, &mut header)?;
    header.truncate(n);
    if !header.starts_with(&PNG_SIGNATURE) {
        return Ok(detect_image(&header));
    }
    if !is_png(&header) {
        return Ok(None);
    }
    let animated = is_animated_png(size, &mut |offset, length| {
        let mut buf = vec![0u8; length];
        let n = read_at(file, offset, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    })?;
    Ok((!animated).then_some("image/png"))
}

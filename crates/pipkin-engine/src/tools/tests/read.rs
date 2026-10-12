//! `read`: ports of `tools.test.ts` "read" and `tools-read-differential.test.ts`, plus paths,
//! symlinks, missing files and permissions.

use std::path::Path;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{ctx, diagnostic_text, execute, output_text, temp};
use crate::tools::read::{ReadTool, detect_image, read_file};
use crate::tools::text::{
    DEFAULT_MAX_BYTES, TruncatedBy, character_end, format_size, js_number, truncate_head,
};
use crate::tools::{Diagnostic, Severity, ToolError, ToolOutput};

fn lines(n: usize) -> String {
    (1..=n)
        .map(|i| format!("Line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn read(dir: &Path, args: Value) -> Result<ToolOutput, ToolError> {
    execute(&ReadTool, args, &ctx(dir)).await.0
}

#[test]
fn detects_the_complete_gif_signatures() {
    assert_eq!(detect_image(b"GIF87a"), Some("image/gif"));
    assert_eq!(detect_image(b"GIF89a"), Some("image/gif"));
    assert_eq!(detect_image(b"GIF8"), None);
}

#[tokio::test]
async fn reads_text_with_offsets_and_limits_and_reports_continuation() {
    let dir = temp();
    std::fs::write(dir.path().join("test.txt"), lines(100)).unwrap();
    let out = read(
        dir.path(),
        json!({"path": "test.txt", "offset": 41, "limit": 20}),
    )
    .await
    .unwrap();
    let text = output_text(&out);
    assert!(!text.contains("Line 40"));
    assert!(text.starts_with("Line 41\n"));
    assert!(text.ends_with("Line 60"));
    assert_eq!(
        diagnostic_text(&out),
        "40 more lines in file. Use offset=61 to continue."
    );
    assert_eq!(out.diagnostics[0].code, "");
    assert!(out.details.is_none());
}

#[tokio::test]
async fn truncates_large_text_by_line_count() {
    let dir = temp();
    std::fs::write(dir.path().join("large.txt"), lines(2500)).unwrap();
    let out = read(dir.path(), json!({"path": "large.txt"}))
        .await
        .unwrap();
    assert_eq!(
        diagnostic_text(&out),
        "Showing lines 1-2000 of 2500. Use offset=2001 to continue."
    );
    assert_eq!(out.diagnostics[0].code, "truncated");
    assert_eq!(out.diagnostics[0].severity, Severity::Info);
    let t = &out.details.unwrap()["truncation"];
    assert_eq!(t["truncated"], true);
    assert_eq!(t["truncatedBy"], "lines");
    assert_eq!(t["totalLines"], 2500);
    assert_eq!(t["outputLines"], 2000);
}

#[tokio::test]
async fn reports_the_byte_limit_when_bytes_cut_first() {
    let dir = temp();
    let line = "x".repeat(99);
    std::fs::write(dir.path().join("wide.txt"), vec![line; 1000].join("\n")).unwrap();
    let out = read(dir.path(), json!({"path": "wide.txt", "offset": 3}))
        .await
        .unwrap();
    assert_eq!(
        diagnostic_text(&out),
        "Showing lines 3-514 of 1000 (50.0KB limit). Use offset=515 to continue."
    );
}

#[tokio::test]
async fn does_not_count_a_trailing_newline_as_an_extra_line_at_the_limit() {
    let dir = temp();
    std::fs::write(
        dir.path().join("exact.txt"),
        format!("{}\n", vec!["x"; 2000].join("\n")),
    )
    .unwrap();
    let out = read(dir.path(), json!({"path": "exact.txt"}))
        .await
        .unwrap();
    assert!(out.details.is_none());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn shows_the_start_of_a_line_longer_than_the_byte_limit() {
    let dir = temp();
    std::fs::write(
        dir.path().join("long.txt"),
        format!("{}\nnext\n", "é".repeat(40_000)),
    )
    .unwrap();
    let out = read(dir.path(), json!({"path": "long.txt"})).await.unwrap();
    assert_eq!(output_text(&out), "é".repeat(25_600));
    assert_eq!(
        diagnostic_text(&out),
        "Line 1 is 78.1KB, exceeds the 50.0KB limit; showing its first 50.0KB. Use bash: sed -n '1p' long.txt | tail -c +51201"
    );
    assert_eq!(out.diagnostics[0].severity, Severity::Warn);
    let t = &out.details.unwrap()["truncation"];
    assert_eq!(t["firstLineExceedsLimit"], true);
    assert_eq!(t["outputBytes"], 51_200);
    assert_eq!(t["outputLines"], 1);
    assert!(t.get("content").is_none());
}

#[tokio::test]
async fn rejects_offsets_beyond_the_file() {
    let dir = temp();
    std::fs::write(dir.path().join("short.txt"), "one\ntwo\nthree").unwrap();
    let err = read(dir.path(), json!({"path": "short.txt", "offset": 100}))
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        "Offset 100 is beyond end of file (3 lines total)"
    );
}

#[tokio::test]
async fn reports_images_by_content_as_unsupported() {
    use base64::Engine;
    let dir = temp();
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==")
        .unwrap();
    std::fs::write(dir.path().join("image.txt"), png).unwrap();
    let out = read(dir.path(), json!({"path": "image.txt"}))
        .await
        .unwrap();
    assert!(out.is_error);
    assert_eq!(out.content, Some(vec![]));
    assert_eq!(
        out.diagnostics,
        vec![Diagnostic::new(
            Severity::Error,
            "unsupported_image",
            "image.txt is an image (image/png); reading images is not supported"
        )]
    );
}

#[tokio::test]
async fn detects_animated_pngs_whose_actl_chunk_lies_far_beyond_the_header() {
    let chunk = |kind: &str, length: usize| {
        let mut v = (length as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind.as_bytes());
        v.extend(std::iter::repeat_n(0u8, length + 4));
        v
    };
    let mut png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend(chunk("IHDR", 13));
    png.extend(chunk("iCCP", 200_000));
    png.extend(chunk("acTL", 8));
    png.extend(chunk("IDAT", 10));
    let dir = temp();
    std::fs::write(dir.path().join("a.png"), &png).unwrap();
    let actual = read(dir.path(), json!({"path": "a.png"})).await;
    let expected = reference_read(&png, "a.png", None, None);
    assert_eq!(normalize(actual), normalize(expected));
}

#[tokio::test]
async fn missing_paths_directories_and_permissions_have_node_texts() {
    let dir = temp();
    let abs = dir.path().join("missing.txt");
    let err = read(dir.path(), json!({"path": "missing.txt"}))
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "ENOENT: no such file or directory, open '{}'",
            abs.display()
        )
    );
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let err = read(dir.path(), json!({"path": "sub"})).await.unwrap_err();
    assert_eq!(
        err.message,
        "EISDIR: illegal operation on a directory, read"
    );
    std::fs::write(dir.path().join("file"), "x").unwrap();
    let err = read(dir.path(), json!({"path": "file/inner"}))
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "ENOTDIR: not a directory, lstat '{}'",
            dir.path().join("file/inner").display()
        )
    );
    #[cfg(unix)]
    if unsafe { libc::geteuid() } != 0 {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.path().join("locked.txt");
        std::fs::write(&locked, "secret").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = read(dir.path(), json!({"path": "locked.txt"}))
            .await
            .unwrap_err();
        assert_eq!(
            err.message,
            format!("EACCES: permission denied, open '{}'", locked.display())
        );
    }
    #[cfg(unix)]
    {
        // A FIFO is refused without waiting for a writer.
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let err = read(dir.path(), json!({"path": "fifo"})).await.unwrap_err();
        assert_eq!(err.message, "Not a regular file");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn follows_symlinks_and_reports_dangling_ones() {
    let dir = temp();
    std::fs::write(dir.path().join("target.txt"), "through the link\n").unwrap();
    std::os::unix::fs::symlink("target.txt", dir.path().join("link.txt")).unwrap();
    let out = read(dir.path(), json!({"path": "link.txt"})).await.unwrap();
    assert_eq!(output_text(&out), "through the link\n");
    std::os::unix::fs::symlink("gone.txt", dir.path().join("dangling.txt")).unwrap();
    let err = read(dir.path(), json!({"path": "dangling.txt"}))
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "ENOENT: no such file or directory, open '{}'",
            dir.path().join("dangling.txt").display()
        )
    );
}

#[tokio::test]
async fn resolves_at_prefix_unicode_spaces_home_file_urls_and_name_variants() {
    let dir = temp();
    std::fs::write(dir.path().join("a b.txt"), "spaced").unwrap();
    let out = read(dir.path(), json!({"path": "@a\u{a0}b.txt"}))
        .await
        .unwrap();
    assert_eq!(output_text(&out), "spaced");
    let url = url::Url::from_file_path(dir.path().join("a b.txt")).unwrap();
    let out = read(Path::new("/"), json!({"path": url.as_str()}))
        .await
        .unwrap();
    assert_eq!(output_text(&out), "spaced");
    // macOS screenshot names use U+202F before AM/PM.
    std::fs::write(
        dir.path()
            .join("Screenshot 2024-01-01 at 10.00.00\u{202f}AM.png.txt"),
        "shot",
    )
    .unwrap();
    let out = read(
        dir.path(),
        json!({"path": "Screenshot 2024-01-01 at 10.00.00 AM.png.txt"}),
    )
    .await
    .unwrap();
    assert_eq!(output_text(&out), "shot");
    // Curly apostrophe and NFD variants.
    std::fs::write(dir.path().join("it\u{2019}s.txt"), "curly").unwrap();
    let out = read(dir.path(), json!({"path": "it's.txt"})).await.unwrap();
    assert_eq!(output_text(&out), "curly");
    std::fs::write(dir.path().join("cafe\u{301}.txt"), "nfd").unwrap();
    let out = read(dir.path(), json!({"path": "caf\u{e9}.txt"}))
        .await
        .unwrap();
    assert_eq!(output_text(&out), "nfd");
    // `..` is resolved lexically, like Node's `path.resolve`.
    let out = read(dir.path(), json!({"path": "nowhere/../a b.txt"}))
        .await
        .unwrap();
    assert_eq!(output_text(&out), "spaced");
}

#[tokio::test]
async fn reads_with_bom_crlf_and_invalid_utf8_like_a_whole_file_decode() {
    let dir = temp();
    let bytes = b"\xef\xbb\xbfone\r\ntw\xffo\r\n\xe2\x82".to_vec();
    std::fs::write(dir.path().join("f.txt"), &bytes).unwrap();
    let out = read(dir.path(), json!({"path": "f.txt"})).await.unwrap();
    assert_eq!(output_text(&out), "one\r\ntw\u{fffd}o\r\n\u{fffd}");
    let out = read(
        dir.path(),
        json!({"path": "f.txt", "offset": 2, "limit": 1}),
    )
    .await
    .unwrap();
    assert_eq!(output_text(&out), "tw\u{fffd}o\r");
    assert_eq!(
        diagnostic_text(&out),
        "1 more lines in file. Use offset=3 to continue."
    );
}

#[tokio::test]
async fn empty_selection_has_no_content() {
    let dir = temp();
    std::fs::write(dir.path().join("f.txt"), "a\nb\n").unwrap();
    let out = read(dir.path(), json!({"path": "f.txt", "limit": 0}))
        .await
        .unwrap();
    assert_eq!(out.content, Some(vec![]));
    assert_eq!(
        diagnostic_text(&out),
        "3 more lines in file. Use offset=1 to continue."
    );
    std::fs::write(dir.path().join("empty.txt"), "").unwrap();
    let out = read(dir.path(), json!({"path": "empty.txt"}))
        .await
        .unwrap();
    assert_eq!(out.content, Some(vec![]));
    assert!(out.diagnostics.is_empty());
}

#[test]
fn absolute_paths_expand_home_and_file_urls_like_node() {
    use crate::tools::path::{absolute_path, normalize_tool_path};
    let cwd = Path::new("/work/dir");
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    assert_eq!(absolute_path(cwd, "~"), home);
    assert_eq!(absolute_path(cwd, "~/a/../b"), home.join("b"));
    assert_eq!(absolute_path(cwd, "x/./y/"), Path::new("/work/dir/x/y"));
    assert_eq!(absolute_path(cwd, "../../../up"), Path::new("/up"));
    assert_eq!(
        absolute_path(cwd, "file:///tmp/a%20b"),
        Path::new("/tmp/a b")
    );
    assert_eq!(
        absolute_path(cwd, "file://localhost/tmp/x"),
        Path::new("/tmp/x")
    );
    // Malformed or remote file URLs stay ordinary (relative) paths.
    assert_eq!(
        absolute_path(cwd, "file://host/x"),
        Path::new("/work/dir/file:/host/x")
    );
    assert_eq!(
        absolute_path(cwd, "file:///a%2Fb"),
        Path::new("/work/dir/file:/a%2Fb")
    );
    assert_eq!(normalize_tool_path("@\u{3000}x\u{202f}y"), " x y");
    assert_eq!(normalize_tool_path("@@x"), "@x");
}

#[tokio::test]
async fn a_cancelled_read_is_aborted() {
    let dir = temp();
    std::fs::write(dir.path().join("f.txt"), "a").unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = read_file(dir.path(), "f.txt", None, None, &cancel).unwrap_err();
    assert_eq!(err.message, "aborted");
}

// ---------------------------------------------------------------------------------------------
// Differential test (`tools-read-differential.test.ts`): the bounded reader must return exactly
// what decoding the whole file, splitting it into lines and truncating the selection returns.

/// `Array.prototype.slice(start, end)` bounds for an array of `len`.
fn js_slice(len: usize, start: f64, end: Option<f64>) -> (usize, usize) {
    let len_f = len as f64;
    let clamp = |v: f64| {
        let v = if v.is_nan() { 0.0 } else { v.trunc() };
        if v < 0.0 {
            (len_f + v).max(0.0)
        } else {
            v.min(len_f)
        }
    };
    let from = clamp(start);
    let to = end.map_or(len_f, clamp);
    (from as usize, to.max(from) as usize)
}

/// The `read` tool before bounded reads, kept as the reference.
fn reference_read(
    bytes: &[u8],
    path: &str,
    offset: Option<f64>,
    limit: Option<f64>,
) -> Result<ToolOutput, ToolError> {
    if let Some(mime) = detect_image(bytes) {
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
    let decoded = String::from_utf8_lossy(bytes);
    let text = decoded.strip_prefix('\u{feff}').unwrap_or(&decoded);
    let all: Vec<&str> = text.split('\n').collect();
    let total = all.len() as f64;
    let start_line = match offset {
        Some(o) if o != 0.0 && !o.is_nan() => (o - 1.0).max(0.0),
        _ => 0.0,
    };
    let display = start_line + 1.0;
    if start_line >= total {
        return Err(ToolError::new(format!(
            "Offset {} is beyond end of file ({} lines total)",
            js_number(offset.unwrap_or(f64::NAN)),
            all.len()
        )));
    }
    let mut user_limited = None;
    let selected = match limit {
        Some(limit) => {
            let end = if (start_line + limit).is_nan() {
                f64::NAN
            } else {
                (start_line + limit).min(total)
            };
            user_limited = Some(end - start_line);
            let (a, b) = js_slice(all.len(), start_line, Some(end));
            all[a..b].join("\n")
        }
        None => {
            let (a, b) = js_slice(all.len(), start_line, None);
            all[a..b].join("\n")
        }
    };
    let t = truncate_head(&selected, 2000, DEFAULT_MAX_BYTES);
    let mut diagnostics = vec![];
    let mut out = t.content.clone();
    let mut details = None;
    if t.first_line_exceeds_limit {
        let line: &str = if start_line.fract() == 0.0 {
            all.get(start_line as usize).copied().unwrap_or_default()
        } else {
            ""
        };
        let end = character_end(line.as_bytes(), DEFAULT_MAX_BYTES);
        out = line[..end.min(line.len())].to_owned();
        diagnostics.push(Diagnostic::new(
            Severity::Warn,
            "truncated",
            format!(
                "Line {d} is {}, exceeds the {} limit; showing its first {}. Use bash: sed -n '{d}p' {path} | tail -c +{}",
                format_size(line.len()),
                format_size(DEFAULT_MAX_BYTES),
                format_size(end),
                end + 1,
                d = js_number(display)
            ),
        ));
        let mut j = t.to_json();
        j["outputBytes"] = json!(end);
        j["outputLines"] = json!(1);
        details = Some(json!({ "truncation": j }));
    } else if t.truncated {
        let end_display = display + t.output_lines as f64 - 1.0;
        let limit_text = if t.truncated_by == Some(TruncatedBy::Lines) {
            String::new()
        } else {
            " (50.0KB limit)".into()
        };
        diagnostics.push(Diagnostic::new(
            Severity::Info,
            "truncated",
            format!(
                "Showing lines {}-{} of {}{limit_text}. Use offset={} to continue.",
                js_number(display),
                js_number(end_display),
                all.len(),
                js_number(end_display + 1.0)
            ),
        ));
        details = Some(json!({ "truncation": t.to_json() }));
    } else if let Some(u) = user_limited
        && start_line + u < total
    {
        diagnostics.push(Diagnostic::new(
            Severity::Info,
            "",
            format!(
                "{} more lines in file. Use offset={} to continue.",
                js_number(total - (start_line + u)),
                js_number(start_line + u + 1.0)
            ),
        ));
    }
    Ok(ToolOutput {
        content: Some(if out.is_empty() {
            vec![]
        } else {
            vec![pipkin_engine_protocol::Block::text(out)]
        }),
        is_error: false,
        details,
        diagnostics,
    })
}

fn normalize(result: Result<ToolOutput, ToolError>) -> Result<ToolOutput, String> {
    result.map_err(|e| e.message)
}

/// mulberry32, as the reference test seeds it.
fn random(seed: u32) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x6d2b_79f5);
        let mut t = state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

const PIECES: [&[u8]; 11] = [
    &[0x0a],
    &[0x0a],
    &[0x61],
    &[0x62, 0x63, 0x64, 0x65],
    &[0xef, 0xbb, 0xbf],
    &[0xc3, 0xa9],
    &[0xe2, 0x82, 0xac],
    &[0xf0, 0x9f, 0x98, 0x80],
    &[0xe2, 0x82],
    &[0xff],
    &[0x0d, 0x0a],
];

fn random_file(next: &mut impl FnMut() -> f64) -> Vec<u8> {
    let kind = next();
    if kind < 0.1 {
        let mut v = vec![0xef, 0xbb, 0xbf];
        v.extend("x\n".repeat(2100).as_bytes());
        return v;
    }
    if kind < 0.2 {
        let mut line = vec![0x61u8; DEFAULT_MAX_BYTES + 200 + (next() * 400.0) as usize];
        let at = DEFAULT_MAX_BYTES - 1 + (next() * 3.0) as usize;
        line[at] = 0xc3;
        line.extend([0x0a, 0x62]);
        return line;
    }
    if kind < 0.3 {
        return format!("{}\n", "0123456789".repeat(30))
            .repeat(200)
            .into_bytes();
    }
    let mut bytes = Vec::new();
    let pieces = (next() * 80.0) as usize;
    for _ in 0..pieces {
        bytes.extend_from_slice(PIECES[(next() * PIECES.len() as f64) as usize]);
    }
    bytes
}

#[test]
fn returns_exactly_what_reading_the_whole_file_returned() {
    let offsets = [
        None,
        Some(0.0),
        Some(1.0),
        Some(2.0),
        Some(3.0),
        Some(2.5),
        Some(-4.0),
        Some(50.0),
        Some(2001.0),
        Some(3000.0),
        Some(1e20),
        Some(f64::NAN),
    ];
    let limits = [
        None,
        Some(0.0),
        Some(1.0),
        Some(2.0),
        Some(-3.0),
        Some(-1e20),
        Some(1.5),
        Some(7.0),
        Some(2000.0),
        Some(2500.0),
        Some(1e20),
        Some(f64::NAN),
    ];
    let dir = temp();
    let path = dir.path().join("f.txt");
    let cancel = CancellationToken::new();
    for seed in 1..=400u32 {
        let mut next = random(seed);
        let file = random_file(&mut next);
        std::fs::write(&path, &file).unwrap();
        for _ in 0..4 {
            let offset = offsets[(next() * offsets.len() as f64) as usize];
            let limit = limits[(next() * limits.len() as f64) as usize];
            let actual = read_file(dir.path(), "f.txt", offset, limit, &cancel);
            let expected = reference_read(&file, "f.txt", offset, limit);
            assert_eq!(
                normalize(actual),
                normalize(expected),
                "seed {seed} offset {offset:?} limit {limit:?}"
            );
        }
    }
}

/// Compares against a corpus produced by the reference implementation itself (see the report:
/// `PIPKIN_READ_CORPUS=<json>`); defaults to the frozen corpus, with an optional JSON override.
#[test]
fn matches_a_reference_corpus() {
    let bytes = match std::env::var("PIPKIN_READ_CORPUS") {
        Ok(path) => std::fs::read(path).unwrap(),
        Err(_) => zstd::stream::decode_all(
            &include_bytes!("../../../../../fixtures/native-tool-corpora/read.json.zst")[..],
        )
        .unwrap(),
    };
    let cases: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
    let dir = temp();
    let cancel = CancellationToken::new();
    let mut checked = 0;
    for case in cases {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(case["bytes"].as_str().unwrap())
            .unwrap();
        std::fs::write(dir.path().join("f.txt"), &bytes).unwrap();
        let num = |v: &Value| match v {
            Value::Null => None,
            Value::String(s) if s == "NaN" => Some(f64::NAN),
            v => v.as_f64(),
        };
        let actual = read_file(
            dir.path(),
            "f.txt",
            num(&case["offset"]),
            num(&case["limit"]),
            &cancel,
        );
        let actual = match actual {
            Ok(out) => json!({
                "text": output_text(&out),
                "isError": out.is_error,
                "details": out.details,
                "diagnostics": out.diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
            }),
            Err(e) => json!({ "error": e.message }),
        };
        assert_eq!(actual, case["expected"], "case {}", case["id"]);
        checked += 1;
    }
    eprintln!("checked {checked} reference cases");
}

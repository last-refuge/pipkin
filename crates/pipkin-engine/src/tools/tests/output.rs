//! Ports of `harness-output.test.ts` and `env-truncate.test.ts`, plus the number helpers.

use crate::tools::text::*;
use crate::tools::{OutputLimits, Retain};

fn head(max_lines: usize, max_bytes: usize) -> OutputLimits {
    OutputLimits {
        max_bytes,
        max_lines,
        retain: Retain::Head,
    }
}

fn tail(max_lines: usize, max_bytes: usize) -> OutputLimits {
    OutputLimits {
        max_bytes,
        max_lines,
        retain: Retain::Tail,
    }
}

fn bound(text: &str, limits: OutputLimits) -> (String, usize, usize) {
    let s = bound_output(text, limits);
    (s.text, s.dropped_bytes, s.dropped_lines)
}

#[test]
fn removes_control_characters_but_keeps_tabs_newlines_and_other_text() {
    assert_eq!(
        sanitize_output("a\0b\tc\nd\re\u{7}f\u{fff9}g\u{fffb}h😀"),
        "ab\tc\ndefgh😀"
    );
}

#[test]
fn keeps_output_within_the_limits_unchanged() {
    assert_eq!(bound("a\nb\n", head(2, 1000)), ("a\nb\n".into(), 0, 0));
    assert_eq!(bound("a\nb", tail(2, 1000)), ("a\nb".into(), 0, 0));
    assert_eq!(bound("", tail(2, 1000)), ("".into(), 0, 0));
}

#[test]
fn keeps_nothing_with_a_zero_limit() {
    assert_eq!(bound("ab\ncd\n", head(10, 0)), ("".into(), 6, 2));
    assert_eq!(bound("ab\ncd\n", tail(0, 1000)), ("".into(), 6, 2));
}

#[test]
fn keeps_exact_slices_of_whole_lines_trailing_newline_included() {
    assert_eq!(bound("a\nb\nc\n", head(2, 1000)), ("a\nb\n".into(), 2, 1));
    assert_eq!(bound("a\nb\nc\n", tail(2, 1000)), ("b\nc\n".into(), 2, 1));
    assert_eq!(bound("a\nb\nc", tail(2, 1000)), ("b\nc".into(), 2, 1));
    assert_eq!(
        bound("a\nb\nc\n\n", tail(3, 1000)),
        ("b\nc\n\n".into(), 2, 1)
    );
}

#[test]
fn cuts_at_the_byte_limit_on_whole_lines_when_possible() {
    assert_eq!(
        bound("aa\nbb\ncc\n", head(10, 7)),
        ("aa\nbb\n".into(), 3, 1)
    );
    assert_eq!(
        bound("aa\nbb\ncc\n", tail(10, 7)),
        ("bb\ncc\n".into(), 3, 1)
    );
}

#[test]
fn cuts_a_single_line_longer_than_the_byte_limit_on_a_character_boundary() {
    assert_eq!(bound("ééé\n", head(10, 5)), ("éé".into(), 3, 0));
    assert_eq!(bound("x\néééé", tail(10, 5)), ("éé".into(), 6, 1));
}

fn buffer_tail(content: &str, max_bytes: usize) -> &str {
    let bytes = content.as_bytes();
    if bytes.len() <= max_bytes {
        return content;
    }
    let mut start = bytes.len() - max_bytes;
    while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
        start += 1;
    }
    &content[start..]
}

#[test]
fn cuts_tails_exactly_like_a_byte_tail_slice_across_fuzz_cases() {
    let alphabet = [
        "a",
        "\u{7f}",
        "\u{80}",
        "é",
        "\u{7ff}",
        "\u{800}",
        "中",
        "\u{d7ff}",
        "🙂",
        "\u{e000}",
        "\u{ffff}",
        "👩\u{200d}💻",
    ];
    let mut seed: u32 = 0x1234_5678;
    let mut random = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed as f64 / 4_294_967_296.0
    };
    for _ in 0..1000 {
        let mut input = String::new();
        let length = (random() * 40.0) as usize;
        for _ in 0..length {
            input.push_str(alphabet[(random() * alphabet.len() as f64) as usize]);
        }
        for max_bytes in 0..input.len() + 3 {
            let kept = bound_output(&input, tail(10, max_bytes)).text;
            assert_eq!(
                kept,
                buffer_tail(&input, max_bytes),
                "{input:?} {max_bytes}"
            );
            assert!(kept.len() <= max_bytes);
        }
    }
}

#[test]
fn output_buffer_keeps_exact_totals_across_chunks() {
    let mut buffer = OutputBuffer::new(tail(2, 1000));
    buffer.push("a\nb\n");
    buffer.push("😀\n");
    let s = buffer.snapshot();
    assert_eq!(
        (s.text.as_str(), s.dropped_bytes, s.dropped_lines),
        ("b\n😀\n", 2, 1)
    );
}

#[test]
fn output_buffer_sanitizes_the_retained_text_but_counts_the_raw_stream() {
    let mut buffer = OutputBuffer::new(tail(1, 1000));
    buffer.push("a\u{7}\n");
    let s = buffer.snapshot();
    assert_eq!(
        (s.text.as_str(), s.dropped_bytes, s.dropped_lines),
        ("a\n", 0, 0)
    );
    buffer.push("b\u{1b}\n");
    let s = buffer.snapshot();
    assert_eq!(
        (s.text.as_str(), s.dropped_bytes, s.dropped_lines),
        ("b\n", 3, 1)
    );
}

#[test]
fn output_buffer_matches_bounding_the_whole_stream_between_snapshots() {
    for limits in [head(3, 40), tail(3, 40), head(50, 25), tail(50, 25)] {
        let mut buffer = OutputBuffer::new(limits);
        let mut stream = String::new();
        for index in 0..300 {
            let chunk = if index % 7 == 0 {
                format!("{}\n", "é".repeat(index % 30))
            } else {
                format!("line {index}\n")
            };
            stream.push_str(&chunk);
            buffer.push(&chunk);
            if index % 5 != 4 {
                continue;
            }
            let expected = bound_output(&stream, limits);
            let s = buffer.snapshot();
            assert_eq!(s.text, expected.text);
            assert_eq!(s.dropped_bytes, expected.dropped_bytes as u64);
            assert_eq!(s.dropped_lines, expected.dropped_lines as u64);
        }
    }
}

#[test]
fn output_buffer_stops_storing_head_output_once_the_window_is_full() {
    let mut buffer = OutputBuffer::new(head(2, 1000));
    for index in 0..1000 {
        buffer.push(&format!("line {index}\n"));
    }
    assert!(buffer.stored_bytes() < 20);
    let s = buffer.snapshot();
    assert_eq!(
        (s.text.as_str(), s.dropped_bytes, s.dropped_lines),
        ("line 0\nline 1\n", 8876, 998)
    );
}

#[test]
fn output_buffer_stores_only_the_tail_window_after_each_snapshot() {
    let mut buffer = OutputBuffer::new(tail(3, 100));
    let mut stream = String::new();
    for index in 0..2000 {
        let chunk = format!("line {index}\n\n");
        stream.push_str(&chunk);
        buffer.push(&chunk);
        let s = buffer.snapshot();
        assert!(buffer.stored_bytes() <= 100);
        assert_eq!(s.text, bound_output(&stream, tail(3, 100)).text);
    }
}

#[test]
fn lossy_decoding_in_pieces_equals_decoding_at_once() {
    let inputs: [&[u8]; 6] = [
        "héllo €😀".as_bytes(),
        &[0xe2, 0x82, 0x41, 0xff, 0xc3],
        &[0xed, 0xa0, 0x80, 0x0a, 0xf4, 0x90, 0x80],
        &[0xf0, 0x9f, 0x98],
        &[0xef, 0xbb, 0xbf, 0x61],
        b"plain",
    ];
    for input in inputs {
        let whole = String::from_utf8_lossy(input).into_owned();
        for split in 0..=input.len() {
            let mut decoder = LossyDecoder::default();
            let mut text = decoder.decode(&input[..split]);
            text.push_str(&decoder.decode(&input[split..]));
            text.push_str(&decoder.finish());
            assert_eq!(text, whole, "{input:?} split at {split}");
        }
        // Byte by byte.
        let mut decoder = LossyDecoder::default();
        let mut text = String::new();
        for b in input {
            text.push_str(&decoder.decode(std::slice::from_ref(b)));
        }
        text.push_str(&decoder.finish());
        assert_eq!(text, whole);
    }
}

#[test]
fn stream_decoder_drops_only_a_leading_bom() {
    let mut decoder = StreamDecoder::default();
    assert_eq!(decoder.decode(&[0xef, 0xbb], false), "");
    assert_eq!(decoder.decode(&[0xbf, b'a'], false), "a");
    assert_eq!(decoder.decode("\u{feff}b".as_bytes(), false), "\u{feff}b");
    assert_eq!(decoder.decode(&[0xe2], true), "\u{fffd}");
}

// `env-truncate.test.ts`

#[test]
fn reports_utf8_byte_counts_in_truncation_results() {
    let content = "aé🙂\nb";
    let r = truncate_head(content, 10, 100);
    assert!(!r.truncated);
    assert_eq!(r.total_bytes, 9);
    assert_eq!(r.output_bytes, 9);
}

#[test]
fn does_not_count_a_trailing_newline_as_an_extra_line() {
    let r = truncate_head("line\nline\nline\n", 3, 100);
    assert!(!r.truncated);
    assert_eq!((r.total_lines, r.output_lines), (3, 3));
}

#[test]
fn truncates_head_by_line_limits() {
    let r = truncate_head("one\ntwo\nthree\nfour", 2, 100);
    assert_eq!(r.content, "one\ntwo");
    assert!(r.truncated);
    assert_eq!(r.truncated_by, Some(TruncatedBy::Lines));
    assert_eq!((r.total_lines, r.output_lines), (4, 2));
}

#[test]
fn reports_bytes_when_only_a_trailing_newline_exceeds_limits_at_the_line_cap() {
    let r = truncate_head("hello\nworld\n", 2, 11);
    assert_eq!(r.content, "hello\nworld");
    assert_eq!(r.truncated_by, Some(TruncatedBy::Bytes));
    assert_eq!((r.total_lines, r.output_lines), (2, 2));
}

#[test]
fn truncates_head_on_utf8_byte_limits_without_partial_lines() {
    let r = truncate_head("éé\nabc", 10, 4);
    assert_eq!(r.content, "éé");
    assert_eq!(r.truncated_by, Some(TruncatedBy::Bytes));
    assert_eq!(r.output_bytes, 4);
    assert!(!r.first_line_exceeds_limit);
    let r = truncate_head("éé\nabc", 10, 3);
    assert_eq!(r.content, "");
    assert!(r.first_line_exceeds_limit);
}

#[test]
fn formats_sizes() {
    assert_eq!(format_size(1023), "1023B");
    assert_eq!(format_size(1536), "1.5KB");
    assert_eq!(format_size(3 * 1024 * 1024), "3.0MB");
    assert_eq!(format_size(51_200), "50.0KB");
    assert_eq!(format_size(80_000), "78.1KB");
}

#[test]
fn formats_and_parses_numbers_like_javascript() {
    for (value, text) in [
        (0.0, "0"),
        (-0.0, "0"),
        (2.5, "2.5"),
        (0.01, "0.01"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (0.000001, "0.000001"),
        (123456789012345680000.0, "123456789012345680000"),
        (2147483.647, "2147483.647"),
        (-1.5e300, "-1.5e+300"),
        (100.0, "100"),
    ] {
        assert_eq!(js_number(value), text);
    }
    for (text, value) in [
        ("", Some(0.0)),
        ("  ", Some(0.0)),
        (" 3 ", Some(3.0)),
        ("0x10", Some(16.0)),
        ("0b11", Some(3.0)),
        ("0o17", Some(15.0)),
        ("-0x10", None),
        ("1e2", Some(100.0)),
        (".5", Some(0.5)),
        ("5.", Some(5.0)),
        ("+5", Some(5.0)),
        ("1_000", None),
        ("1e", None),
        ("abc", None),
    ] {
        assert_eq!(js_parse_number(text), value, "{text:?}");
    }
    assert_eq!(js_parse_number("Infinity"), Some(f64::INFINITY));
}

#[test]
fn diffs_large_rewrites_in_bounded_memory() {
    use crate::tools::diff::{diff_lines, unified_patch};
    let old: String = (0..4000).map(|i| format!("old line {i}\n")).collect();
    let new: String = (0..4000).map(|i| format!("new line {i}\n")).collect();
    let changes = diff_lines(&old, &new);
    assert_eq!(changes.len(), 2);
    assert!(changes[0].removed && changes[1].added);
    let patch = unified_patch("f", &old, &new, 4);
    assert!(patch.starts_with("--- f\n+++ f\n@@ -1,4000 +1,4000 @@\n-old line 0\n"));
}

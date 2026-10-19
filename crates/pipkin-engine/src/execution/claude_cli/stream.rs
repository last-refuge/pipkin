//! Phase 0, read-only projection of Claude CLI `stream-json` NDJSON.
//!
//! This is a synthetic contract, not evidence of live CLI compatibility or billing.
//! Only init/result identity and outcome, plus a few envelope counters, are projected.
//! Other fields (including messages, tool inputs, credentials and usage) are discarded
//! by serde rather than collected into a JSON value. No CLI text appears in errors.
//! The supervisor must bound total bytes/time and independently check process exit.
//!
//! LF delimits records; CRLF is accepted. The byte limit excludes LF but includes CR.
//! A complete final JSON object without LF is accepted; an incomplete one is not.
//! Empty lines, duplicate init/results, post-result content and malformed envelopes fail
//! closed. Trailing system telemetry is drained. Unknown envelopes never establish success.
//! This is the smoke/probe decoder, not a second validator in the provider runtime.

use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_LINE_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmokeMarker {
    Fresh,
    Resume,
}

#[derive(Debug, Default, Serialize)]
pub struct StreamSummary {
    pub init_seen: bool,
    pub declared_tools: Option<usize>,
    pub write_tool_available: bool,
    pub unexpected_write_fixture_tools: Option<usize>,
    /// ASCII hex UUID in 8-4-4-4-12 shape, normalized to lowercase; no version claim.
    pub session_id: Option<String>,
    pub result_seen: bool,
    pub result_success: bool,
    /// Only a fixed, nonsecret qualification marker; arbitrary answer text is discarded.
    pub smoke_marker: Option<SmokeMarker>,
    pub records: u64,
    pub unknown_records: u64,
    /// Envelope observations, not deduplicated messages or executed actions.
    pub assistant_records: u64,
    pub stream_event_records: u64,
    pub text_delta_records: u64,
}

/// Retains at most one bounded unfinished line, never a transcript or raw error.
/// Deliberately has no Debug/Serialize implementation exposing the pending bytes.
#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    summary: StreamSummary,
    failed: Option<&'static str>,
}

impl Decoder {
    pub fn feed(&mut self, mut bytes: &[u8]) -> Result<(), &'static str> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        while !bytes.is_empty() {
            let newline = bytes.iter().position(|&byte| byte == b'\n');
            let count = newline.unwrap_or(bytes.len());
            if count > MAX_LINE_BYTES - self.line.len() {
                return self.fail("NDJSON line exceeds byte limit");
            }
            // Avoid geometric growth beyond the per-line limit.
            self.line.reserve_exact(count);
            self.line.extend_from_slice(&bytes[..count]);
            if newline.is_none() {
                break;
            }
            if let Err(error) = self.consume_line() {
                return self.fail(error);
            }
            self.clear_line();
            bytes = &bytes[count + 1..];
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<StreamSummary, &'static str> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if !self.line.is_empty() {
            let result = self.consume_line();
            self.clear_line();
            result?;
        }
        // EOF alone establishes neither a result nor success.
        Ok(self.summary)
    }

    fn clear_line(&mut self) {
        self.line.fill(0);
        self.line.clear();
    }

    fn fail(&mut self, error: &'static str) -> Result<(), &'static str> {
        self.clear_line();
        self.summary.result_success = false;
        self.failed = Some(error);
        Err(error)
    }

    fn consume_line(&mut self) -> Result<(), &'static str> {
        // Validate even ignored text; serde's skipped fields must not bypass UTF-8.
        let line = std::str::from_utf8(&self.line).map_err(|_| "invalid NDJSON UTF-8")?;
        let header: Header = parse(line)?;
        if let Some(session_id) = &header.session_id {
            if let Some(previous) = &self.summary.session_id {
                if previous != session_id {
                    return Err("inconsistent session ID");
                }
            } else {
                self.summary.session_id = Some(session_id.clone());
            }
        }
        increment(&mut self.summary.records)?;
        if self.summary.result_seen {
            if header.kind == "system" && parse::<Subtype>(line)?.subtype != "init" {
                return increment(&mut self.summary.unknown_records);
            }
            return Err(if header.kind == "result" {
                "duplicate result record"
            } else {
                "record after terminal result"
            });
        }
        match header.kind.as_str() {
            "system" => {
                let system: Subtype = parse(line)?;
                if system.subtype == "init" {
                    if header.session_id.is_none() {
                        return Err("init requires a session ID");
                    }
                    if self.summary.init_seen {
                        return Err("duplicate init record");
                    }
                    self.summary.init_seen = true;
                    let init: InitFields = parse(line)?;
                    if let Some(tools) = init.tools {
                        self.summary.declared_tools = Some(tools.len());
                        self.summary.write_tool_available =
                            tools.iter().any(|tool| tool == "Write");
                        self.summary.unexpected_write_fixture_tools = Some(
                            tools
                                .iter()
                                .filter(|tool| {
                                    !matches!(tool.as_str(), "Write" | "EndConversation")
                                })
                                .count(),
                        );
                    }
                } else {
                    increment(&mut self.summary.unknown_records)?;
                }
            }
            "result" => {
                let result: ResultFields = parse(line)?;
                if header.session_id.is_none() {
                    return Err("result requires a session ID");
                }
                if result.subtype == "success" && !result.is_error {
                    if !self.summary.init_seen {
                        return Err("successful result requires prior init");
                    }
                    self.summary.result_success = true;
                    self.summary.smoke_marker = result.marker;
                }
                // Future result subtypes are observable, but never successful.
                if !matches!(
                    result.subtype.as_str(),
                    "success"
                        | "error_during_execution"
                        | "error_max_turns"
                        | "error_max_budget_usd"
                        | "error_max_structured_output_retries"
                ) {
                    increment(&mut self.summary.unknown_records)?;
                }
                self.summary.result_seen = true;
            }
            "assistant" => increment(&mut self.summary.assistant_records)?,
            "user" => {} // Observed envelope only; never retain message/tool results.
            "stream_event" => {
                let stream: StreamEvent = parse(line)?;
                increment(&mut self.summary.stream_event_records)?;
                match stream.event.kind.as_str() {
                    "content_block_delta" => {
                        let delta: DeltaEvent = parse(line)?;
                        if delta.event.delta.kind == "text_delta" {
                            // Validate text's type, without retaining its value.
                            let _: TextDeltaEvent = parse(line)?;
                            increment(&mut self.summary.text_delta_records)?;
                        } else if !matches!(
                            delta.event.delta.kind.as_str(),
                            "input_json_delta"
                                | "thinking_delta"
                                | "signature_delta"
                                | "citations_delta"
                        ) {
                            increment(&mut self.summary.unknown_records)?;
                        }
                    }
                    "message_start"
                    | "message_delta"
                    | "message_stop"
                    | "content_block_start"
                    | "content_block_stop"
                    | "ping" => {}
                    _ => increment(&mut self.summary.unknown_records)?,
                }
            }
            _ => increment(&mut self.summary.unknown_records)?,
        }
        Ok(())
    }
}

fn increment(counter: &mut u64) -> Result<(), &'static str> {
    *counter = counter.checked_add(1).ok_or("record counter overflow")?;
    Ok(())
}

fn parse<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, &'static str> {
    // Struct deserializers also accept positional JSON arrays. NDJSON envelopes
    // must instead be objects; only JSON's four whitespace bytes are permitted.
    if !line
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .starts_with('{')
    {
        return Err("NDJSON record must be an object");
    }
    // Never forward serde errors: they may quote arbitrary input.
    serde_json::from_str(line).map_err(|_| "malformed or truncated NDJSON record")
}

#[derive(Deserialize)]
struct Header {
    #[serde(rename = "type")]
    kind: String,
    // Missing is allowed for unknown/event envelopes; present null is not an ID.
    #[serde(default, deserialize_with = "session_id")]
    session_id: Option<String>,
}

fn session_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let id = String::deserialize(deserializer)?;
    let valid = id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !valid {
        return Err(serde::de::Error::custom("invalid session ID shape"));
    }
    Ok(Some(id.to_ascii_lowercase()))
}

#[derive(Deserialize)]
struct InitFields {
    #[serde(default)]
    tools: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Subtype {
    subtype: String,
}

#[derive(Deserialize)]
struct ResultFields {
    subtype: String,
    is_error: bool,
    // Optional text, but if present it must be text. Its contents are never kept.
    #[serde(rename = "result", default, deserialize_with = "smoke_marker")]
    marker: Option<SmokeMarker>,
}

fn smoke_marker<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SmokeMarker>, D::Error> {
    struct Marker;
    impl serde::de::Visitor<'_> for Marker {
        type Value = Option<SmokeMarker>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("text")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(match value.trim() {
                "PIPKIN_BRIDGE_OK" => Some(SmokeMarker::Fresh),
                "PIPKIN_BRIDGE_MEMORY_7Z9" => Some(SmokeMarker::Resume),
                _ => None,
            })
        }
    }
    deserializer.deserialize_str(Marker)
}

fn discard_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    struct Text;
    impl serde::de::Visitor<'_> for Text {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("text")
        }
        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
    }
    deserializer.deserialize_str(Text)
}

#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct StreamEvent {
    event: Kind,
}

#[derive(Deserialize)]
struct DeltaEvent {
    event: Delta,
}

#[derive(Deserialize)]
struct Delta {
    delta: Kind,
}

#[derive(Deserialize)]
struct TextDeltaEvent {
    #[serde(rename = "event")]
    _event: TextDelta,
}

#[derive(Deserialize)]
struct TextDelta {
    #[serde(rename = "delta")]
    _delta: Text,
}

#[derive(Deserialize)]
struct Text {
    #[serde(rename = "text", deserialize_with = "discard_text")]
    _text: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    // All input below is fabricated. No live CLI/account commands are involved.
    const ID: &str = "12345678-1234-4234-a234-123456789abc";
    const OTHER: &str = "87654321-4321-4321-b432-cba987654321";
    const SECRET: &str = "DO_NOT_RETAIN_secret_token_account_tool_text";

    #[test]
    fn smoke_answers_are_fixed_markers_not_arbitrary_retained_text() {
        for (answer, expected) in [
            ("PIPKIN_BRIDGE_OK", Some(SmokeMarker::Fresh)),
            (" PIPKIN_BRIDGE_MEMORY_7Z9 ", Some(SmokeMarker::Resume)),
            (SECRET, None),
        ] {
            let input = format!(
                "{}\n{}\n",
                init(),
                serde_json::json!({
                    "type": "result", "subtype": "success", "is_error": false,
                    "session_id": ID, "result": answer,
                })
            );
            let summary = decode(input.as_bytes()).unwrap();
            assert_eq!(summary.smoke_marker, expected);
            assert!(!serde_json::to_string(&summary).unwrap().contains(SECRET));
        }
        let absent =
            decode(format!("{}\n{}", init(), result("success", false)).as_bytes()).unwrap();
        assert_eq!(absent.smoke_marker, None);
    }

    #[test]
    fn write_fixture_inventory_is_explicit_and_does_not_include_shell_or_mcp() {
        for (tools, available, unexpected) in [
            (serde_json::json!(["Write"]), true, 0),
            (serde_json::json!(["Write", "EndConversation"]), true, 0),
            (serde_json::json!(["Write", "Bash", "mcp__secret"]), true, 2),
            (serde_json::json!([]), false, 0),
        ] {
            let input =
                serde_json::json!({"type":"system","subtype":"init","session_id":ID,"tools":tools})
                    .to_string();
            let summary = decode(input.as_bytes()).unwrap();
            assert_eq!(summary.write_tool_available, available);
            assert_eq!(summary.unexpected_write_fixture_tools, Some(unexpected));
            assert!(
                !serde_json::to_string(&summary)
                    .unwrap()
                    .contains("mcp__secret")
            );
        }
        assert_eq!(
            decode(init().as_bytes())
                .unwrap()
                .unexpected_write_fixture_tools,
            None
        );
    }

    #[test]
    fn init_projects_only_tool_count_not_names() {
        let input = serde_json::json!({"type": "system", "subtype": "init", "session_id": ID,
            "tools": [SECRET]})
        .to_string();
        let summary = decode(input.as_bytes()).unwrap();
        assert_eq!(summary.declared_tools, Some(1));
        assert!(!serde_json::to_string(&summary).unwrap().contains(SECRET));
    }

    fn init() -> String {
        format!(r#"{{"type":"system","subtype":"init","session_id":"{ID}"}}"#)
    }

    fn result(subtype: &str, is_error: bool) -> String {
        format!(
            r#"{{"type":"result","subtype":"{subtype}","is_error":{is_error},"session_id":"{ID}"}}"#
        )
    }

    fn decode(input: &[u8]) -> Result<StreamSummary, &'static str> {
        let mut decoder = Decoder::default();
        decoder.feed(input)?;
        decoder.finish()
    }

    fn transcript() -> String {
        format!(
            "{}\n{}\n{}\n{}\n",
            init(),
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hëllo 🦀"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hëllo 🦀"}]}}"#,
            result("success", false)
        )
    }

    #[test]
    fn every_chunk_boundary_including_utf8() {
        let input = transcript();
        for split in 0..=input.len() {
            let mut decoder = Decoder::default();
            decoder.feed(&input.as_bytes()[..split]).unwrap();
            decoder.feed(&[]).unwrap();
            decoder.feed(&input.as_bytes()[split..]).unwrap();
            let summary = decoder.finish().unwrap();
            assert!(summary.init_seen && summary.result_seen && summary.result_success);
            assert_eq!(summary.session_id.as_deref(), Some(ID));
            assert_eq!(summary.records, 4);
            assert_eq!(summary.unknown_records, 0);
            assert_eq!(summary.assistant_records, 1);
            assert_eq!(summary.stream_event_records, 1);
            assert_eq!(summary.text_delta_records, 1);
        }
        let mut decoder = Decoder::default();
        for byte in input.bytes() {
            decoder.feed(&[byte]).unwrap();
        }
        assert!(decoder.finish().unwrap().result_success);
    }

    #[test]
    fn accepts_crlf_and_complete_final_line_without_newline() {
        let input = format!("{}\r\n{}", init(), result("success", false));
        assert!(decode(input.as_bytes()).unwrap().result_success);
    }

    #[test]
    fn exact_line_limit_and_oversize_are_chunk_independent() {
        let prefix = r#"{"type":"future","ignored":""#;
        let mut line = prefix.to_owned();
        line.push_str(&"x".repeat(MAX_LINE_BYTES - prefix.len() - 2));
        line.push_str("\"}");
        assert_eq!(line.len(), MAX_LINE_BYTES);
        assert_eq!(decode(line.as_bytes()).unwrap().unknown_records, 1);
        line.push('\n');
        assert_eq!(decode(line.as_bytes()).unwrap().records, 1);
        let mut decoder = Decoder::default();
        decoder.feed(&vec![b' '; MAX_LINE_BYTES]).unwrap();
        assert!(decoder.line.capacity() <= MAX_LINE_BYTES);
        let error = decoder.feed(b" ").unwrap_err();
        assert_eq!(error, "NDJSON line exceeds byte limit");
        assert!(decoder.line.is_empty());
        assert_eq!(decoder.feed(init().as_bytes()), Err(error));
        assert_eq!(decoder.finish().unwrap_err(), error);
        assert!(decode(&vec![b'x'; MAX_LINE_BYTES + 1]).is_err());
    }

    #[test]
    fn total_input_is_not_the_line_bound() {
        let line = b"{\"type\":\"future\"}\n";
        let count = MAX_LINE_BYTES / line.len() + 1;
        let summary = decode(&line.repeat(count)).unwrap();
        assert_eq!(summary.records, count as u64);
    }

    #[test]
    fn malformed_and_truncated_records_fail_without_echoing_input() {
        for input in [
            "{",
            "{\"type\":",
            "[]",
            "[\"future\"]",
            "[\"future\",\"12345678-1234-4234-a234-123456789abc\"]",
            "null",
            "1",
            "{}",
            "\n",
            " \n",
            "{\"type\":7}",
            "{\"type\":\"future\"} trailing",
            "{\"type\":\"future\"}{\"type\":\"future\"}",
            "{\"type\":\"future\",\"secret\":\"unfinished",
            "{\"type\":\"result\",\"type\":\"future\"}",
        ] {
            assert!(decode(input.as_bytes()).is_err(), "{input}");
        }
        let input = format!("{}\n{{\"type\":\"result\"", init());
        assert!(decode(input.as_bytes()).is_err());
        assert!(decode(b"{\"type\":\"future\",\"ignored\":\"\xff\"}\n").is_err());
        assert!(decode(b"{\"type\":\"future\",\"ignored\":\"\xf0\x9f").is_err());
    }

    #[test]
    fn no_result_is_not_success() {
        for input in [String::new(), init(), format!("{}\n", init())] {
            let summary = decode(input.as_bytes()).unwrap();
            assert!(!summary.result_seen && !summary.result_success);
        }
        assert!(decode(result("success", false).as_bytes()).is_err());
    }

    #[test]
    fn error_or_future_result_never_succeeds() {
        for subtype in [
            "error_during_execution",
            "error_max_turns",
            "error_max_budget_usd",
            "error_max_structured_output_retries",
            "future_outcome",
        ] {
            for is_error in [true, false] {
                let input = format!("{}\n{}", init(), result(subtype, is_error));
                let summary = decode(input.as_bytes()).unwrap();
                assert!(summary.result_seen && !summary.result_success);
            }
        }
        let input = format!("{}\n{}", init(), result("success", true));
        assert!(!decode(input.as_bytes()).unwrap().result_success);
        // Early CLI failure may have no init; it cannot establish success.
        let summary = decode(result("error_during_execution", true).as_bytes()).unwrap();
        assert!(summary.result_seen && !summary.init_seen && !summary.result_success);
    }

    #[test]
    fn result_fields_have_strict_types_and_required_presence() {
        let good = serde_json::from_str::<serde_json::Value>(&result("success", false)).unwrap();
        for field in ["subtype", "is_error", "session_id"] {
            for bad in [
                serde_json::Value::Null,
                serde_json::json!([]),
                serde_json::json!(7),
            ] {
                let mut value = good.clone();
                value[field] = bad;
                assert!(decode(format!("{}\n{value}", init()).as_bytes()).is_err());
            }
            let mut value = good.clone();
            value.as_object_mut().unwrap().remove(field);
            assert!(decode(format!("{}\n{value}", init()).as_bytes()).is_err());
        }
        let mut value = good.clone();
        value["is_error"] = serde_json::json!("false");
        assert!(decode(format!("{}\n{value}", init()).as_bytes()).is_err());
        for bad in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!(false),
        ] {
            let mut value = good.clone();
            value["result"] = bad;
            assert!(decode(format!("{}\n{value}", init()).as_bytes()).is_err());
        }
    }

    #[test]
    fn session_shape_mismatch_and_duplicate_init() {
        for bad in [
            SECRET,
            "",
            "1234567812344234a234123456789abc",
            "12345678_1234-4234-a234-123456789abc",
            "12345678-1234-4234-a234-123456789abz",
        ] {
            assert!(decode(init().replace(ID, bad).as_bytes()).is_err());
        }
        for record in [
            result("success", false),
            init(),
            format!(r#"{{"type":"future","session_id":"{ID}"}}"#),
        ] {
            let input = format!("{}\n{}", init(), record.replace(ID, OTHER));
            assert_eq!(
                decode(input.as_bytes()).unwrap_err(),
                "inconsistent session ID"
            );
        }
        assert!(decode(format!("{}\n{}", init(), init()).as_bytes()).is_err());
        let input = format!(
            "{}\n{}",
            init().replace(ID, &ID.to_uppercase()),
            result("success", false)
        );
        assert_eq!(
            decode(input.as_bytes()).unwrap().session_id.as_deref(),
            Some(ID)
        );
        assert!(decode(br#"{"type":"system","subtype":"init","session_id":null}"#).is_err());
        assert!(decode(br#"{"type":"system","subtype":"init"}"#).is_err());
    }

    #[test]
    fn duplicate_result_and_trailing_records_poison_decoder() {
        for trailing in [
            result("success", false),
            "{\"type\":\"future\"}".to_owned(),
            "{".to_owned(),
        ] {
            let input = format!("{}\n{}\n{trailing}\n", init(), result("success", false));
            let mut decoder = Decoder::default();
            let error = decoder.feed(input.as_bytes()).unwrap_err();
            assert_eq!(decoder.feed(&[]), Err(error));
            assert_eq!(decoder.finish().unwrap_err(), error);
        }
    }

    #[test]
    fn unknown_records_are_counted_not_interpreted() {
        let input = concat!(
            "{\"type\":\"future\",\"result\":\"success\",\"is_error\":false}\n",
            "{\"type\":\"system\",\"subtype\":\"future\"}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"future\"}}\n"
        );
        let summary = decode(input.as_bytes()).unwrap();
        assert_eq!(summary.records, 3);
        assert_eq!(summary.unknown_records, 3);
        assert!(!summary.result_seen && !summary.result_success);
    }

    #[test]
    fn ignored_secrets_never_reach_summary_debug_json_or_errors() {
        let input = format!(
            "{}\n{}\n{}\n{}\n",
            serde_json::json!({"type":"system", "subtype":"init", "session_id":ID,
                "apiKeySource":SECRET, "account":{"email":SECRET}, "token":SECRET}),
            serde_json::json!({"type":"assistant", "message":{"content":[
                {"type":"text", "text":SECRET}, {"type":"tool_use", "input":{"token":SECRET}}
            ]}}),
            serde_json::json!({"type":"future", "unknown":SECRET, "stderr":SECRET}),
            serde_json::json!({"type":"result", "subtype":"success", "is_error":false,
                "session_id":ID, "result":SECRET, "usage":{"unknown":SECRET},
                "permission_denials":[{"tool_input":SECRET}]})
        );
        let mut decoder = Decoder::default();
        decoder.feed(input.as_bytes()).unwrap();
        assert!(decoder.line.is_empty());
        let summary = decoder.finish().unwrap();
        assert!(summary.result_success);
        assert!(!serde_json::to_string(&summary).unwrap().contains(SECRET));
        assert!(!format!("{summary:?}").contains(SECRET));
        let bad = format!(r#"{{"type":"result","is_error":"{SECRET}"}}"#);
        assert!(!decode(bad.as_bytes()).unwrap_err().contains(SECRET));
        assert!(
            !decode(init().replace(ID, SECRET).as_bytes())
                .unwrap_err()
                .contains(SECRET)
        );
    }
}

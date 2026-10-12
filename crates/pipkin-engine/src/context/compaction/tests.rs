// Range selection and serialization cases ported from durable/test/harness-compaction.test.ts
// ("range selection", "serialization"), adapted to Pipkin entries (no positional system
// entries; "omit" edits are retirement). Estimates follow ai/src/utils/estimate.ts.

use pipkin_engine_protocol::{Block, EntryKind, Message, StopReason};
use serde_json::{Map, json};

use super::*;
use crate::context::test_support::*;

/// Message entries with ids 1.. in order.
fn entries(messages: Vec<Message>) -> Vec<Entry> {
    messages
        .into_iter()
        .enumerate()
        .map(|(index, m)| message(index as u64 + 1, m))
        .collect()
}

fn cut_index(entries: &[Entry], keep: u64) -> Option<usize> {
    select_cut(entries, keep).map(|id| entries.iter().position(|e| e.id == id).unwrap())
}

#[test]
fn context_estimates_and_ordering_match_executed_pi_reference() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/native-context-estimate/reference.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let entries: Vec<Entry> = serde_json::from_value(case["entries"].clone()).unwrap();
        let extra: Vec<Message> = serde_json::from_value(case["extra"].clone()).unwrap();
        let ordered: Vec<Message> = serde_json::from_value(case["ordered"].clone()).unwrap();
        assert_eq!(
            estimate_context(&entries, &extra),
            case["expected"].as_u64().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            derive::model_messages(&entries),
            ordered,
            "{}",
            case["name"]
        );
    }
}

#[test]
fn estimates_borrowed_history_after_measured_usage_with_repeated_and_missing_results() {
    let repeated = tool_result("same", &"x".repeat(65536));
    let e = entries(vec![
        user(&"old history".repeat(65536)),
        assistant_with(
            "calls",
            &["same", "same", "missing"],
            StopReason::ToolUse,
            123,
        ),
        repeated,
        tool_result("same", "ignored duplicate"),
        user("next"),
    ]);
    let extra = user("extra");
    let missing_tokens = estimate_message_tokens(&derive::missing_result("missing", "read", 0));
    assert_eq!(
        estimate_context(&e, &[extra]),
        123 + 2 * 16384 + missing_tokens + 1 + 2
    );
    // Without measured usage the whole retained range is still estimated, with exactly
    // the same derived ordering and missing-result semantics as the model context.
    let mut unmeasured = e;
    if let EntryKind::Message {
        message: Message::Assistant { usage, .. },
        ..
    } = &mut unmeasured[1].kind
    {
        *usage = Default::default();
    }
    assert_eq!(
        context_tokens(&unmeasured),
        estimate_tokens(&derive::model_messages(&unmeasured))
    );
}

#[test]
fn keeps_about_keep_recent_tokens_and_cuts_at_the_first_candidate_after_the_budget() {
    let e = entries(vec![
        user(&text("1", 10)),
        assistant("2", &["c1"]),
        tool_result("c1", &text("3", 3000)),
        assistant(&text("4", 10), &[]),
        user(&text("5", 10)),
        assistant(&text("6", 10), &[]),
    ]);
    assert_eq!(cut_index(&e, 2000), Some(3));
}

#[test]
fn cuts_at_a_user_entry() {
    let e = entries(vec![
        user(&text("u1", 100)),
        assistant(&text("a1", 100), &[]),
        user(&text("u2", 100)),
        assistant(&text("a2", 100), &[]),
    ]);
    assert_eq!(cut_index(&e, 150), Some(2));
}

#[test]
fn cuts_at_an_assistant_in_one_long_run_and_never_at_a_tool_result() {
    let mut messages = vec![user("do it")];
    for index in 0..5 {
        let call = format!("c{index}");
        messages.push(assistant(&format!("step {index}"), &[call.as_str()]));
        messages.push(tool_result(&call, &text(&format!("r{index}"), 100)));
    }
    let e = entries(messages);
    let cut = cut_index(&e, 150).unwrap();
    assert!(matches!(
        &e[cut].kind,
        EntryKind::Message {
            message: Message::Assistant { .. },
            ..
        }
    ));
    assert_eq!(cut, 9);
}

#[test]
fn keeps_a_huge_last_tool_result_with_its_assistant() {
    let e = entries(vec![
        user("u"),
        assistant("a", &["c"]),
        tool_result("c", &text("big", 5000)),
    ]);
    assert_eq!(cut_index(&e, 100), Some(1));
}

#[test]
fn never_cuts_at_a_display_entry_or_an_excluded_answer() {
    let mut e = entries(vec![
        user(&text("u1", 100)),
        assistant(&text("a1", 100), &[]),
        user(&text("u2", 100)),
        assistant(&text("a2", 100), &[]),
    ]);
    e.push(notice(5));
    e.push(message(
        6,
        assistant_with(&text("err", 100), &[], StopReason::Error, 0),
    ));
    e.push(message(
        7,
        assistant_with(&text("stopped", 100), &[], StopReason::Aborted, 0),
    ));
    // A zero budget is reached at the newest entry; no candidate follows it, so the cut is
    // the last candidate, a2.
    assert_eq!(cut_index(&e, 0), Some(3));
}

#[test]
fn a_retired_entry_adds_nothing_and_is_no_candidate() {
    let mut e = entries(vec![
        user(&text("u1", 100)),
        assistant(&text("a1", 100), &[]),
        user(&text("u2", 100)),
        assistant(&text("a2", 100), &[]),
    ]);
    assert_eq!(cut_index(&e, 150), Some(2));
    e[2].retired = true;
    assert_eq!(cut_index(&e, 150), Some(1));
}

#[test]
fn does_not_cut_at_a_user_entry_that_a_result_of_the_preceding_call_follows() {
    let e = entries(vec![
        user(&text("u1", 100)),
        assistant("a", &["c"]),
        user(&text("steer", 100)),
        tool_result("c", &text("r", 100)),
        assistant(&text("a2", 100), &[]),
    ]);
    assert_eq!(cut_index(&e, 250), Some(4));
}

#[test]
fn finds_nothing_when_the_budget_is_never_reached_or_only_the_marker_precedes() {
    let small = entries(vec![user("hi"), assistant("hello", &[])]);
    assert_eq!(select_cut(&small, 150), None);
    let with_marker = vec![
        compaction(10, "summary", 1),
        message(1, user(&text("u", 200))),
    ];
    assert_eq!(select_cut(&with_marker, 150), None);
}

#[test]
fn summarizes_an_earlier_summary_marker_first() {
    let mut e = vec![compaction(10, "EARLIER", 1)];
    e.extend(entries(vec![
        user(&text("u1", 100)),
        assistant(&text("a1", 100), &[]),
        user(&text("u2", 100)),
        assistant(&text("a2", 100), &[]),
    ]));
    assert_eq!(cut_index(&e, 150), Some(3));
    let first_kept = select_cut(&e, 150).unwrap();
    let source = summary_source(&e, first_kept);
    assert_eq!(source.len(), 3);
    assert!(
        serialize_for_summary(&source).starts_with(
            "[User]: The conversation history before this point was compacted into the following summary:\n\n<summary>\nEARLIER\n</summary>"
        )
    );
    assert!(summary_source(&e, 999).is_empty());
}

#[test]
fn serializes_a_transcript_and_truncates_tool_results() {
    let call = Block::tool_call("c", "read", json!({"path": "a.ts"}));
    let messages = vec![
        user("hello"),
        Message::Assistant {
            content: vec![
                Block::Thinking {
                    thinking: "hmm".into(),
                    signature: None,
                    redacted: false,
                    extra: Map::new(),
                },
                Block::text("sure"),
                call,
            ],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            extra: Map::new(),
        },
        tool_result("c", &"y".repeat(2500)),
    ];
    let serialized = serialize_for_summary(&messages);
    assert_eq!(
        serialized,
        format!(
            "[User]: hello\n\n[Assistant thinking]: hmm\n\n[Assistant]: sure\n\n[Assistant tool calls]: read(path=\"a.ts\")\n\n[Tool result]: {}\n\n[... 500 more characters truncated]",
            "y".repeat(2000)
        )
    );
    // Empty parts are skipped; several calls are joined by "; ".
    let calls = Message::Assistant {
        content: vec![
            Block::tool_call("1", "a", json!({"x": 1, "y": [true]})),
            Block::tool_call("2", "b", json!({})),
        ],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: Default::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
        extra: Map::new(),
    };
    assert_eq!(
        serialize_for_summary(&[user(""), calls]),
        "[Assistant tool calls]: a(x=1, y=[true]); b()"
    );
}

#[test]
fn summary_request_text_is_exact() {
    let prompt = summary_request(&[user("u1")], Some("focus on files"));
    assert!(prompt.starts_with("<conversation>\n[User]: u1\n</conversation>\n\nThe messages above are a conversation to summarize."));
    assert!(prompt.contains("## Goal\n[What is the user trying to accomplish?"));
    assert!(prompt.contains("### Done\n- [x] [Completed tasks/changes]"));
    assert!(prompt.contains("- **[Decision]**: [Brief rationale]"));
    assert!(prompt.ends_with(
        "Keep each section concise. Preserve exact file paths, function names, and error messages.\n\nAdditional focus: focus on files"
    ));
    assert_eq!(summarization_prompt(None), SUMMARIZATION_PROMPT);
    assert!(SUMMARIZATION_SYSTEM_PROMPT.ends_with("ONLY output the structured summary."));
    assert_eq!(
        summary_message_text("S"),
        "The conversation history before this point was compacted into the following summary:\n\n<summary>\nS\n</summary>"
    );
}

#[test]
fn message_estimates_follow_the_reference_rules() {
    let with_image = Message::User {
        content: vec![Block::text("abcd"), Block::image("image/png", "AAAA")],
        timestamp: 0,
        extra: Map::new(),
    };
    assert_eq!(estimate_message_tokens(&with_image), 1201);
    // name (4) + `{"path":"a.ts"}` (15) + text (1) = 20 chars.
    let mut call = assistant("x", &[]);
    if let Message::Assistant { content, .. } = &mut call {
        content.push(Block::tool_call("c", "read", json!({"path": "a.ts"})));
    }
    assert_eq!(estimate_message_tokens(&call), 5);
    // UTF-16 units: one emoji is two.
    assert_eq!(estimate_message_tokens(&user("🎉🎉🎉")), 2);
    assert_eq!(estimate_message_tokens(&tool_result("c", "abcde")), 2);
    assert_eq!(estimate_tokens(&[user("abcd"), user("abcde")]), 3);
    assert_eq!(estimate_text_tokens(""), 0);
}

#[test]
fn context_tokens_use_the_newest_usage_after_the_head_plus_later_estimates() {
    // Without usage: every derived message is estimated.
    let plain = entries(vec![
        user(&"x".repeat(400)),
        assistant(&"y".repeat(40), &[]),
    ]);
    assert_eq!(context_tokens(&plain), 110);

    // A kept assistant from before the compaction does not measure the new prefix.
    let summary_tokens = estimate_message_tokens(&user(&summary_message_text("S")));
    let mut e = vec![
        compaction(10, "S", 1),
        message(1, user("kept")),
        message(2, assistant_with("old", &[], StopReason::Stop, 9_500)),
        message(11, user(&"x".repeat(4000))),
    ];
    assert_eq!(context_tokens(&e), summary_tokens + 1 + 1 + 1000);

    // A response after the marker measures everything before it.
    e.push(message(
        12,
        assistant_with("new", &[], StopReason::Stop, 2_000),
    ));
    e.push(message(13, user("tail")));
    assert_eq!(context_tokens(&e), 2_001);
    // Planned extra messages are added.
    assert_eq!(estimate_context(&e, &[user("abcdefgh")]), 2_003);

    // Aborted answers never measure (they are not in the context).
    e.push(message(
        14,
        assistant_with("stopped", &[], StopReason::Aborted, 50_000),
    ));
    assert_eq!(context_tokens(&e), 2_001);
}

#[test]
fn context_tokens_count_reordered_and_synthesized_results_after_the_measured_call() {
    let e = entries(vec![
        user("go"),
        assistant_with("calling", &["a", "b"], StopReason::ToolUse, 500),
        user(&"s".repeat(8)),
        tool_result("a", &"r".repeat(40)),
    ]);
    let missing = estimate_message_tokens(&crate::context::derive::missing_result("b", "read", 0));
    // After the call: result a (10), synthesized b, then the steer (2).
    assert_eq!(context_tokens(&e), 500 + 10 + missing + 2);
}

#[test]
fn thresholds_and_triggers() {
    let settings = CompactionSettings::default();
    assert_eq!(thresholds(200_000, &settings), (183_616, 150_848));
    assert_eq!(thresholds(10_000, &settings), (-6_384, -39_152));
    assert_eq!(
        threshold_compaction(183_617, 200_000, &settings),
        Some(Threshold::Blocking)
    );
    assert_eq!(
        threshold_compaction(183_616, 200_000, &settings),
        Some(Threshold::Background)
    );
    assert_eq!(threshold_compaction(150_848, 200_000, &settings), None);
    assert_eq!(threshold_compaction(1, 0, &settings), None);
    let disabled = CompactionSettings {
        enabled: false,
        ..settings.clone()
    };
    assert_eq!(threshold_compaction(999_999, 200_000, &disabled), None);
    let no_background = CompactionSettings {
        background_tokens: 0,
        ..settings
    };
    assert_eq!(threshold_compaction(183_000, 200_000, &no_background), None);
    assert_eq!(summary_max_tokens(1000, 0), 800);
    assert_eq!(summary_max_tokens(1000, 500), 500);
    assert_eq!(summary_max_tokens(16_384, 64_000), 13_107);
}

fn reply(content: Vec<Block>, stop: StopReason, error: Option<&str>) -> Message {
    Message::Assistant {
        content,
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: Default::default(),
        stop_reason: stop,
        error_message: error.map(str::to_string),
        timestamp: 0,
        extra: Map::new(),
    }
}

#[test]
fn validates_summaries_with_the_reference_failure_texts() {
    assert_eq!(
        validate_summary(&reply(
            vec![Block::text("  ## Goal"), Block::text("x  ")],
            StopReason::Stop,
            None
        )),
        Ok("## Goal\nx".to_string())
    );
    assert_eq!(
        validate_summary(&reply(vec![], StopReason::Error, Some("529: overloaded"))),
        Err("Summarization failed: 529: overloaded".to_string())
    );
    assert_eq!(
        validate_summary(&reply(vec![], StopReason::Aborted, None)),
        Err("Summarization failed: aborted".to_string())
    );
    assert_eq!(
        validate_summary(&reply(
            vec![Block::text("partial")],
            StopReason::Length,
            None
        )),
        Err("Summarization hit the token limit; the summary is incomplete".to_string())
    );
    assert_eq!(
        validate_summary(&reply(
            vec![Block::text("s"), Block::tool_call("c", "read", json!({}))],
            StopReason::Stop,
            None
        )),
        Err("Summarization attempted to call a tool".to_string())
    );
    assert_eq!(
        validate_summary(&reply(vec![Block::text("  ")], StopReason::Stop, None)),
        Err("Summarization produced no text".to_string())
    );
    assert_eq!(
        validate_summary(&user("x")),
        Err("Summarization produced no text".to_string())
    );
}

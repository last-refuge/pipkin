// Ported from durable/test/harness-context.test.ts (where the cases apply to Pipkin's entry
// model) plus retirement and streaming cases specific to Pipkin.

use pipkin_engine_protocol::{EntryKind, InputMode, Message, StopReason};
use serde_json::json;

use super::*;
use crate::context::test_support::*;

fn described(entries: &[Entry]) -> Vec<String> {
    model_messages(entries).iter().map(describe).collect()
}

#[test]
fn whole_transcript_without_a_head_and_display_entries_contribute_nothing() {
    let entries = vec![
        message(1, user("hi")),
        notice(2),
        entry(
            3,
            EntryKind::ModelChange {
                model: "p/m".into(),
            },
        ),
        entry(
            4,
            EntryKind::ThinkingChange {
                level: "high".into(),
            },
        ),
        entry(
            5,
            EntryKind::Custom {
                custom_type: "ext".into(),
                data: json!({"x": 1}),
            },
        ),
        message(6, assistant("hello", &[])),
    ];
    assert_eq!(described(&entries), ["user:hi", "assistant:hello"]);
}

#[test]
fn excludes_aborted_and_error_assistants_but_keeps_length() {
    let entries = vec![
        message(1, user("q")),
        message(2, assistant_with("partial", &[], StopReason::Aborted, 0)),
        message(3, assistant_with("failed", &[], StopReason::Error, 0)),
        message(4, assistant_with("done", &[], StopReason::Length, 0)),
    ];
    assert_eq!(described(&entries), ["user:q", "assistant:done"]);
}

#[test]
fn a_compaction_head_contributes_its_wrapped_summary_first() {
    let entries = vec![
        compaction(9, "SUMMARY", 5),
        message(5, user("u3")),
        message(6, assistant("a3", &[])),
    ];
    let messages = model_messages(&entries);
    assert_eq!(
        messages[0].text(),
        "The conversation history before this point was compacted into the following summary:\n\n<summary>\nSUMMARY\n</summary>"
    );
    assert!(matches!(messages[0], Message::User { timestamp: 90, .. }));
    assert_eq!(
        messages[1..].iter().map(describe).collect::<Vec<_>>(),
        ["user:u3", "assistant:a3"]
    );
}

#[test]
fn a_reset_head_contributes_its_handoff_or_nothing() {
    let with = vec![
        entry(
            4,
            EntryKind::Reset {
                handoff: Some(user("fresh start")),
            },
        ),
        message(5, assistant("after reset", &[])),
    ];
    assert_eq!(
        described(&with),
        ["user:fresh start", "assistant:after reset"]
    );
    let without = vec![
        entry(4, EntryKind::Reset { handoff: None }),
        message(5, user("next")),
    ];
    assert_eq!(described(&without), ["user:next"]);
}

#[test]
fn only_the_first_entry_is_a_head_marker() {
    // Older head markers drop out of the active range (selectActive).
    let entries = vec![
        compaction(9, "NEW", 5),
        message(5, user("kept")),
        compaction(7, "OLD", 1),
        entry(
            8,
            EntryKind::Reset {
                handoff: Some(user("old handoff")),
            },
        ),
        message(10, user("tail")),
    ];
    let messages = model_messages(&entries);
    assert!(messages[0].text().contains("NEW"));
    assert_eq!(
        messages[1..].iter().map(describe).collect::<Vec<_>>(),
        ["user:kept", "user:tail"]
    );
}

#[test]
fn orders_tool_results_by_call_order_and_drops_strays() {
    let entries = vec![
        message(1, user("run tools")),
        message(2, assistant("calling", &["b", "a"])),
        message(3, tool_result("a", "result a")),
        message(4, user("steer")),
        message(5, tool_result("b", "result b")),
        message(6, tool_result("zz", "stray")),
        message(7, assistant("done", &[])),
    ];
    assert_eq!(
        described(&entries),
        [
            "user:run tools",
            "assistant:calling",
            "result:b:result b",
            "result:a:result a",
            "user:steer",
            "assistant:done",
        ]
    );
}

#[test]
fn synthesizes_missing_results_with_the_reference_shape() {
    let mut call = assistant("calling", &["x", "y"]);
    if let Message::Assistant { timestamp, .. } = &mut call {
        *timestamp = 42;
    }
    let entries = vec![
        message(1, user("go")),
        message(2, call),
        message(3, tool_result("y", "y done")),
    ];
    let messages = model_messages(&entries);
    assert_eq!(
        messages.iter().map(describe).collect::<Vec<_>>(),
        [
            "user:go",
            "assistant:calling",
            "result:x:error",
            "result:y:y done"
        ]
    );
    assert_eq!(
        serde_json::to_value(&messages[2]).unwrap(),
        json!({
            "role": "toolResult",
            "toolCallId": "x",
            "toolName": "read",
            "content": [{"type": "text", "text": MISSING_RESULT_TEXT}],
            "isError": true,
            "timestamp": 42,
            "details": {"reason": "missing_result"}
        })
    );
    assert_eq!(
        MISSING_RESULT_TEXT,
        "Tool result unavailable: history ends before this call completed."
    );
}

#[test]
fn results_after_the_next_assistant_do_not_count_and_the_first_duplicate_wins() {
    let entries = vec![
        message(1, assistant("one", &["c"])),
        message(2, tool_result("c", "first")),
        message(3, tool_result("c", "second")),
        message(4, assistant("two", &["d"])),
        message(5, assistant("three", &[])),
        message(6, tool_result("d", "late")),
    ];
    assert_eq!(
        described(&entries),
        [
            "assistant:one",
            "result:c:first",
            "assistant:two",
            "result:d:error",
            "assistant:three",
        ]
    );
}

#[test]
fn retired_and_streaming_entries_leave_the_context() {
    // A stopped run: its instruction and partial reply are retired; a completed tool call and
    // its result stay (the stop retires the instruction, not completed effects).
    let mut stopped = message(3, user("stopped instruction"));
    stopped.retired = true;
    let mut partial_reply = message(6, assistant_with("partial", &[], StopReason::Aborted, 0));
    partial_reply.retired = true;
    let streaming = entry(
        8,
        EntryKind::Message {
            message: assistant("streaming", &[]),
            mode: InputMode::Prompt,
            request: None,
            partial: true,
        },
    );
    let entries = vec![
        message(1, user("first")),
        message(2, assistant("ok", &[])),
        stopped,
        message(4, assistant("calling", &["c"])),
        message(5, tool_result("c", "done")),
        partial_reply,
        message(7, user("next")),
        streaming,
    ];
    assert_eq!(
        described(&entries),
        [
            "user:first",
            "assistant:ok",
            "assistant:calling",
            "result:c:done",
            "user:next",
        ]
    );
}

#[test]
fn a_retired_head_contributes_nothing() {
    let mut head = compaction(9, "S", 5);
    head.retired = true;
    assert_eq!(described(&[head, message(5, user("u"))]), ["user:u"]);
}

#[test]
fn empty_input_is_an_empty_context() {
    assert!(model_messages(&[]).is_empty());
}

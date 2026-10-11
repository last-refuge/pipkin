//! `run`: ports of `harness-tools.test.ts` "tool results" and "tool progress and lifetime".

use pipkin_engine_protocol::Block;
use pipkin_engine_protocol::limits::MAX_RETAINED_OUTPUT_BYTES;
use serde_json::{Value, json};

use super::{ctx, result_text, temp};
use crate::provider::{BoxFuture, ToolSpec};
use crate::tools::{
    Diagnostic, OutputLimits, Retain, Severity, Tool, ToolCtx, ToolError, ToolOutput, aborted, run,
    unavailable,
};

type Body = dyn Fn(&ToolCtx) -> Result<ToolOutput, ToolError> + Send + Sync;

/// A test tool: `{text?: string}` parameters and a synchronous body.
struct Fake {
    limits: OutputLimits,
    body: Box<Body>,
}

fn fake(
    limits: OutputLimits,
    body: impl Fn(&ToolCtx) -> Result<ToolOutput, ToolError> + Send + Sync + 'static,
) -> Fake {
    Fake {
        limits,
        body: Box::new(body),
    }
}

impl Tool for Fake {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fake".into(),
            description: "test".into(),
            parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        }
    }
    fn limits(&self) -> OutputLimits {
        self.limits
    }
    fn prepare_arguments(&self, args: Value) -> Value {
        match args {
            Value::String(text) => json!({ "text": text }),
            other => other,
        }
    }
    fn execute<'a>(
        &'a self,
        _args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move { (self.body)(ctx) })
    }
}

fn limits(max_lines: usize, retain: Retain) -> OutputLimits {
    OutputLimits {
        max_lines,
        retain,
        ..OutputLimits::default()
    }
}

#[tokio::test]
async fn uses_retained_output_with_diagnostics_in_order() {
    let dir = temp();
    let tool = fake(limits(2, Retain::Head), |ctx| {
        ctx.output("line 1\n");
        ctx.output("line 2\nline 3\n");
        Ok(ToolOutput {
            details: Some(json!({"step": 2})),
            diagnostics: vec![
                Diagnostic::new(Severity::Info, "", "from api"),
                Diagnostic::new(Severity::Warn, "", "from result"),
            ],
            ..ToolOutput::default()
        })
    });
    let result = run(&tool, json!({}), &ctx(dir.path())).await;
    assert_eq!(result.details, Some(json!({"step": 2})));
    assert_eq!(
        result_text(&result),
        "line 1\nline 2\n|<harness>\n[info] from api\n[warn] from result\n[warn] Output truncated to its beginning: 1 lines, 7 bytes dropped\n</harness>"
    );
    assert_eq!(result.retained, "line 1\nline 2\nline 3\n");
    assert_eq!(result.dropped_bytes, 0);
}

#[tokio::test]
async fn bounds_explicit_text_content_and_keeps_other_content() {
    let dir = temp();
    let tool = fake(limits(2, Retain::Tail), |_| {
        Ok(ToolOutput {
            content: Some(vec![
                Block::text("a\nb\n"),
                Block::image("image/png", "AAAA"),
                Block::text("c\nd\n"),
            ]),
            ..ToolOutput::default()
        })
    });
    let result = run(&tool, json!({}), &ctx(dir.path())).await;
    assert_eq!(
        result_text(&result),
        "[image]|c\nd\n|<harness>\n[warn] Output truncated to its end: 2 lines, 4 bytes dropped\n</harness>"
    );
    assert_eq!(result.retained, "a\nb\nc\nd\n");
}

#[tokio::test]
async fn turns_a_failure_into_a_tool_error_result_with_the_partial_output() {
    let dir = temp();
    let tool = fake(OutputLimits::default(), |ctx| {
        ctx.output("partial\n");
        Err(ToolError::new("boom"))
    });
    let result = run(&tool, json!({}), &ctx(dir.path())).await;
    assert!(result.is_error);
    assert_eq!(
        result_text(&result),
        "partial\n|<harness>\n[error] boom\n</harness>"
    );
    assert_eq!(result.retained, "partial\n");
}

#[tokio::test]
async fn applies_the_default_output_limits() {
    let dir = temp();
    let tool = fake(OutputLimits::default(), |ctx| {
        for index in 1..=2500 {
            ctx.output(&format!("{index}\n"));
        }
        Ok(ToolOutput::default())
    });
    let result = run(&tool, json!({}), &ctx(dir.path())).await;
    let text = result_text(&result);
    assert!(text.starts_with("1\n2\n"));
    assert!(text.ends_with(
        "\n2000\n|<harness>\n[warn] Output truncated to its beginning: 500 lines, 2500 bytes dropped\n</harness>"
    ));
    let full: String = (1..=2500).map(|i| format!("{i}\n")).collect();
    assert_eq!(result.retained, full);
}

#[tokio::test]
async fn sanitizes_running_output_but_keeps_explicit_content_as_returned() {
    let dir = temp();
    let noisy = fake(OutputLimits::default(), |ctx| {
        ctx.output("a\u{7}b\r\n");
        Ok(ToolOutput::default())
    });
    let explicit = fake(OutputLimits::default(), |_| {
        Ok(ToolOutput {
            content: Some(vec![Block::text("c\u{1b}d")]),
            ..ToolOutput::default()
        })
    });
    let c = ctx(dir.path());
    assert_eq!(result_text(&run(&noisy, json!({}), &c).await), "ab\n");
    assert_eq!(
        result_text(&run(&explicit, json!({}), &c).await),
        "c\u{1b}d"
    );
}

#[tokio::test]
async fn repairs_then_validates_arguments_and_reports_invalid_ones() {
    let dir = temp();
    let tool = fake(OutputLimits::default(), |_| {
        Ok(ToolOutput {
            content: Some(vec![Block::text("ran")]),
            ..ToolOutput::default()
        })
    });
    let c = ctx(dir.path());
    // The repair turns a bare string into `{text}`.
    assert_eq!(result_text(&run(&tool, json!("hi"), &c).await), "ran");
    let invalid = run(&tool, json!({"text": [1]}), &c).await;
    assert!(invalid.is_error);
    assert_eq!(
        result_text(&invalid),
        "<harness>\n[error] Validation failed for tool \"fake\":\n  - text: must be string\n\nReceived arguments:\n{\n  \"text\": [\n    1\n  ]\n}\n</harness>"
    );
}

#[tokio::test]
async fn a_call_cancelled_before_it_starts_is_aborted_without_running() {
    let dir = temp();
    let tool = fake(OutputLimits::default(), |_| panic!("must not run"));
    let c = ctx(dir.path());
    c.cancel.cancel();
    assert_eq!(run(&tool, json!({}), &c).await, aborted("fake", ""));
}

#[tokio::test]
async fn a_cancelled_failure_becomes_the_aborted_result_with_reported_diagnostics() {
    let dir = temp();
    let c = ctx(dir.path());
    let cancel = c.cancel.clone();
    let tool = fake(limits(1, Retain::Tail), move |ctx| {
        ctx.output("a\nb\n");
        cancel.cancel();
        Err(ToolError {
            message: "Command aborted".into(),
            diagnostics: vec![Diagnostic::new(
                Severity::Info,
                "full_output",
                "Full output: /x",
            )],
            details: None,
        })
    });
    let result = run(&tool, json!({}), &c).await;
    assert!(result.is_error);
    assert_eq!(
        result_text(&result),
        "b\n|<harness>\n[info] Full output: /x\n[warn] Output truncated: 1 lines, 2 bytes dropped\n[error] Tool fake was aborted\n</harness>"
    );
}

#[tokio::test]
async fn retains_at_most_the_retained_limit_and_counts_the_rest() {
    let dir = temp();
    let tool = fake(limits(2000, Retain::Tail), |ctx| {
        let chunk = "x".repeat(1024 * 1024);
        for _ in 0..17 {
            ctx.output(&chunk);
        }
        ctx.output("end\n");
        Ok(ToolOutput::default())
    });
    let result = run(&tool, json!({}), &ctx(dir.path())).await;
    assert_eq!(result.retained.len(), MAX_RETAINED_OUTPUT_BYTES);
    assert!(result.retained.ends_with("xend\n"));
    assert_eq!(
        result.dropped_bytes,
        (17 * 1024 * 1024 + 4 - MAX_RETAINED_OUTPUT_BYTES) as u64
    );
    // The model sees the tail cut on the byte limit.
    assert!(result_text(&result).starts_with(&"x".repeat(51_196)));
}

#[test]
fn unavailable_and_diagnostic_rendering() {
    let result = unavailable("grep");
    assert_eq!(
        result_text(&result),
        "<harness>\n[error] Tool grep is not available\n</harness>"
    );
    assert_eq!(
        Diagnostic::new(Severity::Info, "", "m").to_json(),
        json!({"severity": "info", "message": "m"})
    );
}

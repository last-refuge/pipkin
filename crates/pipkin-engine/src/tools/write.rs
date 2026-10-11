//! `write` (`tools/write.ts`).

use serde_json::Value;

use super::path::resolve_tool_path;
use super::{Tool, ToolCtx, ToolError, ToolOutput, files, spec_from_json};
use crate::provider::{BoxFuture, ToolSpec};
use pipkin_engine_protocol::Block;

const SCHEMA: &str = r#"{"type":"object","required":["path","content"],"properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}}}"#;

const DESCRIPTION: &str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.";

pub struct WriteTool;

impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        spec_from_json("write", DESCRIPTION, SCHEMA)
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or_default().to_owned();
            let content = args["content"].as_str().unwrap_or_default().to_owned();
            let absolute = resolve_tool_path(&ctx.cwd, &path);
            let cancel = ctx.cancel.clone();
            files::with_mutation_lock(ctx, absolute.clone(), move || {
                if cancel.is_cancelled() {
                    return Err(ToolError::new("Operation aborted"));
                }
                files::write_text(&absolute, &content)
                    .map_err(|(message, _)| ToolError::new(message))?;
                if cancel.is_cancelled() {
                    return Err(ToolError::new("Operation aborted"));
                }
                Ok(ToolOutput {
                    content: Some(vec![Block::text(format!("Successfully wrote to {path}"))]),
                    ..ToolOutput::default()
                })
            })
            .await
        })
    }
}

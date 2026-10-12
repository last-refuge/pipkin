//! `mcp_search`, `mcp_describe` and `mcp_call`: the model's compact access to configured
//! MCP servers. The catalog offered to the model is these three tools, whatever is
//! installed; server tool schemas are fetched on demand. `details.mcp` (version 1) carries
//! the structured outcome, including the resolved server, tool and catalog revision of a
//! call so policy, usage and traces see the actual operation.
//!
//! `mcp_search` and `mcp_describe` are pure reads of the catalog (replay-safe). `mcp_call`
//! is not: a call interrupted by a restart is recorded as interrupted, never re-sent.

use pipkin_engine_protocol::Block;
use serde_json::{Value, json};

use super::{Tool, ToolCtx, ToolError, ToolOutput};
use crate::mcp::{McpError, ToolInfo};
use crate::provider::{BoxFuture, ToolSpec};

pub const MCP_SEARCH: &str = "mcp_search";
pub const MCP_DESCRIBE: &str = "mcp_describe";
pub const MCP_CALL: &str = "mcp_call";
pub const RESULT_VERSION: u32 = 1;

pub const SEARCH_DESCRIPTION: &str = "Find tools provided by the configured MCP servers. Give a query (matched against tool names and descriptions) and optionally a server name. Returns tools as server/name with a short description; use mcp_describe for a tool's parameters and mcp_call to run it.";
pub const DESCRIBE_DESCRIPTION: &str =
    "Show one MCP tool's description, parameter schema and annotations, by server/name.";
pub const CALL_DESCRIPTION: &str = "Run an MCP tool by server/name with arguments matching its schema. The server's own result (text, images, structured data) comes back; a timeout or lost reply means the effect is unknown, so do not assume it did not happen.";

fn details(fields: Value) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("version".into(), RESULT_VERSION.into());
    if let Value::Object(fields) = fields {
        body.extend(fields);
    }
    json!({ "mcp": Value::Object(body) })
}

fn error(e: &McpError, extra: Value) -> ToolError {
    let mut fields = json!({ "outcome": "error", "kind": e.kind(), "message": e.to_string() });
    if let (Value::Object(f), Value::Object(x)) = (&mut fields, extra) {
        f.extend(x);
    }
    ToolError {
        message: e.to_string(),
        diagnostics: vec![],
        details: Some(details(fields)),
    }
}

fn tool_json(t: &ToolInfo) -> Value {
    json!({
        "server": t.server,
        "tool": t.name,
        "qualified": t.qualified(),
        "revision": t.revision,
    })
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut s: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        s.push('…');
    }
    s
}

pub struct McpSearchTool;

impl Tool for McpSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_SEARCH.into(),
            description: SEARCH_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words to match in tool names and descriptions (empty lists everything)"},
                    "server": {"type": "string", "description": "Limit to one configured server"}
                }
            }),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let server = args
                .get("server")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let (tools, problems) = host
                .mcp_search(&query, server.as_deref())
                .await
                .map_err(ToolError::new)?
                .map_err(|e| error(&e, json!({})))?;
            let mut lines = vec![format!(
                "{} MCP tool(s){}{}",
                tools.len(),
                if query.trim().is_empty() {
                    String::new()
                } else {
                    format!(" matching \"{}\"", query.trim())
                },
                server
                    .as_deref()
                    .map(|s| format!(" on {s}"))
                    .unwrap_or_default()
            )];
            for t in &tools {
                lines.push(format!(
                    "- {}: {}",
                    t.qualified(),
                    first_line(&t.description, 160)
                ));
            }
            for (server, problem) in &problems {
                lines.push(format!("! {server}: {problem}"));
            }
            if tools.is_empty() && problems.is_empty() {
                lines.push("No tools matched.".into());
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n"))]),
                details: Some(details(json!({
                    "outcome": "search",
                    "query": query,
                    "tools": tools.iter().map(tool_json).collect::<Vec<_>>(),
                    "problems": problems.iter().map(|(s, p)| json!({"server": s, "message": p})).collect::<Vec<_>>(),
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct McpDescribeTool;

impl Tool for McpDescribeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_DESCRIBE.into(),
            description: DESCRIBE_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {"tool": {"type": "string", "description": "server/name"}},
                "required": ["tool"]
            }),
        }
    }

    fn replay_safe(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let qualified = args
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let tool = host
                .mcp_describe(&qualified)
                .await
                .map_err(ToolError::new)?
                .map_err(|e| error(&e, json!({ "tool": qualified })))?;
            let mut lines = vec![format!("{}: {}", tool.qualified(), tool.description)];
            lines.push(format!(
                "Parameters (JSON schema): {}",
                serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default()
            ));
            if let Some(a) = &tool.annotations {
                lines.push(format!(
                    "Annotations (the server's hints, not verified): {}",
                    serde_json::to_string(a).unwrap_or_default()
                ));
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n"))]),
                details: Some(details(json!({
                    "outcome": "describe",
                    "tool": tool_json(&tool),
                    "inputSchema": tool.input_schema,
                    "annotations": tool.annotations,
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct McpCallTool;

impl Tool for McpCallTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_CALL.into(),
            description: CALL_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tool": {"type": "string", "description": "server/name"},
                    "arguments": {"type": "object", "description": "Arguments matching the tool's schema"}
                },
                "required": ["tool"]
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let qualified = args
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let arguments = args.get("arguments").cloned().unwrap_or(Value::Null);
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let outcome = host
                .mcp_call(&ctx.call_id, &qualified, arguments, ctx.cancel.clone())
                .await
                .map_err(ToolError::new)?;
            match outcome {
                Err(McpError::Cancelled) => Err(ToolError::new(
                    "The call was cancelled; its effect is unknown",
                )),
                Err(e) => Err(error(&e, json!({ "tool": qualified }))),
                Ok(outcome) => {
                    let mut content = outcome.content;
                    if content.is_empty() {
                        content.push(Block::text("(the tool returned no content)"));
                    }
                    if let Some(structured) = &outcome.structured {
                        content.push(Block::text(format!(
                            "Structured result: {}",
                            serde_json::to_string(structured).unwrap_or_default()
                        )));
                    }
                    let fields = json!({
                        "outcome": if outcome.is_error { "tool_error" } else { "call" },
                        "tool": tool_json(&outcome.tool),
                        "structured": outcome.structured,
                    });
                    if outcome.is_error {
                        // The server's own error: the model sees the content, marked as a
                        // failure of the remote tool rather than of the transport.
                        return Err(ToolError {
                            message: format!(
                                "{} reported an error: {}",
                                outcome.tool.qualified(),
                                content
                                    .iter()
                                    .filter_map(|b| match b {
                                        Block::Text { text, .. } => Some(text.as_str()),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            ),
                            diagnostics: vec![],
                            details: Some(details(fields)),
                        });
                    }
                    Ok(ToolOutput {
                        content: Some(content),
                        details: Some(details(fields)),
                        ..ToolOutput::default()
                    })
                }
            }
        })
    }
}

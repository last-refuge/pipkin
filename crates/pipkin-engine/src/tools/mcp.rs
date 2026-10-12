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

// ------------------------------------------------------------------ resources and prompts

pub const MCP_RESOURCES: &str = "mcp_resources";
pub const MCP_READ: &str = "mcp_read";
pub const MCP_PROMPTS: &str = "mcp_prompts";
pub const MCP_PROMPT: &str = "mcp_prompt";
/// Default and largest slice `mcp_read` returns, in characters.
pub const READ_DEFAULT_CHARS: usize = 8_000;
pub const READ_MAX_CHARS: usize = 40_000;

pub const RESOURCES_DESCRIPTION: &str = "List the resources (documents, files, data) the configured MCP servers offer, with their URIs; optionally filter by a query or one server. Entries marked as templates take parameters in the URI. Read one with mcp_read.";
pub const READ_DESCRIPTION: &str = "Read an MCP resource by server and URI. Text comes back in bounded slices (offset/limit in characters); binary content is reported by type and size only.";
pub const PROMPTS_DESCRIPTION: &str = "List the prompt templates the configured MCP servers offer, with their arguments; optionally filter by a query or one server. Render one with mcp_prompt.";
pub const PROMPT_DESCRIPTION: &str = "Render an MCP prompt template by server/name with its arguments. The server's messages come back as text for you to follow or quote.";

fn matches(query: &str, haystacks: &[&str]) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty() || haystacks.iter().any(|h| h.to_lowercase().contains(&needle))
}

fn problems_json(problems: &[(String, String)]) -> Value {
    problems
        .iter()
        .map(|(s, p)| json!({"server": s, "message": p}))
        .collect()
}

pub struct McpResourcesTool;

impl Tool for McpResourcesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_RESOURCES.into(),
            description: RESOURCES_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words to match in names, URIs and descriptions"},
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
            let (all, problems) = host
                .mcp_resources(server.as_deref())
                .await
                .map_err(ToolError::new)?
                .map_err(|e| error(&e, json!({})))?;
            let found: Vec<_> = all
                .into_iter()
                .filter(|r| matches(&query, &[&r.name, &r.uri, &r.description]))
                .take(crate::mcp::service::MAX_SEARCH_RESULTS)
                .collect();
            let mut lines = vec![format!(
                "{} MCP resource(s){}{}",
                found.len(),
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
            for r in &found {
                let mut line = format!("- {} {} ({})", r.server, r.uri, r.name);
                if r.template {
                    line.push_str(" [template]");
                }
                if let Some(m) = &r.mime_type {
                    line.push_str(&format!(" {m}"));
                }
                if let Some(size) = r.size {
                    line.push_str(&format!(" {size} bytes"));
                }
                if !r.description.is_empty() {
                    line.push_str(&format!(": {}", first_line(&r.description, 160)));
                }
                lines.push(line);
            }
            for (s, p) in &problems {
                lines.push(format!("! {s}: {p}"));
            }
            if found.is_empty() && problems.is_empty() {
                lines.push("No resources matched.".into());
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n"))]),
                details: Some(details(json!({
                    "outcome": "resources",
                    "query": query,
                    "resources": found.iter().map(|r| json!({"server": r.server, "uri": r.uri, "name": r.name, "template": r.template, "mimeType": r.mime_type})).collect::<Vec<_>>(),
                    "problems": problems_json(&problems),
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct McpReadTool;

impl Tool for McpReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_READ.into(),
            description: READ_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string", "description": "The configured server"},
                    "uri": {"type": "string", "description": "The resource URI (a template with its parameters filled in)"},
                    "offset": {"type": "integer", "description": "Character offset to start from (default 0)"},
                    "limit": {"type": "integer", "description": format!("Characters to return (default {READ_DEFAULT_CHARS}, at most {READ_MAX_CHARS})")}
                },
                "required": ["server", "uri"]
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
            let server = args
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            let uri = args
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|l| (l as usize).clamp(1, READ_MAX_CHARS))
                .unwrap_or(READ_DEFAULT_CHARS);
            if server.is_empty() || uri.is_empty() {
                return Err(ToolError::new("server and uri are required"));
            }
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let parts = host
                .mcp_read(&server, &uri)
                .await
                .map_err(ToolError::new)?
                .map_err(|e| error(&e, json!({ "server": server, "uri": uri })))?;
            let mut lines = Vec::new();
            let mut total_chars = 0usize;
            let mut more = false;
            for part in &parts {
                match (&part.text, part.blob_bytes) {
                    (Some(text), _) => {
                        let total = text.chars().count();
                        total_chars += total;
                        let slice: String = text.chars().skip(offset).take(limit).collect();
                        let end = (offset + slice.chars().count()).min(total);
                        more |= end < total;
                        lines.push(format!(
                            "{} {} — characters {offset}–{end} of {total}{}",
                            part.uri,
                            part.mime_type.as_deref().unwrap_or("text"),
                            if end < total {
                                format!(" (continue with offset {end})")
                            } else {
                                String::new()
                            }
                        ));
                        lines.push(slice);
                    }
                    (None, Some(bytes)) => lines.push(format!(
                        "{} {} — binary content, {bytes} bytes (not shown)",
                        part.uri,
                        part.mime_type
                            .as_deref()
                            .unwrap_or("application/octet-stream")
                    )),
                    (None, None) => lines.push(format!("{} — empty", part.uri)),
                }
            }
            if parts.is_empty() {
                lines.push(format!("{server} returned no content for {uri}"));
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n"))]),
                details: Some(details(json!({
                    "outcome": "read",
                    "server": server,
                    "uri": uri,
                    "parts": parts.len(),
                    "chars": total_chars,
                    "offset": offset,
                    "more": more,
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct McpPromptsTool;

impl Tool for McpPromptsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_PROMPTS.into(),
            description: PROMPTS_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words to match in names and descriptions"},
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
            let (all, problems) = host
                .mcp_prompts(server.as_deref())
                .await
                .map_err(ToolError::new)?
                .map_err(|e| error(&e, json!({})))?;
            let found: Vec<_> = all
                .into_iter()
                .filter(|p| matches(&query, &[&p.name, &p.description]))
                .take(crate::mcp::service::MAX_SEARCH_RESULTS)
                .collect();
            let mut lines = vec![format!(
                "{} MCP prompt(s){}{}",
                found.len(),
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
            for p in &found {
                let args: Vec<String> = p
                    .arguments
                    .iter()
                    .map(|a| {
                        if a.required {
                            a.name.clone()
                        } else {
                            format!("{}?", a.name)
                        }
                    })
                    .collect();
                lines.push(format!(
                    "- {}/{}({}): {}",
                    p.server,
                    p.name,
                    args.join(", "),
                    first_line(&p.description, 160)
                ));
            }
            for (s, p) in &problems {
                lines.push(format!("! {s}: {p}"));
            }
            if found.is_empty() && problems.is_empty() {
                lines.push("No prompts matched.".into());
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n"))]),
                details: Some(details(json!({
                    "outcome": "prompts",
                    "query": query,
                    "prompts": found.iter().map(|p| json!({"server": p.server, "name": p.name, "arguments": p.arguments.iter().map(|a| json!({"name": a.name, "required": a.required, "description": a.description})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                    "problems": problems_json(&problems),
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

pub struct McpPromptTool;

impl Tool for McpPromptTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            constrained_sampling: None,
            name: MCP_PROMPT.into(),
            description: PROMPT_DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "server/name"},
                    "arguments": {"type": "object", "description": "The template's arguments (strings)"}
                },
                "required": ["prompt"]
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
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let (server, prompt) = crate::mcp::service::split_qualified(&qualified)
                .map_err(|e| error(&e, json!({ "prompt": qualified })))?;
            let arguments: serde_json::Map<String, Value> = args
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| {
                    let text = match v {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (k, Value::String(text))
                })
                .collect();
            let Some(host) = &ctx.host else {
                return Err(ToolError::new("No execution host is configured"));
            };
            let outcome = host
                .mcp_prompt(&server, &prompt, arguments)
                .await
                .map_err(ToolError::new)?
                .map_err(|e| error(&e, json!({ "prompt": qualified })))?;
            let mut lines = Vec::new();
            if let Some(d) = &outcome.description {
                lines.push(format!("{qualified}: {d}"));
            }
            for (role, text) in &outcome.messages {
                lines.push(format!("[{role}]\n{text}"));
            }
            if outcome.messages.is_empty() {
                lines.push(format!("{qualified} rendered no messages"));
            }
            Ok(ToolOutput {
                content: Some(vec![Block::text(lines.join("\n\n"))]),
                details: Some(details(json!({
                    "outcome": "prompt",
                    "server": server,
                    "prompt": prompt,
                    "messages": outcome.messages.len(),
                }))),
                ..ToolOutput::default()
            })
        })
    }
}

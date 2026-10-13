//! A scripted MCP server over stdio for the engine's tests (not shipped). Tools:
//! `echo`, `counter_increment` (file-backed side effect, optionally never replying),
//! `slow`, `fail`, `big`, `change_tools` (adds `added_tool` and notifies the client).
//!
//! `MCP_FIXTURE_FAIL_INIT=1` exits before serving; `MCP_FIXTURE_ENV=NAME` makes `echo`
//! append that environment variable's value.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, Peer, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router,
};
use serde::Deserialize;

#[derive(Clone)]
struct Fixture {
    tool_router: ToolRouter<Self>,
    extra: Arc<AtomicBool>,
    /// `change_resources` adds a resource and notifies.
    extra_resource: Arc<AtomicBool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct AskArgs {
    /// `form` asks name/age/ok/color; `empty` asks a bare confirmation; `url` sends a URL
    /// elicitation (unsupported by Pipkin).
    #[serde(default)]
    mode: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct EchoArgs {
    /// The text to echo.
    text: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct CounterArgs {
    /// File holding the counter.
    path: String,
    /// Increment, then never reply (simulates a lost reply).
    #[serde(default)]
    hang_after: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SlowArgs {
    ms: u64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct FailArgs {
    message: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct BigArgs {
    bytes: usize,
}

#[tool_router]
impl Fixture {
    fn new() -> Fixture {
        Fixture {
            tool_router: Self::tool_router(),
            extra: Arc::new(AtomicBool::new(false)),
            extra_resource: Arc::new(AtomicBool::new(false)),
        }
    }

    #[tool(name = "echo", description = "Echo the text back")]
    fn echo(&self, Parameters(args): Parameters<EchoArgs>) -> CallToolResult {
        let mut text = args.text;
        if let Ok(name) = std::env::var("MCP_FIXTURE_ENV")
            && let Ok(value) = std::env::var(&name)
        {
            text.push_str(&format!(" env:{value}"));
        }
        CallToolResult::success(vec![ContentBlock::text(text)])
    }

    #[tool(
        name = "counter_increment",
        description = "Increment the counter in a file and return the new value"
    )]
    async fn counter_increment(
        &self,
        Parameters(args): Parameters<CounterArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let current: u64 = std::fs::read_to_string(&args.path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let next = current + 1;
        std::fs::write(&args.path, next.to_string())
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        if args.hang_after {
            std::future::pending::<()>().await;
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            next.to_string(),
        )]))
    }

    #[tool(name = "slow", description = "Reply after a delay")]
    async fn slow(&self, Parameters(args): Parameters<SlowArgs>) -> CallToolResult {
        tokio::time::sleep(std::time::Duration::from_millis(args.ms)).await;
        CallToolResult::success(vec![ContentBlock::text(format!("slept {} ms", args.ms))])
    }

    #[tool(name = "fail", description = "Report a tool error")]
    fn fail(&self, Parameters(args): Parameters<FailArgs>) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text(args.message)])
    }

    #[tool(name = "big", description = "Return a large text")]
    fn big(&self, Parameters(args): Parameters<BigArgs>) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::text("x".repeat(args.bytes))])
    }

    #[tool(name = "picture", description = "Return a tiny PNG beside a caption")]
    fn picture(&self) -> CallToolResult {
        CallToolResult::success(vec![
            ContentBlock::text("a caption"),
            ContentBlock::image("iVBORw0KGgoAAAANSUhEUg==", "image/png"),
        ])
    }

    #[tool(
        name = "change_resources",
        description = "Add a resource and notify the client"
    )]
    async fn change_resources(&self, peer: Peer<RoleServer>) -> CallToolResult {
        self.extra_resource.store(true, Ordering::SeqCst);
        let _ = peer.notify_resource_list_changed().await;
        CallToolResult::success(vec![ContentBlock::text("changed")])
    }

    #[tool(
        name = "ask_person",
        description = "Ask the person for input through elicitation and return what they said"
    )]
    async fn ask_person(
        &self,
        Parameters(args): Parameters<AskArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = match args.mode.as_str() {
            "empty" => ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: "Proceed with the operation?".into(),
                requested_schema: ElicitationSchema::builder()
                    .build()
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
            },
            "url" => ElicitRequestParams::UrlElicitationParams {
                meta: None,
                message: "Visit the page".into(),
                url: "https://example.com/elicit".into(),
                elicitation_id: "e1".into(),
            },
            _ => ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: "Tell me about yourself".into(),
                requested_schema: ElicitationSchema::builder()
                    .required_property(
                        "name",
                        PrimitiveSchemaDefinition::String(StringSchema::new()),
                    )
                    .property(
                        "age",
                        PrimitiveSchemaDefinition::Integer(IntegerSchema::new()),
                    )
                    .required_property(
                        "ok",
                        PrimitiveSchemaDefinition::Boolean(BooleanSchema::new()),
                    )
                    .required_property(
                        "color",
                        PrimitiveSchemaDefinition::Enum(
                            EnumSchema::builder(vec!["red".into(), "blue".into()]).build(),
                        ),
                    )
                    .build()
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
            },
        };
        let result = peer
            .create_elicitation(params)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let action = serde_json::to_value(&result.action)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::json!({"action": action, "content": result.content}).to_string(),
        )]))
    }

    #[tool(
        name = "change_tools",
        description = "Add a tool to the catalog and notify the client"
    )]
    async fn change_tools(&self, peer: Peer<RoleServer>) -> CallToolResult {
        self.extra.store(true, Ordering::SeqCst);
        let _ = peer.notify_tool_list_changed().await;
        CallToolResult::success(vec![ContentBlock::text("changed")])
    }
}

#[tool_handler]
impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_prompts()
                .enable_prompts_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new("mcp-fixture", "0.0.0"))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let mut resources = vec![
            Resource::new("fixture://readme", "readme")
                .with_title("Read me")
                .with_description("The fixture's notes")
                .with_mime_type("text/plain")
                .with_size(40),
            Resource::new("fixture://logo", "logo")
                .with_description("A tiny binary")
                .with_mime_type("image/png"),
        ];
        if self.extra_resource.load(Ordering::SeqCst) {
            resources.push(Resource::new("fixture://added", "added").with_mime_type("text/plain"));
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("fixture://files/{name}", "file")
                .with_description("A file by name")
                .with_mime_type("text/plain"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.as_str();
        let contents = match uri {
            "fixture://readme" => vec![
                ResourceContents::text("Line one of the readme.\nLine two of the readme.", uri)
                    .with_mime_type("text/plain"),
            ],
            "fixture://logo" => vec![
                ResourceContents::blob("iVBORw0KGgoAAAANSUhEUg==", uri).with_mime_type("image/png"),
            ],
            "fixture://big" => vec![ResourceContents::text("x".repeat(20_000), uri)],
            _ if uri.starts_with("fixture://files/") => {
                let name = uri.trim_start_matches("fixture://files/");
                vec![ResourceContents::text(format!("contents of {name}"), uri)]
            }
            _ => {
                return Err(ErrorData::resource_not_found(
                    format!("no resource {uri}"),
                    None,
                ));
            }
        };
        Ok(ReadResourceResult::new(contents).into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![
            Prompt::new(
                "summarize",
                Some("Summarize a topic"),
                Some(vec![
                    PromptArgument::new("topic")
                        .with_description("What to summarize")
                        .with_required(true),
                    PromptArgument::new("tone")
                        .with_description("Formal or casual")
                        .with_required(false),
                ]),
            ),
            Prompt::new("greet", Some("Say hello"), None),
        ]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        let messages = match request.name.as_str() {
            "summarize" => {
                let topic = args.get("topic").and_then(|v| v.as_str()).unwrap_or("");
                let tone = args
                    .get("tone")
                    .and_then(|v| v.as_str())
                    .unwrap_or("neutral");
                vec![
                    PromptMessage::new_text(
                        Role::User,
                        format!("Summarize {topic} in a {tone} tone."),
                    ),
                    PromptMessage::new_text(Role::Assistant, "Certainly. Which length?"),
                ]
            }
            "greet" => vec![PromptMessage::new_text(Role::User, "Hello there!")],
            other => {
                return Err(ErrorData::invalid_params(
                    format!("no prompt {other}"),
                    None,
                ));
            }
        };
        Ok(GetPromptResult::new(messages)
            .with_description("rendered by the fixture")
            .into())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = self.tool_router.list_all();
        if self.extra.load(Ordering::SeqCst) {
            tools.push(Tool::new(
                "added_tool",
                "A tool that appeared after change_tools",
                Arc::new(serde_json::Map::from_iter([(
                    "type".to_owned(),
                    serde_json::Value::String("object".into()),
                )])),
            ));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }
}

#[tokio::main]
async fn main() {
    if std::env::var("MCP_FIXTURE_FAIL_INIT").is_ok() {
        std::process::exit(3);
    }
    let service = Fixture::new()
        .serve(rmcp::transport::io::stdio())
        .await
        .expect("serve");
    let _ = service.waiting().await;
}

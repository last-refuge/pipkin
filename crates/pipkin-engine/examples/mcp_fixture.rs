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
                .build(),
        )
        .with_server_info(Implementation::new("mcp-fixture", "0.0.0"))
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

//! MCP Server — expose phi-agent as an MCP Server for external orchestrators.
//!
//! This example demonstrates how to turn an agent into an MCP server that
//! external systems (LangGraph, CrewAI, custom scripts) can call through the
//! Model Context Protocol.
//!
//! The agent exposes a single `run` tool — external orchestrators call
//! `tools/call` with `{ name: "run", arguments: { prompt: "..." } }` to
//! delegate tasks. The agent runs a full ReAct loop internally and streams
//! progress notifications back.
//!
//! ## Usage
//!
//! ```bash
//! # Stdio mode (subprocess — default)
//! cargo run --features mcp --example mcp_server
//!
//! # HTTP mode
//! cargo run --features mcp --example mcp_server -- --http 8080
//! ```
//!
//! ## Testing with an MCP client
//!
//! Stdio mode (pipe a JSON-RPC request):
//! ```bash
//! echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | \
//!   cargo run --features mcp --example mcp_server
//! ```
//!
//! HTTP mode:
//! ```bash
//! # In one terminal:
//! cargo run --features mcp --example mcp_server -- --http 8080
//!
//! # In another terminal:
//! curl -X POST http://localhost:8080/mcp \
//!   -H "Content-Type: application/json" \
//!   -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
//! ```

#![cfg(feature = "mcp")]

use std::sync::Arc;

use agent_works::mcp::{McpServeConfig, McpServerTransport};
use phi_agent::{
    ApprovalMode, AutoApprovalHandler, PhiAgent, PhiAgentConfig, SafetyConfig, TurnFactMiddleware,
    TurnToolLimitMiddleware, base_agent_builder, build_system_prompt,
};

#[path = "../common/mod.rs"]
mod common;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Parse --http <port> from command line
    let args: Vec<String> = std::env::args().collect();
    let http_port =
        args.iter().position(|a| a == "--http").and_then(|i| args.get(i + 1).and_then(|p| p.parse::<u16>().ok()));

    // ── 1. Create LLM client ──
    let llm_client = common::client();

    // ── 2. Build agent ──
    //
    // The agent uses Auto approval mode so external callers don't need to
    // approve every tool invocation interactively. Adjust safety settings
    // in SafetyConfig for production use.
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt())
        .approval_handler(Arc::new(AutoApprovalHandler::new(ApprovalMode::Auto)))
        .middleware(TurnFactMiddleware::new())
        .middleware(TurnToolLimitMiddleware::from_config(&SafetyConfig::default()));

    let agent =
        PhiAgent::build(builder, PhiAgentConfig { model: common::resolve_llm_env().model, ..Default::default() })?;

    // ── 3. Configure transport ──
    let transport = match http_port {
        Some(port) => McpServerTransport::Http { host: "127.0.0.1".to_string(), port },
        None => McpServerTransport::Stdio,
    };

    let config =
        McpServeConfig { name: "phi-agent".to_string(), version: env!("CARGO_PKG_VERSION").to_string(), transport };

    // ── 4. Start serving ──
    match &config.transport {
        McpServerTransport::Stdio => {
            eprintln!(
                "phi-agent MCP server ready (stdio). \
                 Waiting for JSON-RPC requests on stdin..."
            );
        },
        McpServerTransport::Http { host, port } => {
            eprintln!(
                "phi-agent MCP server ready (HTTP). \
                 Listening on http://{host}:{port}/mcp"
            );
        },
    }

    let server = agent.into_mcp_server(config);
    server.serve().await?;

    Ok(())
}

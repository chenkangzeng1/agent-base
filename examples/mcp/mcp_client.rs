//! MCP Client — demonstrate connecting to an MCP server at build time.
//!
//! This example shows how to configure and connect to an MCP server
//! when building the agent. Tools from the server are discovered and
//! registered automatically with the `mcp.<server>.<tool>` naming convention.
//!
//! Usage:
//!   cargo run --features mcp --example mcp_client
//!
//! Prerequisites:
//!   - A running MCP server (e.g. a local stdio server or HTTP endpoint)
//!   - Update the MCP_SERVER_COMMAND / MCP_SERVER_URL below to match your server

use std::sync::Arc;

use agent_works::mcp::{EnhancedMcpHub, McpServerConfig, McpTransport};
use phi_agent::{PhiAgent, PhiAgentConfig, ReasoningEffort, SafetyConfig, base_agent_builder, build_system_prompt};

#[path = "../common/mod.rs"]
mod common;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // ── 1. Create LLM client ──
    let llm_client = common::client();

    // ── 2. Configure MCP server ──
    //
    // stdio transport (local process):
    let mcp_config = McpServerConfig {
        name: "my-server".into(),
        transport: McpTransport::Stdio {
            command: "echo".into(), // replace with your MCP server binary
            args: vec![],
        },
        auto_reconnect: false,
    };

    // Alternative: HTTP transport
    // let mcp_config = McpServerConfig {
    //     name: "my-server".into(),
    //     transport: McpTransport::Http {
    //         url: "http://localhost:3000/mcp".into(),
    //     },
    //     auto_reconnect: false,
    // };

    // ── 3. Build MCP hub and connect ──
    let hub = EnhancedMcpHub::new();
    hub.add_server(mcp_config);
    hub.connect_all().await?;

    // Discover tools from all configured servers
    let discovered = hub.discover_all().await?;
    for (server, tools) in &discovered {
        println!("Server '{}': {} tools", server, tools.len());
        for tool in tools {
            println!("  - mcp.{}.{}", server, tool.name);
        }
    }

    // ── 4. Build agent ──
    let mut builder = base_agent_builder(llm_client).system_prompt(build_system_prompt());

    // Register MCP tools into the agent's tool registry
    {
        let tools = builder.tools_mut();
        let mut registry = tools.write().await;
        hub.register_all(&mut registry).await;
    }

    let agent = PhiAgent::build(
        builder,
        PhiAgentConfig {
            model: common::resolve_llm_env().model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
            max_turns: Some(20),
        },
    )?;

    // ── 5. Run ──
    let session = agent.create_session().await;
    let renderer = phi_agent::create_stdout_renderer(&phi_agent::OutputFormat::Terminal {
        show_thinking: true,
        show_tool_args: true,
        color: true,
    });

    println!("\n=== Agent ready with MCP tools ===\n");
    agent.run_turn(session, "List your available tools", |event| renderer.render(event)).await?;

    Ok(())
}

//! Hybrid Architecture Demo: LangGraph + phi-agent via MCP.
//!
//! This example demonstrates how an external orchestrator (LangGraph, CrewAI,
//! or custom scripts) can delegate tasks to phi-agent through the Model Context
//! Protocol.
//!
//! ## Architecture
//!
//! ```
//! ┌─────────────────────┐         MCP (JSON-RPC)        ┌──────────────────┐
//! │  LangGraph / Python  │ ──────────────────────────────▶│  phi-agent (Rust) │
//! │                     │◀──────────────────────────────│                  │
//! │  - Planning          │  tools/call { run: prompt }   │  - ReAct loop     │
//! │  - Routing           │  progress notifications       │  - File tools     │
//! │  - Human-in-the-loop │  final result                 │  - Shell tools    │
//! └─────────────────────┘                               │  - MCP tools      │
//!                                                        └──────────────────┘
//! ```
//!
//! LangGraph handles the high-level orchestration (which agent to call, when
//! to ask for human input, how to route results). phi-agent handles the actual
//! autonomous execution — it takes a task, runs a full ReAct loop, and returns
//! the result.
//!
//! ## Usage
//!
//! Start phi-agent as an MCP server:
//! ```bash
//! cargo run --features mcp -- serve --http 8080
//! ```
//!
//! Then from Python/LangGraph, connect to `http://localhost:8080/mcp` and
//! call the `run` tool with a task prompt.
//!
//! ## Example Python client
//!
//! ```python
//! import json, requests
//!
//! # 1. Discover tools
//! r = requests.post("http://localhost:8080/mcp", json={
//!     "jsonrpc": "2.0", "id": 1, "method": "tools/list"
//! })
//! tools = r.json()["tools"]
//! print(f"Available tools: {[t['name'] for t in tools]}")
//! # → ['run']
//!
//! # 2. Delegate a task
//! r = requests.post("http://localhost:8080/mcp", json={
//!     "jsonrpc": "2.0", "id": 2, "method": "tools/call",
//!     "params": {"name": "run", "arguments": {"prompt": "Search src/ for all SQL queries"}}
//! })
//! print(r.json()["content"][0]["text"])
//! # → phi-agent's analysis result
//! ```
//!
//! ## LangGraph integration sketch
//!
//! ```python
//! from langgraph.graph import StateGraph, END
//! from langgraph.checkpoint import MemorySaver
//! import requests
//!
//! def call_phi_agent(state):
//!     """Node that delegates to phi-agent via MCP."""
//!     prompt = state["task"]
//!     r = requests.post(
//!         "http://localhost:8080/mcp",
//!         json={
//!             "jsonrpc": "2.0", "id": 1, "method": "tools/call",
//!             "params": {"name": "run", "arguments": {"prompt": prompt}},
//!         },
//!         timeout=300,
//!     )
//!     result = r.json()
//!     state["result"] = result["content"][0]["text"]
//!     return state
//!
//! def route_decision(state):
//!     """Route based on phi-agent's result."""
//!     if "NEED_HUMAN_INPUT" in state.get("result", ""):
//!         return "ask_human"
//!     return "done"
//!
//! # Build graph
//! graph = StateGraph(dict)
//! graph.add_node("phi_agent", call_phi_agent)
//! graph.add_node("ask_human", human_input_node)
//! graph.add_conditional_edges("phi_agent", route_decision)
//! graph.set_entry_point("phi_agent")
//!
//! app = graph.compile(checkpointer=MemorySaver())
//! result = app.invoke({"task": "Audit security vulnerabilities in src/"})
//! ```
//!
//! ## Key design points
//!
//! - **phi-agent is a tool, not a framework.** LangGraph owns the graph
//!   topology; phi-agent is just one node that does autonomous work.
//! - **MCP is the contract.** Any MCP client can call phi-agent — Python,
//!   TypeScript, Go, or another Rust program.
//! - **Single `run` tool.** phi-agent exposes itself, not its internal tools.
//!   This way LangGraph doesn't need to know about phi-agent's internals.
//! - **Progress streaming.** phi-agent sends progress notifications during
//!   execution, so the orchestrator can show real-time status.

#![cfg(feature = "mcp")]

use std::sync::Arc;

use phi_agent::{
    ApprovalMode, AutoApprovalHandler, PhiAgent, PhiAgentConfig, SafetyConfig, TurnFactMiddleware,
    TurnToolLimitMiddleware, base_agent_builder, build_system_prompt,
};

#[path = "../common/mod.rs"]
mod common;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // ── Build phi-agent as an MCP server ──
    //
    // This agent is configured for use as a "tool node" in a larger workflow.
    // Auto-approval is used so external orchestrators don't need to manually
    // approve each tool call.
    let llm_client = common::client();
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt())
        .approval_handler(Arc::new(AutoApprovalHandler::new(ApprovalMode::Auto)))
        .middleware(TurnFactMiddleware::new())
        .middleware(TurnToolLimitMiddleware::from_config(&SafetyConfig::default()));

    let agent =
        PhiAgent::build(builder, PhiAgentConfig { model: common::resolve_llm_env().model, ..Default::default() })?;

    // ── Start HTTP MCP server ──
    let config = agent_works::mcp::McpServeConfig {
        name: "phi-agent".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        transport: agent_works::mcp::McpServerTransport::Http { host: "127.0.0.1".to_string(), port: 8080 },
    };

    eprintln!("phi-agent MCP server ready for LangGraph integration.");
    eprintln!("Connect from LangGraph: http://localhost:8080/mcp");
    eprintln!();
    eprintln!(
        "Example: POST {{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"params\":{{\"name\":\"run\",\"arguments\":{{\"prompt\":\"Hello\"}}}}}}"
    );

    let server = agent.into_mcp_server(config);
    server.serve().await?;

    Ok(())
}

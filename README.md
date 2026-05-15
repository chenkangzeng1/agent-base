# agent-base

A lightweight **Agent Runtime Kernel** for building AI agents in Rust.

`agent-base` provides the minimal orchestration layer needed to build custom AI agents — LLM integration, tool dispatch, multi-turn conversation, approval flows, event streaming, and error recovery — all with zero business assumptions.

## Design Principles

- **Clear semantics** — `RunOutcome` explicitly distinguishes `Completed` from `Failed`; events capture the process, the return value captures the final result.
- **Simple state model** — Runtime memory is the source of truth for live sessions; `SessionStore` is an optional persistence adapter.
- **Conservative by default** — On tool failure, the runtime stops by default (`StopOnError`) rather than guessing how to recover.
- **Strategy injection** — All variable behaviors are injected via traits (`ToolErrorRecovery`, `ToolPolicy`, `ApprovalHandler`, `Middleware`), not hardcoded.

## Features

- **LLM Abstraction** — `LlmClient` trait with built-in OpenAI and Anthropic implementations
- **Tool System** — `Tool` trait + `ToolRegistry` for registration and dispatch
- **Approval Flow** — `ApprovalHandler` trait with `AllowOnce` / `AllowAlways` / `Deny` decisions
- **Error Recovery** — `ToolErrorRecovery` trait; defaults to `StopOnError`, opt-in `RetryOnError`
- **Event Streaming** — Structured `AgentEvent` stream for UI, logging, auditing, and debugging
- **Multi-turn Sessions** — `AgentSession` manages message history; `SessionStore` for optional persistence
- **Sub-Agents** — `SubAgentTool` with `Ephemeral` (default) or `Persistent` session policies
- **Context Management** — configurable `ContextWindowManager` for token budget control
- **Middleware** — hooks at `on_user_message`, `on_pre_llm`, and `on_post_llm` for extensions
- **Checkpoints** — structured `Checkpoint` events enable future replay, debugging, and resume
- **MCP Support** — built-in `McpClient` for the Model Context Protocol
- **Skills** — composable capability units with auto-registered tools and on-demand detailed prompts

## Quick Start

```rust
use std::sync::Arc;
use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, RunOutcome,
    AnthropicClient, Tool, ToolContext, ToolOutput,
};
use serde_json::{json, Value};

// 1. Define a tool
struct GreetTool;

#[async_trait::async_trait]
impl Tool for GreetTool {
    fn name(&self) -> &'static str { "greet" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "greet",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": ["name"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("world");
        Ok(ToolOutput {
            summary: format!("Hello, {}!", name),
            raw: None,
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

// 2. Build the runtime
let client = Arc::new(AnthropicClient::new(
    "sk-ant-xxx".into(),
    "claude-3-5-sonnet-20241022".into(),
    None,
));
let mut runtime = AgentBuilder::new(client)
    .system_prompt("You are a friendly assistant.")
    .register_tool(GreetTool)
    .build();

// 3. Run a turn
let session_id = runtime.create_session();
let (events, outcome) = runtime.run_turn_stream(session_id, "Greet Alice").await?;
assert_eq!(outcome, RunOutcome::Completed);
```

## Examples

```bash
# Configure API key
cp .env.example .env
# Edit .env with your OPENAI_API_KEY or ANTHROPIC_API_KEY

# Run the REPL example
cargo run --example repl

# Run the SubAgent demo
cargo run --example subagent_demo

# Run the MCP demo
cargo run --example mcp_demo

# Run the Skill demo
cargo run --example skill_demo
```

## What agent-base Does NOT Do

- ❌ No SSH, filesystem, or database tools
- ❌ No workflow DAG / multi-agent orchestration engine
- ❌ No memory / RAG framework
- ❌ No terminal UI or approval dialogs
- ❌ No heavy persistence or transaction system

Business-specific tools and strategies belong in **upper layers** (`ops-agent`, `db-agent`, etc.).

## Typical Layering

```
ops-agent / db-agent / browser-agent    ← Business agents
    └── agent-base                       ← Lightweight Runtime Kernel
```

## v1 Semantics

| Convention | Meaning |
|---|---|
| `run_turn_*` → `AgentResult<RunOutcome>` | `Ok(Completed)` = success, `Ok(Failed)` = finished with error |
| `AgentEvent::RunFinished` | Process ended — final status is in `RunOutcome` |
| Tool failure → defaults to `StopOnError` | Inject `RetryOnError` for self-healing agents |
| SubAgent → defaults to `Ephemeral` | Use `with_persistent()` for shared context |
| Session → memory is source of truth | `SessionStore` is an optional persistence adapter |

## Stability

This project is in early development (v0.1.0). The core abstractions are settling but not yet frozen. Expect minor API changes as the ecosystem evolves.

## License

MIT

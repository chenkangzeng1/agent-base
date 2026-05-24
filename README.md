# agent-base

[![crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base)
[![Documentation](https://docs.rs/agent-base/badge.svg)](https://docs.rs/agent-base)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A lightweight **Agent Runtime Kernel** for building AI agents in Rust.

`agent-base` provides the minimal orchestration layer needed to build custom AI agents — LLM integration, tool dispatch, multi-turn conversation, approval flows, event streaming, and error recovery — all with zero business assumptions.

## Installation

```toml
[dependencies]
agent-base = "0.1.1"
```

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
- **Tool Enforcement** — `ToolEnforcementMiddleware` nudges the LLM to call tools instead of just describing actions

## Quick Start

### 1. Define a Tool

Any capability you want your agent to have is expressed as a `Tool`:

```rust
use agent_base::{Tool, ToolContext, ToolOutput, ToolControlFlow, AgentResult};
use async_trait::async_trait;
use serde_json::{json, Value};

struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &'static str { "get_weather" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get current weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string", "description": "City name" }
                    },
                    "required": ["city"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let city = args["city"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("Weather in {}: 22°C, sunny", city),
            raw: None,
            control_flow: ToolControlFlow::Continue,
            truncated: false,
        })
    }
}
```

### 2. Build the Agent

```rust
use std::sync::Arc;
use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, RunOutcome,
    OpenAiClient, StopOnError,
};

#[tokio::main]
async fn main() -> AgentResult<()> {
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").unwrap(),
        "gpt-4o".into(),
        None,
    ));

    let mut runtime = AgentBuilder::new(llm)
        .system_prompt("You are a helpful weather assistant.")
        .register_tool(WeatherTool)
        .build();

    let session_id = runtime.create_session();
    let (events, outcome) = runtime.run_turn_stream(
        session_id,
        "What's the weather in Tokyo?",
    ).await?;

    for event in &events {
        match event {
            AgentEvent::TextDelta { text, .. } => print!("{}", text),
            AgentEvent::ToolCallStarted { tool_name, .. } => {
                println!("\n[Calling tool: {}]", tool_name);
            }
            AgentEvent::ToolCallFinished { summary, .. } => {
                println!("[Tool result: {}]", summary);
            }
            AgentEvent::RunFinished { .. } => println!("\n[Done]"),
            _ => {}
        }
    }

    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
}
```

### 3. Handle Tool Errors

By default, tool failures stop the run. For self-healing agents (e.g. code agents that retry compilation), inject `RetryOnError`:

```rust
use agent_base::RetryOnError;

let mut runtime = AgentBuilder::new(llm)
    .register_tool(MyTool)
    .error_recovery(Arc::new(RetryOnError))  // ← retry on failure
    .build();
```

### 4. Add Approval for Sensitive Tools

```rust
use agent_base::{
    ApprovalHandler, ApprovalRequest, ApprovalDecision,
    ToolPolicy, RiskLevel,
};

struct MyApprovalHandler;
#[async_trait::async_trait]
impl ApprovalHandler for MyApprovalHandler {
    async fn approve(&self, _req: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        // Ask user via UI, CLI, etc.
        Ok(ApprovalDecision::AllowOnce)
    }
}

struct MyToolPolicy;
impl ToolPolicy for MyToolPolicy {
    fn evaluate_approval(&self, tool_name: &str, _args: &Value, _json: &str)
        -> Option<ApprovalRequest>
    {
        if tool_name == "dangerous_tool" {
            Some(ApprovalRequest {
                title: "Confirm action".into(),
                message: format!("Execute `{}`?", tool_name),
                risk_level: RiskLevel::Sensitive,
                ..Default::default()
            })
        } else {
            None  // auto-allow
        }
    }
}

let mut runtime = AgentBuilder::new(llm)
    .register_tool(DangerousTool)
    .tool_policy(Arc::new(MyToolPolicy))
    .approval_handler(Arc::new(MyApprovalHandler))
    .build();
```

### 5. Use a Sub-Agent

```rust
use agent_base::SubAgentTool;

// Build a sub-agent runtime
let sub_llm = Arc::new(OpenAiClient::new(key, model, None));
let sub_runtime = AgentBuilder::new(sub_llm)
    .system_prompt("You are a math expert.")
    .build();

// Wrap it as a tool
let math_tool = SubAgentTool::new(
    "calculate",
    "Delegate math problems to a math expert",
    sub_runtime,
);

// Register in the parent agent
let mut parent = AgentBuilder::new(parent_llm)
    .register_tool(math_tool)
    .build();
```

Each sub-agent call creates a fresh session by default. Use `SubAgentTool::with_persistent()` to share context across calls.

## Examples

```bash
# Configure API key
cp .env.example .env
# Edit .env with your OPENAI_API_KEY or ANTHROPIC_API_KEY

# Run the REPL example
cargo run --example repl

# Run the SubAgent demo
cargo run --example subagent_demo

# Run the Middleware demo
cargo run --example middleware_demo

# Run the Plan demo
cargo run --example plan_demo
```

## What agent-base Does NOT Do

- Built-in SSH, filesystem, or database tools
- Workflow DAG or multi-agent orchestration engine
- Memory or RAG (Retrieval-Augmented Generation) framework
- Terminal UI or built-in approval dialog
- Production-grade persistence or transaction system

Business-specific tools and strategies belong in **upper layers** (e.g. `ops-agent`, `agent-works`, `db-agent`, `browser-agent`).

## Typical Layering

```
ops-agent / agent-works / ...          ← Business agents / Enhanced toolkits
    └── agent-base                      ← Lightweight Runtime Kernel
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

This project is in early development (v0.1.1). The core abstractions are settling but not yet frozen. Expect minor API changes as the ecosystem evolves.

## License

MIT

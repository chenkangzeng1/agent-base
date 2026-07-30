# Quick Start: Build Your Own Agent in 5 Minutes

> From zero to a working agent with tools, approval flow, and event streaming.

This guide walks you through building a **Server Health Check Agent** — a practical scenario that demonstrates all core `agent-base` concepts. No boilerplate fluff, just the real patterns you'll use.

---

## What You'll Build

A CLI agent that can:
- Check disk usage on remote servers
- Check memory status
- Restart services (with human approval)
- Stream events in real-time

By the end, you'll understand: **Tools → Runtime → Events → Approval → Middleware**.

---

## Step 0: Project Setup

```bash
mkdir my-agent && cd my-agent
cargo init
```

Add to `Cargo.toml`:

```toml
[dependencies]
agent-base = "0.1.2"
async-trait = "0.1"
serde_json = "1"
tokio = { version = "1", features = ["full"] }
dotenvy = "0.15"
```

---

## Step 1: Define Your First Tool

A **Tool** is anything the LLM can invoke. It has three parts:
- `name()` — unique identifier the LLM uses to call it
- `definition()` — JSON Schema telling the LLM what arguments to send
- `call()` — async execution logic

```rust
// src/tools.rs
use agent_base::{Tool, ToolContext, ToolOutput, ToolControlFlow, AgentResult};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct DiskCheckTool;

#[async_trait]
impl Tool for DiskCheckTool {
    fn name(&self) -> &'static str {
        "check_disk"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_disk",
                "description": "Check disk usage on the server. Returns used/total space and usage percentage.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Filesystem path to check (e.g. '/', '/home', '/var')"
                        }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("/");
        // In a real agent, you'd SSH or use sysinfo here.
        // For this tutorial, we simulate the output.
        let output = format!(
            "Filesystem: {}\nSize: 50G  Used: 32G  Avail: 18G  Use%: 64%",
            path
        );
        Ok(ToolOutput {
            summary: output,
            raw: Some(json!({ "path": path, "used_gb": 32, "total_gb": 50, "percent": 64 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}
```

> **Key insight:** `ToolControlFlow::Continue` tells the runtime "let the LLM keep reasoning after this tool". Use `Break` when a tool is a final answer (like a calculator).

---

## Step 2: Wire Up the Runtime

The **AgentBuilder** is your entry point. It configures the LLM, registers tools, and builds the runtime.

```rust
// src/main.rs
mod tools;

use std::sync::Arc;
use agent_base::{AgentBuilder, RuntimeEvent, AgentResult, OpenAiClient, RunOutcome, RetryOnError};
use tools::DiskCheckTool;

const SYSTEM_PROMPT: &str = r#"You are a server health check assistant.
You can check disk usage and memory status.
When users ask about server health, call the appropriate tools.
Be concise. Report findings as bullet points."#;

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    // 1. Create LLM client (OpenAI-compatible)
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").expect("Set OPENAI_API_KEY"),
        "gpt-4o-mini",           // use any OpenAI-compatible model
        None,                    // or Some("https://your-proxy.com/v1")
    ));

    // 2. Build the runtime
    let runtime = AgentBuilder::new(llm)
        .system_prompt(SYSTEM_PROMPT)
        .register_tool(DiskCheckTool)
        // .error_recovery(Arc::new(RetryOnError))  // uncomment for self-healing
        .build()?;

    // 3. Create a session and run
    let session_id = runtime.create_session().await;

    let (events, outcome) = runtime
        .run_turn_collect(session_id, "Check disk usage on /")
        .await?;

    // 4. Print results
    for event in &events {
        match event {
            RuntimeEvent::TextDelta { text, .. } => print!("{}", text),
            RuntimeEvent::ToolCallStarted { tool_name, .. } => {
                println!("\n🔧 Calling: {}", tool_name);
            }
            RuntimeEvent::ToolCallFinished { summary, .. } => {
                println!("✅ Result: {}", summary);
            }
            RuntimeEvent::RunFinished { .. } => println!("\n[Done]"),
            _ => {}
        }
    }

    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
}
```

Run it:

```bash
OPENAI_API_KEY=sk-xxx cargo run
```

**That's it.** The LLM will see the tool definition, decide to call `check_disk`, get the result, and format a response. The runtime handles the full loop: LLM → tool call → execute → feed result back → LLM responds.

---

## Step 3: Add a Dangerous Tool with Approval

Some tools need human approval (restarting services, deleting data). `agent-base` splits this into two traits:

| Trait | Role | When it runs |
|---|---|---|
| `ToolPolicy` | Decides *if* a tool needs approval | Before every tool call (sync, stateless) |
| `ApprovalHandler` | Executes the approval *interaction* | Only when policy says approval is needed (async) |

```rust
// src/approval.rs
use agent_base::{
    ApprovalHandler, ApprovalRequest, ApprovalDecision,
    ToolPolicy, ToolContext, AgentResult, RiskLevel,
};
use async_trait::async_trait;
use serde_json::Value;

/// Policy: restart_service needs approval, everything else is auto-allowed
pub struct HealthCheckPolicy;

#[async_trait]
impl ToolPolicy for HealthCheckPolicy {
    async fn evaluate_approval(
        &self,
        tool_name: &str,
        _args: &Value,
    ) -> Option<ApprovalRequest> {
        match tool_name {
            "restart_service" => Some(ApprovalRequest {
                title: "Service Restart".into(),
                message: format!("Allow restarting service?"),
                risk_level: RiskLevel::Sensitive,
                action_key: Some(format!("restart:{}", _args.get("service").unwrap_or(&Value::Null))),
                raw: None,
            }),
            _ => None, // all other tools auto-approve
        }
    }

    fn before_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }

    fn after_call(
        &self, _tool_name: &str, _args: &Value,
        _result: &agent_base::ToolOutput, _ctx: &ToolContext,
    ) -> AgentResult<()> {
        Ok(())
    }
}

/// Handler: CLI-based approval (ask user in terminal)
pub struct CliApproval;

#[async_trait]
impl ApprovalHandler for CliApproval {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!("\n⚠️  Approval needed: {}", request.title);
        println!("   Risk: {:?}", request.risk_level);
        println!("   {}", request.message);
        print!("   Allow? [y/n]: ");
        // In production, read from stdin. Here we auto-approve for demo.
        Ok(ApprovalDecision::AllowOnce)
    }
}
```

Register them in `main.rs`:

```rust
use approval::{HealthCheckPolicy, CliApproval};

let runtime = AgentBuilder::new(llm)
    .system_prompt(SYSTEM_PROMPT)
    .register_tool(DiskCheckTool)
    .register_tool(RestartServiceTool)  // define this yourself
    .tool_policy(Arc::new(HealthCheckPolicy))
    .approval_handler(Arc::new(CliApproval))
    .build()?;
```

**How it flows:**
```
LLM decides to call "restart_service"
    → ToolPolicy::evaluate_approval() returns Some(ApprovalRequest)
    → ApprovalHandler::approve() asks the human
    → Approved? → tool executes
    → Denied?  → LLM gets a "denied" message and adapts
```

---

## Step 4: Add a Middleware

**Middleware** hooks into three points of the agent loop:

| Hook | When | Use case |
|---|---|---|
| `on_user_message` | Before the user message enters the session | Input sanitization, command rewriting |
| `on_pre_llm` | Before sending to the LLM | Inject extra context, filter messages |
| `on_post_llm` | After the LLM responds | Suppress hallucinations, block sensitive output |

Here's a practical example — **anti-hallucination middleware** that nudges the LLM to call tools instead of just describing what it would do:

```rust
// src/middleware.rs
use std::sync::atomic::{AtomicUsize, Ordering};
use agent_base::{Middleware, PostLlmCtx, AgentResult};
use async_trait::async_trait;

pub struct ToolEnforcement {
    max_nudges: usize,
    nudge_count: AtomicUsize,
}

impl ToolEnforcement {
    pub fn new(max_nudges: usize) -> Self {
        Self {
            max_nudges,
            nudge_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Middleware for ToolEnforcement {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        // Only nudge if: tools are available, LLM didn't call any, and hasn't been nudged too many times
        if ctx.available_tools.is_empty()
            || ctx.is_tool_call
            || ctx.full_text.is_empty()
            || ctx.total_tool_calls > 0
        {
            return Ok(());
        }

        let count = self.nudge_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_nudges {
            return Ok(()); // give up after max nudges
        }

        // Suppress the text response and inject a follow-up instruction
        ctx.skip_push = true;
        ctx.follow_up_message = Some(
            "You have tools available. Call them now instead of describing what you would do.".into()
        );
        Ok(())
    }
}
```

Register it:

```rust
use middleware::ToolEnforcement;

let runtime = AgentBuilder::new(llm)
    .system_prompt(SYSTEM_PROMPT)
    .register_tool(DiskCheckTool)
    .middleware(ToolEnforcement::new(3))  // nudge up to 3 times
    .build()?;
```

---

## Step 5: Real-Time Event Streaming

Instead of collecting all events and printing at the end, stream them live using `run_turn`:

```rust
use std::io::{self, Write};
use agent_base::{RuntimeEvent, AgentResult};

// This is a synchronous callback — called for each event as it happens
fn on_event(event: RuntimeEvent) -> AgentResult<()> {
    match event {
        RuntimeEvent::TextDelta { text, .. } => {
            print!("{}", text);
            io::stdout().flush().unwrap();
        }
        RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
            println!("\n🔧 {}({})", tool_name, args_json);
        }
        RuntimeEvent::ToolCallFinished { summary, .. } => {
            // Truncate long outputs for display
            let display = if summary.len() > 200 {
                format!("{}...", &summary[..200])
            } else {
                summary.clone()
            };
            println!("  → {}", display);
        }
        RuntimeEvent::RunFinished { .. } => {
            println!("\n✅ Done");
        }
        _ => {}
    }
    Ok(())
}

// Use it:
let outcome = runtime
    .run_turn(session_id, "Check disk and memory", on_event)
    .await?;
```

> **Tip:** `run_turn` is for real-time streaming (CLI, WebSocket). `run_turn_collect` collects all events into a Vec — good for testing or batch processing.

---

## Step 6: Multi-Turn Conversation

Sessions persist message history automatically. Just keep calling `run_turn_*` with the same `session_id`:

```rust
let session_id = runtime.create_session().await;

// Turn 1: Ask about disk
runtime.run_turn(session_id.clone(), "Check disk on /", on_event).await?;

// Turn 2: Follow-up — the LLM remembers the previous context
runtime.run_turn(session_id.clone(), "What about /var?", on_event).await?;

// Turn 3: Decision making — LLM has full context of both checks
runtime.run_turn(session_id.clone(), "Which one should I worry about?", on_event).await?;
```

The runtime automatically manages the message history (user, assistant, tool calls, tool results) inside the session.

---

## Step 7: Error Recovery

By default, tool failures **stop the run** (`StopOnError`). For agents that should self-heal (e.g., retry a failed command), inject `RetryOnError`:

```rust
use agent_base::RetryOnError;

let runtime = AgentBuilder::new(llm)
    .register_tool(DiskCheckTool)
    .error_recovery(Arc::new(RetryOnError))  // feed error back to LLM for retry
    .build()?;
```

With `RetryOnError`, when a tool fails:
1. The error message is injected into the conversation as a user message
2. The LLM sees the error and can adjust its approach
3. The loop continues (up to `max_turns`)

You can also implement custom recovery logic:

```rust
use agent_base::{ToolErrorRecovery, ToolErrorAction, AgentResult, SessionId, AgentError};

struct SmartRecovery;

impl ToolErrorRecovery for SmartRecovery {
    fn on_error(
        &self,
        _session_id: &SessionId,
        tool_names: &[String],
        error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        // Retry SSH timeouts, but stop on auth failures
        if error.to_string().contains("timeout") {
            Ok(ToolErrorAction::Retry)
        } else {
            Ok(ToolErrorAction::Stop)
        }
    }
}
```

---

## Complete Minimal Agent (Copy-Paste Ready)

```rust
// src/main.rs
use std::sync::Arc;
use std::io::{self, Write};

use agent_base::{
    AgentBuilder, RuntimeEvent, AgentResult, OpenAiClient, RunOutcome,
    Tool, ToolContext, ToolOutput, ToolControlFlow,
};
use async_trait::async_trait;
use serde_json::{json, Value};

struct CheckDiskTool;

#[async_trait]
impl Tool for CheckDiskTool {
    fn name(&self) -> &'static str { "check_disk" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_disk",
                "description": "Check disk usage for a given path",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to check" }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("/");
        Ok(ToolOutput {
            summary: format!("{}: 50G total, 32G used (64%)", path),
            raw: Some(json!({ "path": path, "percent": 64 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

fn on_event(event: RuntimeEvent) -> AgentResult<()> {
    match event {
        RuntimeEvent::TextDelta { text, .. } => { print!("{}", text); io::stdout().flush().unwrap(); }
        RuntimeEvent::ToolCallStarted { tool_name, .. } => println!("\n🔧 {}", tool_name),
        RuntimeEvent::ToolCallFinished { summary, .. } => println!("  → {}", summary),
        RuntimeEvent::RunFinished { .. } => println!("\n✅"),
        _ => {}
    }
    Ok(())
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    let llm = Arc::new(OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").expect("Set OPENAI_API_KEY"),
        "gpt-4o-mini", None,
    ));

    let runtime = AgentBuilder::new(llm)
        .system_prompt("You are a server health assistant. Use tools to check status. Be concise.")
        .register_tool(CheckDiskTool)
        .build()?;

    let session_id = runtime.create_session().await;

    // REPL loop
    loop {
        print!("> ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();
        if input == "exit" { break; }

        match runtime.run_turn(session_id.clone(), input, on_event).await {
            Ok(_) => {}
            Err(e) => eprintln!("Error: {}", e),
        }
    }
    Ok(())
}
```

```bash
cargo run
# > Check disk on /
# 🔧 check_disk
#   → /: 50G total, 32G used (64%)
# Based on the disk usage report for /, the filesystem has 50G total...
# ✅
```

---

## Concepts Cheat Sheet

| Concept | What | When to use |
|---|---|---|
| `Tool` trait | Define a capability the LLM can invoke | Every agent needs at least one |
| `ToolControlFlow::Continue` | Let the LLM keep reasoning after this tool | Multi-step tools, probes |
| `ToolControlFlow::Break` | End the turn after this tool | Final-answer tools, calculators |
| `ToolPolicy` | Decide if a tool needs human approval | Sensitive operations |
| `ApprovalHandler` | Execute the approval UI/flow | Always paired with ToolPolicy |
| `Middleware` | Hook into the agent loop | Input/output filtering, nudging |
| `ToolErrorRecovery` | What happens when a tool fails | `StopOnError` (default) or `RetryOnError` |
| `RuntimeEvent` stream | Real-time events from the runtime | UI updates, logging, debugging |
| `SessionId` | Multi-turn conversation handle | Every REPL or chat UI |
| `SubAgentTool` | Wrap another agent as a tool | Delegation, specialist agents |

---

## What's Next

- **Sub-agents**: Build specialist agents and compose them as tools → `examples/subagent_demo.rs`
- **Plan orchestrator**: Multi-step task planning and execution → `examples/plan_demo.rs`
- **Middleware patterns**: Anti-hallucination, content filtering → `examples/middleware_demo.rs`
- **Custom LLM provider**: Implement the `LlmClient` trait for your own provider

---

## Typical Layering

```
your-domain-agent (e.g. ops-agent, db-agent)
    ├── Domain-specific tools (SSH, SQL, API calls)
    ├── Domain-specific middleware (hallucination guard, safety)
    └── agent-base (runtime kernel)
         ├── LLM abstraction
         ├── Tool dispatch
         ├── Approval flow
         └── Event streaming
```

`agent-base` does the orchestration. **You** bring the domain knowledge.
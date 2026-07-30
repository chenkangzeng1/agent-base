# Getting Started

5 minutes to your first phi-agent.

## Prerequisites

- [Rust](https://rustup.rs) (stable, edition 2024)
- An LLM API key (OpenAI-compatible endpoint)

## Install

```bash
cargo install phi-agent
```

## Option 1: Scaffold a project (recommended)

Use `phi init` to generate a complete project with an example tool and REPL:

```bash
phi init my-agent
cd my-agent
cp .env.example .env   # edit with your API key
cargo run
```

```
phi> What time is it?
🔧 get_time
 Current time: 2025-07-30 19:30:00

phi> /exit
```

Open `src/main.rs` — you'll see three parts:

**1. Define a tool** — implement the `Tool` trait:

```rust
struct ClockTool;

#[async_trait]
impl Tool for ClockTool {
    fn name(&self) -> &'static str { "get_time" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_time",
                "description": "Get the current date and time",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        Ok(ToolOutput {
            summary: format!("Current time: {}", now),
            control_flow: ToolControlFlow::Continue,
            raw: None, truncation: None,
        })
    }
}
```

**2. Register the tool** — attach it to the Agent:

```rust
let agent = PhiAgent::build(
    base_agent_builder(llm)
        .system_prompt(build_system_prompt())
        .register_tool(ClockTool),      // ← register here
    PhiAgentConfig { ... },
)?;
```

**3. REPL** — the Agent decides when to call your tool.

Model your own tool after `ClockTool`. See [Custom Tools](custom-tool.md) for more examples.

## Option 2: Library integration

Add phi-agent to an existing project:

```bash
cargo new my-agent && cd my-agent
cargo add phi-agent tokio --features full anyhow dotenvy async-trait serde_json chrono rustyline
```

Create a `.env` file with your API key, then copy this to `src/main.rs`:

```rust
use phi_agent::{
    base_agent_builder, build_system_prompt,
    PhiAgent, PhiAgentConfig, OpenAiClient,
    SafetyConfig, ReasoningEffort,
    OutputFormat, create_stdout_renderer,
    AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use rustyline::DefaultEditor;
use serde_json::{Value, json};
use std::sync::Arc;

// ── ClockTool ──

struct ClockTool;

#[async_trait]
impl Tool for ClockTool {
    fn name(&self) -> &'static str { "get_time" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_time",
                "description": "Get the current date and time",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        Ok(ToolOutput {
            summary: format!("Current time: {}", now),
            control_flow: ToolControlFlow::Continue,
            raw: None, truncation: None,
        })
    }
}

// ── REPL ──

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".into());
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("LLM_API_KEY")?,
        model.clone(),
        std::env::var("LLM_BASE_URL").ok(),
    ));

    let agent = PhiAgent::build(
        base_agent_builder(llm)
            .system_prompt(build_system_prompt())
            .register_tool(ClockTool),
        PhiAgentConfig {
            model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    let mut rl = DefaultEditor::new()?;
    let mut renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true, show_tool_args: true, color: true,
    });

    println!("phi-agent REPL — type /exit to quit\n");
    loop {
        let line = rl.readline("phi> ")?;
        let input = line.trim().to_string();
        if input.is_empty() { continue; }
        if input == "/exit" { break; }
        rl.add_history_entry(&input)?;

        let session = agent.create_session().await;
        agent.run_turn(session, &input, |event| renderer.render(event)).await?;
        println!();
    }
}
```

Three steps: Define Tool → Register → REPL. `cargo run` to start.
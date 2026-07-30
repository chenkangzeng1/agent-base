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
cargo add phi-agent tokio --features full anyhow dotenvy async-trait serde_json chrono
```

Then write your `main.rs` following the `ClockTool` example above.
# agent-works

[![crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works)
[![Documentation](https://docs.rs/agent-works/badge.svg)](https://docs.rs/agent-works)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Batteries-included Agent toolbox built on [agent-base](https://github.com/chenkangzeng1/agent-base).**

`agent-works` adds production-ready capabilities on top of the `agent-base` runtime kernel: MCP multi-server management, Skills with progressive disclosure, built-in file tools, tool enforcement middleware, and a CLI REPL loop — all behind feature flags. Pick what you need.

## Relationship with agent-base

```
agent-base         Pure runtime kernel (~12 deps, trait interfaces only)
    ↑
agent-works        Batteries-included toolbox (wraps agent-base + enhancements)
```

- **Use `agent-base` alone** when you only need the runtime (LLM + tools + middleware).
- **Use `agent-works`** when you want MCP, Skills, built-in tools, CLI — and still get everything from agent-base through re-exports.
- Switching from `agent-base` to `agent-works` is a one-line import change.

## Installation

```toml
[dependencies]
agent-works = { version = "0.1.1", features = ["full"] }
```

Or pick specific features:

```toml
agent-works = { version = "0.1.1", features = ["mcp", "skill"] }
```

## Feature Flags

| Feature | Description | Extra deps |
|---------|-------------|------------|
| `mcp` | `McpHUb` — multi-server MCP with HTTP + stdio transport | — |
| `skill` | `Skill` trait + `LazySkillPrompter` / `FullDetailPrompter` + `SkillDetailTool` + `SkillLoader` | — |
| `builtin-tools` | `ReadFileTool`, `WriteFileTool`, `ListDirectoryTool`, `FileExistsTool`, `SearchReplaceTool` | `walkdir` |
| `cli` | `CliRepl` (generic REPL loop) + `CliEventPrinter` (terminal event output) | — |
| `full` | All of the above | — |

All types from `agent-base` are re-exported (`AgentBuilder`, `AgentRuntime`, `Tool`, `Middleware`, ...), so you only need to depend on `agent-works`.

## Quick Start

### Skills

Skills package tools + descriptions into reusable units with progressive disclosure:

```rust
use std::sync::Arc;
use agent_works::{
    AgentBuilder, AgentEvent, AgentResult,
    skill::{Skill, LazySkillPrompter},
    Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// 1. Define tools
struct AddTool;
#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &'static str { "add" }
    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "add",
                "description": "Calculate the sum of two integers",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer" },
                        "b": { "type": "integer" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }
    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        Ok(ToolOutput {
            summary: format!("{a} + {b} = {}", a + b),
            raw: Some(json!({ "result": a + b })),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}

// 2. Pack into a Skill
struct MathSkill;
impl Skill for MathSkill {
    fn name(&self) -> &'static str { "math" }
    fn brief_description(&self) -> String {
        "Math: supports addition".to_string()
    }
    fn detailed_description(&self) -> String {
        "## Math Skill\n\n- **add**: Calculate the sum of two integers".to_string()
    }
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(AddTool)]
    }
}

// 3. Build with agent-works AgentBuilder
let runtime = AgentBuilder::new(llm)
    .system_prompt("You are a helpful assistant.")
    .register_skill(MathSkill)  // auto-registers tools, injects prompt, adds detail tool
    .build()?;
```

The builder automatically:
- Registers skill tools and detects name conflicts
- Injects skill brief descriptions into the system prompt (via `LazySkillPrompter`)
- Registers `SkillDetailTool` for on-demand detailed prompt loading

### MCP Multi-Server

```rust
use agent_works::mcp::*;

let mut hub = McpHUb::new();
hub.add_server(McpServerConfig {
    name: "filesystem".into(),
    transport: McpTransport::Stdio {
        command: "npx".into(),
        args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
    },
    auto_reconnect: true,
});
hub.connect_all().await?;

// Discover tools from all servers
let all_tools = hub.discover_all().await?;

// Register into the agent runtime
let mut tools = runtime.tools_mut();
hub.register_all(&mut tools);
```

### Built-in File Tools

```rust
use agent_works::builtin::{ReadFileTool, WriteFileTool};
use std::path::PathBuf;

let runtime = AgentBuilder::new(llm)
    .register_tool(ReadFileTool { workspace: PathBuf::from(".") })
    .register_tool(WriteFileTool { workspace: PathBuf::from(".") })
    .build()?;
```

### CLI REPL

```rust
use agent_works::cli::{CliRepl, CliEventPrinter};

let mut repl = CliRepl::new(runtime);

// Register custom shell commands
repl.register_shell_command("time", Box::new(|_| {
    println!(">>> {}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    true
}));

repl.run().await?;
```

### Tool Enforcement

The `ToolEnforcementMiddleware` (inherited from agent-base) nudges the LLM to actually call tools instead of just describing what it would do:

```rust
use agent_works::ToolEnforcementMiddleware;
use agent_works::ToolEnforcementConfig;

let runtime = AgentBuilder::new(llm)
    .register_tool(MyTool)
    .middleware(ToolEnforcementMiddleware::new(ToolEnforcementConfig::default()))
    .build()?;
```

## Examples

```bash
# Skills with progressive disclosure
cargo run --example skill_demo --features skill

# MCP multi-server connection
cargo run --example mcp_demo --features mcp

# Built-in file tools
cargo run --example builtin_demo --features builtin-tools

# CLI REPL + event printer
cargo run --example cli_demo --features cli
```

## Module Structure

```
src/
├── lib.rs              # Re-exports agent-base + feature-gated modules
├── builder.rs          # AgentBuilder wrapper with skill integration
├── mcp/                # McpHUb + McpClient (HTTP + stdio transport)
├── skill/              # Skill trait + prompter strategies + detail tool
├── builtin/            # ReadFile / WriteFile / ListDirectory / FileExists / SearchReplace
└── cli/                # CliRepl + CliEventPrinter
```

## License

MIT

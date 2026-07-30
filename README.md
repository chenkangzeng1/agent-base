# <picture><source media="(prefers-color-scheme: dark)" srcset="assets/logo.svg"><img alt="phi-agent" src="assets/logo.svg" height="60"></picture>

[![CI](https://github.com/hibuka-labs/phi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-book-green.svg)](https://hibuka-labs.github.io/phi-agent)

A general-purpose AI Agent framework in Rust, built on [agent-base](https://crates.io/crates/agent-base) and [agent-works](https://crates.io/crates/agent-works).

**phi-agent provides the infrastructure. You bring the tools.**

## Ecosystem

phi-agent is part of a family of independent crates:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| `agent-base` | [![Crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base) | Lightweight runtime kernel — LLM clients, Tool trait, event stream |
| `agent-works` | [![Crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works) | Batteries-included toolbox — MCP, Skills, built-in file tools |
| `phi-agent` | [![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent) | Full framework — builder factory, renderers, config, CLI binary |

**Just need the runtime?** `cargo add agent-base`. **Need the full framework?** `cargo add phi-agent`.

## Architecture

```
                      ┌─────────────────────┐
                      │     agent-base       │
                      │  Tool trait · Runtime │
                      │  LLM clients · Events  │
                      └──────────┬──────────┘
                                 │
          ┌──────────────────────┼──────────────────────┐
          │                      │                      │
┌─────────▼─────────┐  ┌────────▼────────┐  ┌──────────▼──────────┐
│    agent-works     │  │   phi-tools     │  │    your-tools       │
│  MCP · Skills      │  │ LocalShellTool  │  │ Custom Tool impls   │
│  Builtin tools     │  │                 │  │                     │
└─────────┬─────────┘  └────────┬────────┘  └──────────┬──────────┘
          │                      │                      │
          └──────────────────────┼──────────────────────┘
                                 │
                      ┌──────────▼──────────┐
                      │     phi-agent        │
                      │  Builder factory     │
                      │  Renderers (3)       │
                      │  Config · Session    │
                      │  CLI (phi)            │
                      └──────────┬──────────┘
                                 │
                    ┌────────────┼────────────┐
                    │            │            │
              ┌─────▼────┐ ┌────▼─────┐ ┌────▼─────┐
              │ Terminal  │ │  JSON    │ │   Web    │
              │   REPL    │ │  Stream  │ │ Backend  │
              └───────────┘ └──────────┘ └──────────┘
```

**Core principle**: phi-agent itself does **not** bundle any tools. It provides the agent builder factory, renderers, config resolution, and session management — tools are injected by consumers.

## Why phi-agent

**Simple.** A tool is 3 methods: `name()`, `definition()`, `call()`. No framework to learn, no abstractions to fight.

**Rust.** Single binary, no runtime dependency. `cargo install` and you're done. Memory-safe, crash-resistant, fast. Deploy anywhere — from cloud servers to edge devices.

**Pure.** No built-in memory, no vector database, no hidden state. The agent doesn't remember anything you don't tell it to. Predictable, debuggable, and you control where your data goes.

**Your tools, your rules.** phi-agent doesn't know what tools exist. You bring them, you own them. No vendor lock-in.

## Features

- **Builder factory** — `base_agent_builder()` with sensible defaults (thinking, recovery, limits)
- **Three renderers** — Terminal (rich, colored, streaming), JSON stream (JSONL), Null (silent)
- **CLI-ready** — REPL and one-shot modes with 30+ configurable flags
- **Session management** — auto-cleanup, file locking, JSONL turn logging
- **Tool-agnostic** — no built-in tools; register your own via `AgentBuilder`
- **Extensible** — middleware, approval handlers, custom renderers

## Quick Start

```rust
use phi_agent::{
    base_agent_builder, build_system_prompt, PhiAgent, PhiAgentConfig,
    OpenAiClient, SafetyConfig, ReasoningEffort,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Create LLM client
    let llm_client = Arc::new(OpenAiClient::new(
        std::env::var("LLM_API_KEY")?,
        "opus".into(),
        Some("https://api.openai.com/v1".into()),
    ));

    // 2. Build agent (register your tools here)
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt())
        .register_tool(your_tool);

    let agent = PhiAgent::build(builder, PhiAgentConfig {
        model: "opus".into(),
        enable_thinking: true,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
    })?;

    // 3. Run
    let session = agent.create_session().await;
    let renderer = phi_agent::create_stdout_renderer(
        &phi_agent::OutputFormat::Terminal {
            show_thinking: true,
            show_tool_args: true,
            color: true,
        }
    );

    agent.run_turn(session, "Hello!", |event| {
        renderer.render(event)
    }).await?;

    Ok(())
}
```

See [examples/](examples/) for more complete examples.

## CLI

```bash
cargo install phi-agent
phi "What's in this directory?"
```

```bash
# REPL mode
phi

# JSON output for scripting
phi --format json "list files"
```

## Custom Tool Example

```rust
use agent_base::{Tool, ToolContext, ToolOutput, ToolControlFlow, AgentResult};
use serde_json::{Value, json};
use async_trait::async_trait;

struct HelloTool;

#[async_trait]
impl Tool for HelloTool {
    fn name(&self) -> &'static str { "hello" }

    fn definition(&self) -> Value {
        json!({
            "name": "hello",
            "description": "Say hello to someone",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Who to greet" }
                },
                "required": ["name"]
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("world");
        Ok(ToolOutput { summary: format!("Hello, {}!", name), control_flow: ToolControlFlow::Continue, raw: None, truncation: None })
    }
}
```

Full guide: [guide/custom-tool.md](guide/custom-tool.md)

## Documentation

📖 **Full documentation**: [hibuka-labs.github.io/phi-agent](https://hibuka-labs.github.io/phi-agent)

| Document | Description |
|----------|-------------|
| [Getting Started](guide/getting-started.md) | 5-minute quick start |
| [Custom Tools](guide/custom-tool.md) | How to write a Tool |
| [Configuration](guide/configuration.md) | Config reference |
| [Focus](guide/focus.md) | Structured single-purpose LLM calls |
| [Advanced](guide/advanced.md) | Middleware, sessions, event log |

## FAQ

**Q: What's the difference between phi-agent and agent-base?**

agent-base is the runtime kernel (LLM calls, tool orchestration, event stream). phi-agent wraps it with a builder factory, renderers, config resolution, and session management — plus a CLI binary.

**Q: Can I use phi-agent without the CLI?**

Yes. Import it as a library (`phi_agent`) and use `PhiAgent::build()` programmatically. The CLI is just one consumer.

**Q: How do I add my own tools?**

Implement the `Tool` trait from `agent-base` and register with `builder.register_tool(...)`. phi-agent has zero knowledge of what tools exist.

**Q: Does phi-agent support Anthropic / other providers?**

Yes. agent-base provides `AnthropicClient` and `OpenAiClient`. Any client implementing `LlmClient` works.

## Contributing

```bash
git clone git@github.com:hibuka-labs/phi-agent.git
cd phi-agent
cargo check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed setup instructions and PR guidelines.

## License

MIT — see [LICENSE](LICENSE) for details.

[中文文档](README_CN.md)

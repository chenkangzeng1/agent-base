# <picture><source media="(prefers-color-scheme: dark)" srcset="assets/logo.svg"><img alt="phi-agent" src="assets/logo.svg" height="60"></picture>

[![CI](https://github.com/hibuka-labs/phi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-book-green.svg)](https://docs.phi-agent.dev)

A general-purpose AI Agent framework in Rust, built on [agent-base](https://crates.io/crates/agent-base) and [agent-works](https://crates.io/crates/agent-works).

**phi-agent provides the infrastructure. You bring the tools.**

## Ecosystem

phi-agent is part of a family of independent crates:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| `agent-base` | [![Crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base) | Lightweight runtime kernel — LLM clients, Tool trait, event stream |
| `agent-works` | [![Crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works) | Batteries-included toolbox — MCP, Skills, Focus |
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
│  Focus             │  │                 │  │                     │
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

**Rust.** Single binary, no runtime dependency. `cargo install` and you're done. Memory-safe, crash-resistant, fast. Deploy anywhere — from cloud servers to edge devices.

**Simple.** No hidden state, no magic. Explicit control flow that's easy to read, trace, and trust. A tool is 3 methods — `name()`, `definition()`, `call()`.

**Your tools, your rules.** phi-agent has zero built-in tools. You bring them, you own them. No vendor lock-in.

**Observable.** Built-in turn logging, session metrics, and tracing. Every decision is recorded, every outcome is measurable. Know exactly what your agent did and why.

## Features

- **Builder factory** — `base_agent_builder()` with sensible defaults (thinking, recovery, limits)
- **Three renderers** — Terminal (rich, colored, streaming), JSON stream (JSONL), Null (silent)
- **CLI-ready** — REPL and one-shot modes with 20+ configurable flags
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

## Browser Automation

phi-agent includes 21 browser tools (gated behind the `browser` Cargo feature) for web browsing, form interaction, and data extraction via Chrome DevTools Protocol.

### Quick Start

```bash
# Build and run with browser enabled
cargo run --features browser -- --enable-browser "上网查今天天气"

# Headed mode (visible browser window, useful for debugging)
cargo run --features browser -- --enable-browser --headed "打开淘宝搜索机械键盘"

# Connect to an existing Chrome instance
# First, start Chrome with remote debugging:
#   /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --remote-debugging-port=9222
cargo run --features browser -- --connect-ws ws://localhost:9222 "在当前页面查找..."
```

### Available Browser Tools (21)

| Category | Tools |
|---|---|
| **Navigation** | `browser_navigate`, `browser_go_back`, `browser_go_forward`, `browser_wait` |
| **Interaction** | `browser_click`, `browser_hover`, `browser_input_fill`, `browser_select`, `browser_press_key`, `browser_scroll` |
| **Viewing** | `browser_snapshot`, `browser_screenshot`, `browser_get_markdown`, `browser_read_links`, `browser_evaluate` |
| **Tab** | `browser_new_tab`, `browser_tab_list`, `browser_switch_tab`, `browser_close_tab` |
| **Control** | `browser_close`, `browser_extract_content` |

### How It Works

1. `--enable-browser` launches a headless Chrome instance
2. `browser_navigate` opens a URL and returns an ARIA accessibility snapshot with numbered interactive elements
3. The AI clicks elements by index (e.g., `browser_click index=5`) instead of writing fragile CSS selectors
4. `browser_screenshot` captures visual snapshots; `browser_get_markdown` extracts readable content

### Requirements

- Chrome or Chromium installed
- `cargo run --features browser` (the `browser` feature gates the heavy `headless_chrome` dependency)

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

📖 **Full documentation**: [docs.phi-agent.dev](https://docs.phi-agent.dev)

| Document | Description |
|----------|-------------|
| [Getting Started](guide/getting-started.md) | 5-minute quick start |
| [Custom Tools](guide/custom-tool.md) | How to write a Tool |
| [CLI Usage](guide/cli.md) | CLI flags, REPL, one-shot |
| [Configuration](guide/configuration.md) | Config reference |
| [Focus](guide/focus.md) | Structured single-purpose LLM calls |
| [Architecture](guide/architecture.md) | Design decisions and internals |
| [Observability](guide/observability.md) | Logging, tracing, metrics |
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

## Contact

:material-email-outline: [phiagent@hibuka.com](mailto:phiagent@hibuka.com)

[中文文档](README_CN.md)

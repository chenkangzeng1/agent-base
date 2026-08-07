# <picture><source media="(prefers-color-scheme: dark)" srcset="assets/logo.svg"><img alt="phi-agent" src="assets/logo.svg" height="60"></picture>

[![CI](https://github.com/hibuka-labs/phi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![codecov](https://codecov.io/gh/hibuka-labs/phi-agent/branch/master/graph/badge.svg)](https://codecov.io/gh/hibuka-labs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-book-green.svg)](https://docs.phi-agent.dev)
[![PyPI](https://img.shields.io/pypi/v/phi-agent.svg)](https://pypi.org/project/phi-agent/)

Not another AI Agent, but an open application framework for building Agents — purpose-built for embedded, edge, and vertical industries, equally suited for highly customizable, high-performance cloud and desktop AI applications. Simple, pure, predictable.

> **Unlike LangChain, CrewAI, or AutoGen, phi-agent ships with zero built-in tools.** No pre-packaged toolkits, no hidden prompt engineering, no magic workflow engine — just a clean Rust runtime. You define every tool, you control every behavior.

Built on [agent-base](https://crates.io/crates/agent-base) and [agent-works](https://crates.io/crates/agent-works). **phi-agent provides the infrastructure. You bring the tools.**

## Ecosystem

phi-agent is part of a family of independent crates:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| `agent-base` | [![Crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base) | Lightweight runtime kernel — LLM clients, Tool trait, event stream |
| `agent-works` | [![Crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works) | Batteries-included toolbox — MCP, Skills, Focus |
| `phi-agent` | [![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent) | Full framework — builder factory, renderers, config, CLI binary |

**Just need the runtime?** `cargo add agent-base`. **Need the full framework?** `cargo add phi-agent`.

## SDKs

Prefer Python? phi-agent supports multiple languages — write tools in your favorite language, powered by the same Rust runtime.

| Language | Package | Version |
|----------|---------|---------|
| Python | `pip install phi-agent` | [![PyPI](https://img.shields.io/pypi/v/phi-agent.svg)](https://pypi.org/project/phi-agent/) |

### Python

```bash
pip install phi-agent
```

```python
from phi_agent import Agent, tool

@tool
async def search(query: str) -> str:
    """Search the web."""
    return f"Results for: {query}"

agent = Agent(model="gpt-4o")
agent.register(search)

async for event in agent.run("What's new today?"):
    print(event)
```

The Python SDK communicates with the `phi` Rust binary over stdio — you write tools in Python, and the Rust runtime handles the agent loop, LLM calls, and event streaming.

📖 [Python SDK docs →](https://pypi.org/project/phi-agent/)

## Architecture

```mermaid
graph TB
    AB[agent-base<br/>Tool trait · Runtime<br/>LLM clients · Events]

    AB --> AW[agent-works<br/>MCP · Skills · Focus]
    AB --> PT[phi-tools<br/>LocalShellTool]
    AB --> YT[your-tools<br/>Custom Tool impls]

    AW --> PA[phi-agent<br/>Builder factory<br/>Renderers · Config · Session<br/>CLI binary]

    PT --> PA
    YT --> PA

    PA --> Terminal[Terminal REPL]
    PA --> JSON[JSON Stream]
    PA --> Web[Web Backend]
```

**Core principle**: phi-agent ships with **zero** built-in tools. You define them, you register them. phi-agent discovers and manages them at runtime — listing, logging, and routing tool calls automatically.

## Why phi-agent

**Built for Vertical Scenarios.** Not a generic chatbot, but an Agent framework for embedded systems, industrial, IoT, and other vertical domains, as well as desktop and cloud applications that demand deep customization — your scenario, your tools, your full control.

**Lightweight, Runs Anywhere.** A single Rust binary with zero runtime dependencies — from embedded Linux and edge gateways to cloud containers and desktop applications, `cargo install` gets you started in seconds, deploy anywhere.

**Zero Built-in Tools, Fully Customizable.** No pre-packaged tools, no platform lock-in — a tool is just 3 methods: `name()`, `definition()`, `call()`, you register what you need, the Agent uses what you register, only bring what your scenario truly needs, LLM freedom, precise and clean.

**Fully Observable, Every Step Explainable.** Every decision is logged, every step is traceable, with built-in session logging, structured tracing, and session metrics at a glance — compliance and audit trails without the stress.

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

See [examples/](examples/) for more complete examples, organized by category:
- [minimal/](examples/minimal/) — hello agent
- [tools/](examples/tools/) — custom tools and policies
- [mcp/](examples/mcp/) — MCP client and dynamic attach/detach
- [session/](examples/session/) — session persistence and lifecycle
- [observability/](examples/observability/) — event log, middleware hooks
- [advanced/](examples/advanced/) — window memory, summary memory, focus

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

## ✅ What phi-agent is Good For

- **Embedded & edge applications** — single binary, zero system dependencies, runs on ARM Linux and IoT gateways
- **Industrial & compliance workloads** — fully observable, every step logged to JSONL, audit trails out of the box
- **Desktop & cloud AI apps** — `cargo install` into CLI/backend, use as a library or standalone binary
- **Custom vertical agents** — you define every tool, you own every prompt, zero vendor lock-in
- **High-performance workflows** — Rust runtime, async I/O, sub-millisecond tool dispatch
- **Python + Rust hybrid** — write tools in Python, execution engine runs in Rust

## ⚠️ What phi-agent Does NOT Provide

phi-agent the **framework** is intentionally lean. The following are **explicitly out of scope** and will not be built into the framework itself (but some may be available as separate opt-in crates in the ecosystem):

- **Built-in tools** — the framework ships with zero tools. You define and register every tool yourself. For convenience, the ecosystem provides optional companion crates (e.g., `phi-tools`, the planned `phi-extra`), and more tool libraries may be contributed over time — all opt-in, none bundled.
- **Built-in memory / vector DB** — no Pinecone/Chroma/Weaviate integration, no automatic embedding. You manage state yourself.
- **Pre-built agent types** — no "research agent," "coding agent," "support agent" templates. You compose your own.
- **Workflow engine** — no DAG execution, no conditional branching engine, no LangGraph-style graph compiler. Agent behavior is driven by LLM tool-choice.
- **Prompt templates** — no langchain-style prompt chains, no automatic context stuffing. You control the system prompt.
- **Streaming HTTP server** — phi-agent is a library and CLI. You build the server layer (Actix/Axum/Warp) yourself.
- **Multi-agent orchestration** — currently not in scope (on the roadmap for v0.4.0, but will be a separate opt-in crate).

If you need these, combine phi-agent with:
- **Memory**: bring your own vector DB (Qdrant, pgvector, LanceDB)
- **Workflows**: use [LangGraph](https://www.langchain.com/langgraph) or [Temporal](https://temporal.io/) for orchestration
- **Tools**: use [phi-tools](https://crates.io/crates/phi-tools) for common utilities, or build your own
- **HTTP**: add [axum](https://crates.io/crates/axum) or [actix-web](https://crates.io/crates/actix-web)

## 🧩 phi-agent + LangGraph

phi-agent and **LangGraph** solve different problems and work well together:

| | phi-agent | LangGraph |
|---|---|---|
| **What it does** | Single-agent runtime | Multi-step workflow engine |
| **Strengths** | Fast tool dispatch, event streaming, embedded deployment | Graph-based control flow, checkpointing, human-in-the-loop |
| **How they fit** | Agent nodes inside a LangGraph graph | Orchestration layer above phi-agent agents |

**Recommended pattern**: Use LangGraph for workflow-level control flow (routing, branching, retries), and use phi-agent as the execution engine for individual agent nodes. phi-agent agents → LangGraph nodes, phi-agent tools → LangChain tools.

## 🔒 Security

**phi-agent does NOT sandbox the LLM or sanitize tool calls.** The agent executes whatever tools you register, with whatever permissions those tools have. You are responsible for:

- **Tool permissions** — if you register a shell tool, the LLM can run arbitrary commands. Consider allowlists, sandboxing, or OS-level restrictions.
- **Prompt injection** — user input goes directly into the prompt. There is no input filtering or sanitization built in.
- **Network access** — the LLM client makes outbound HTTP calls to the configured API endpoint. No traffic inspection is performed.
- **Session data** — session logs are written to `~/.phi-agent/sessions/` in plain JSONL. They may contain sensitive information from your conversations.

**For production use**, apply the principle of least privilege: only register tools the agent actually needs, and run the agent process with the minimum necessary OS permissions.

Report security vulnerabilities to **[phiagent@hibuka.com](mailto:phiagent@hibuka.com)**. See [SECURITY.md](SECURITY.md) for our full policy.

## FAQ

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

## Contributors

Thanks goes to these wonderful people:

<!-- ALL-CONTRIBUTORS-LIST:START - Do not remove or modify this section -->
<!-- prettier-ignore-start -->
<!-- markdownlint-disable -->
<table>
  <tr>
    <td align="center"><a href="https://github.com/shard872"><img src="https://github.com/shard872.png" width="100px;" alt=""/><br /><sub><b>shard872</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/7" title="Code">💻</a></td>
    <td align="center"><a href="https://github.com/Krshs90"><img src="https://github.com/Krshs90.png" width="100px;" alt=""/><br /><sub><b>Krish Shah</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/8" title="Code">💻</a></td>
    <td align="center"><a href="https://github.com/slegarraga"><img src="https://github.com/slegarraga.png" width="100px;" alt=""/><br /><sub><b>Sebastian Legarraga</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/9" title="Code">💻</a></td>
    <td align="center"><a href="https://github.com/MsfPablo"><img src="https://github.com/MsfPablo.png" width="100px;" alt=""/><br /><sub><b>Pablo Garcia</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/11" title="Code">💻</a></td>
  </tr>
</table>
<!-- ALL-CONTRIBUTORS-LIST:END -->

([emoji key](https://allcontributors.org/docs/en/emoji-key)) — This project follows the [all-contributors](https://github.com/all-contributors/all-contributors) specification.

## License

MIT — see [LICENSE](LICENSE) for details.

## Contact

:material-email-outline: [phiagent@hibuka.com](mailto:phiagent@hibuka.com)

[中文文档](README_CN.md)

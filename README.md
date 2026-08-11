# <picture><source media="(prefers-color-scheme: dark)" srcset="assets/logo.svg"><img alt="phi-agent" src="assets/logo.svg" height="60"></picture>

[![CI](https://github.com/hibuka-labs/phi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![codecov](https://codecov.io/gh/hibuka-labs/phi-agent/branch/master/graph/badge.svg)](https://codecov.io/gh/hibuka-labs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-book-green.svg)](https://docs.phiagent.dev)
[![PyPI](https://img.shields.io/pypi/v/phi-agent.svg)](https://pypi.org/project/phi-agent/)

Not another AI Agent, but an open application framework for building Agents — purpose-built for embedded, edge, and vertical industries, equally suited for highly customizable, high-performance cloud and desktop AI applications. Simple, pure, predictable.

> **phi-agent ships with zero application tools.** No web search, no database connector, no code executor — just a clean Rust runtime. What tools your agent needs, and how it interacts with the world, is entirely up to you.
>
> Kernel primitives like file I/O, shell execution, and sub-agent spawning are provided via `phi-kernel-tools` as opt-in infrastructure — the OS-level syscalls an agent needs to sense and act. Every kernel tool is gated behind a feature flag; nothing you don't ask for is included.

Built on [agent-base](https://crates.io/crates/agent-base) and [agent-works](https://crates.io/crates/agent-works). **phi-agent provides the infrastructure. You bring the tools.**

## Ecosystem

phi-agent is part of a family of independent crates:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| `agent-base` | [![Crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base) | Lightweight runtime kernel — LLM clients, Tool trait, event stream |
| `agent-works` | [![Crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works) | Batteries-included toolbox — MCP, Skills, Focus |
| `phi-agent` | [![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent) | Full framework — builder factory, renderers, config, CLI binary |

**Just need the runtime?** `cargo add agent-base`. **Need the full framework?** `cargo add phi-agent` (telemetry + logging only by default; enable `file`, `shell`, etc. for kernel tools).

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
    AB --> PKT[phi-kernel-tools<br/>Kernel tools]
    AB --> YT[your-tools<br/>Custom Tool impls]

    AW --> PA[phi-agent<br/>Builder factory<br/>Renderers · Config · Session<br/>CLI binary]

    PKT --> PA
    YT --> PA

    PA --> Terminal[Terminal REPL]
    PA --> JSON[JSON Stream]
    PA --> Web[Web Backend]
```

**Core principle**: phi-agent ships with **zero** application tools. You define them, you register them. phi-agent discovers and manages them at runtime — listing, logging, and routing tool calls automatically.

### Kernel Tools

phi-agent bundles **kernel tools** via `phi-kernel-tools` — low-level primitives that give the agent basic environmental awareness. Think of them as an OS kernel's syscalls, not application-level tools. All are opt-in via feature flags:

| Feature | Capability | Default |
|---------|-----------|---------|
| `file` | Read / write / list files | off |
| `shell` | Execute shell commands | off |
| `multi-agent` | Spawn sub-agents | off |

These are **not** tools in the LangChain sense — no web search, no database connector, no pre-built agent templates. Kernel tools are infrastructure. **Application tools are still 100% your responsibility.**

## Why phi-agent

**Your Domain, Your Rules.** Not a generic chatbot. An open framework where you call the shots — no presets, no lock-in, no decisions made for you. You define the scenario. You write the rules.

**Lightweight, Runs Anywhere.** A single Rust binary with zero runtime dependencies — from embedded Linux and edge gateways to cloud containers and desktop applications, `cargo install` gets you started in seconds, deploy anywhere.

**Zero Application Tools, Fully Customizable.** Kernel primitives (file I/O, shell, sub-agents) are opt-in infrastructure — no pre-packaged application tools, no platform lock-in. A tool is just 3 methods: `name()`, `definition()`, `call()`. You register what you need, the Agent uses what you register. Only bring what your scenario truly needs — LLM freedom, precise and clean.

**Fully Observable, Every Step Explainable.** Every decision is logged, every step is traceable, with built-in session logging, structured tracing, and session metrics at a glance — compliance and audit trails without the stress.

## Features

- **Builder factory** — `base_agent_builder()` with sensible defaults (thinking, recovery, limits)
- **Three renderers** — Terminal (rich, colored, streaming), JSON stream (JSONL), Null (silent)
- **CLI-ready** — REPL and one-shot modes with 20+ configurable flags
- **Session management** — auto-cleanup, file locking, JSONL turn logging
- **Tool-agnostic** — no built-in tools; register your own via `AgentBuilder`
- **Extensible** — middleware, approval handlers, custom renderers

## Quick Start

> **Note:** By default, phi-agent includes only telemetry + logging. Enable kernel tools via features:
> ```bash
> cargo add phi-agent --features file,shell
> ```

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
        "gpt-4o".into(),
        Some("https://api.openai.com/v1".into()),
    ));

    // 2. Build agent (register your tools here)
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt())
        .register_tool(your_tool);

    let agent = PhiAgent::build(builder, PhiAgentConfig {
        model: "gpt-4o".into(),
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

See [examples/](examples/) for runnable examples. No-API-key examples are great for quick exploration:

| Example | Description | API Key |
|---------|-------------|:-------:|
| [`custom_policy`](examples/tools/custom_policy.rs) | ToolPolicy + Middleware + event hooks with mock LLM | ❌ |
| [`session_persist`](examples/session/session_persist.rs) | Session creation, resume, locking, cleanup | ❌ |
| [`event_log`](examples/observability/event_log.rs) | Per-turn JSONL event persistence | ❌ |
| [`hello_agent`](examples/minimal/hello_agent.rs) | Minimal agent setup with LLM | ✅ |
| [`custom_tool`](examples/tools/custom_tool.rs) | Implement a custom Tool | ✅ |
| [`multi_tool`](examples/tools/multi_tool.rs) | Register multiple tools | ✅ |
| [`file_ops`](examples/tools/file_ops.rs) | File system operations | ✅ |
| [`focus_demo`](examples/advanced/focus_demo.rs) | Focus feature | ✅ |
| [`multi_agent`](examples/multi-agent/multi_agent.rs) | Sub-agent spawning and orchestration | ✅ |
| [`middleware_hooks`](examples/observability/middleware_hooks.rs) | Middleware lifecycle hooks | ✅ |
| [`html_renderer`](examples/observability/html_renderer.rs) | HTML event stream renderer | ✅ |
| [`window_memory`](examples/advanced/window_memory.rs) | Sliding window memory | ✅ |
| [`summary_memory`](examples/advanced/summary_memory.rs) | Summary-based memory | ✅ |
| [`mcp_client`](examples/mcp/mcp_client.rs) | MCP client connection | ✅ |
| [`mcp_server`](examples/mcp/mcp_server.rs) | MCP server bridge | ✅ |
| [`mcp_dynamic_attach`](examples/mcp/mcp_dynamic_attach.rs) | Dynamic MCP attach/detach | ✅ |
| [`hybrid_langgraph`](examples/advanced/hybrid_langgraph.rs) | Hybrid LangGraph integration | ✅ |

Run with `cargo run --example <name>`.

## CLI

```bash
# Install with required features for the CLI binary
cargo install phi-agent --features shell,mcp,telemetry,logging
phi "What's in this directory?"
```

```bash
# REPL mode
phi

# JSON output for scripting
phi --format json "list files"
```

### Building from Source

```bash
git clone https://github.com/hibuka-labs/phi-agent.git
cd phi-agent

# The phi binary requires shell, mcp, telemetry, logging features
cargo run --features full

# Or enable specific features:
cargo run --features shell,mcp,telemetry,logging
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

- **Pre-packaged application tools** — no web search, no database connector, no code executor template, no platform-specific integrations. phi-agent provides **kernel tools** (file I/O, shell, sub-agents) as opt-in infrastructure — all gated behind feature flags. You define all application-level tools yourself.
- **Vector database / embeddings** — phi-agent includes file-based memory (`.phi/memory/`, prompt-injection mode) for persistence across turns, but there is no Pinecone/Chroma/Weaviate integration, no automatic embedding, no semantic search. For RAG or long-term semantic memory, bring your own vector DB.
- **Pre-built agent types** — no "research agent," "coding agent," "support agent" templates. You compose your own.
- **Workflow engine** — no DAG execution, no conditional branching engine, no LangGraph-style graph compiler. Agent behavior is driven by LLM tool-choice.
- **Prompt templates** — no langchain-style prompt chains, no automatic context stuffing. You control the system prompt.
- **Streaming HTTP server** — phi-agent is a library and CLI. You build the server layer (Actix/Axum/Warp) yourself.
- **Multi-agent orchestration** — available behind the `multi-agent` feature gate (opt-in). The framework itself does not prescribe multi-agent topologies; sub-agent spawning is provided as a kernel primitive.

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

- **Tool permissions** — if you register a shell tool, the LLM can run arbitrary commands. phi-agent provides an **approval handler** mechanism: tools declare their risk level (Safe / Sensitive / Destructive), and you can intercept every tool call before execution — auto-approve in CI, prompt interactively in a terminal, or implement custom review logic via the `ApprovalHandler` trait. Consider additional sandboxing or OS-level restrictions for production.
- **Prompt injection** — user input goes directly into the prompt. There is no input filtering or sanitization built in.
- **Network access** — the LLM client makes outbound HTTP calls to the configured API endpoint. No traffic inspection is performed.
- **Session data** — session logs are written to `~/.phi-agent/sessions/` in plain JSONL. They may contain sensitive information from your conversations.

**For production use**, apply the principle of least privilege: only register tools the agent actually needs, and run the agent process with the minimum necessary OS permissions.

Report security vulnerabilities to **[phiagent@hibuka.com](mailto:phiagent@hibuka.com)**. See [SECURITY.md](SECURITY.md) for our full policy.

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

📖 **[docs.phiagent.dev](https://docs.phiagent.dev)** — full documentation site with search, navigation, and bilingual (EN/CN) support.

| Resource | Description |
|----------|-------------|
| [Getting Started](https://docs.phiagent.dev/guide/getting-started/) | 5-minute quick start |
| [Configuration](https://docs.phiagent.dev/guide/getting-started/configuration/) | Config reference |
| [Custom Tools](https://docs.phiagent.dev/guide/tools/custom-tool/) | How to write a Tool |
| [Kernel Tools](https://docs.phiagent.dev/guide/tools/file-tools/) | read_file / write_file / list_files / shell / multi-agent |
| [Multi-Agent](https://docs.phiagent.dev/guide/advanced/multi-agent/) | Sub-agent spawning and orchestration |
| [MCP](https://docs.phiagent.dev/guide/advanced/mcp/) | Client + Server (Model Context Protocol) |
| [Skills](https://docs.phiagent.dev/guide/concepts/skills/) | Reusable agent behaviors (agentskills.io) |
| [Memory](https://docs.phiagent.dev/guide/concepts/memory/) | File-based persistence across turns |
| [Session & Snapshots](https://docs.phiagent.dev/guide/advanced/session/) | Session lifecycle, snapshots, REPL commands |
| [Focus](https://docs.phiagent.dev/guide/concepts/focus/) | Structured single-purpose LLM calls |
| [Architecture](https://docs.phiagent.dev/guide/concepts/architecture/) | Design decisions and internals |
| [CLI Usage](https://docs.phiagent.dev/guide/cli/cli/) | CLI flags, REPL, one-shot |
| [phi serve](https://docs.phiagent.dev/guide/cli/phi-serve/) | MCP Server / Bridge protocol |
| [Observability](https://docs.phiagent.dev/guide/advanced/observability/) | Logging, tracing, metrics |
| [Advanced](https://docs.phiagent.dev/guide/advanced/advanced/) | Middleware, policies |
| [API Reference](https://docs.rs/phi-agent) | Rustdoc on docs.rs |

Source files are also available in [guide/](guide/) for offline reading.

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

# phi-agent

[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A general-purpose AI Agent framework built on [agent-base](https://crates.io/crates/agent-base).

Provides builder factory, renderer, config resolution, and session management infrastructure. **Does not bundle any tools** — tools are injected by consumers.

## Features

- **Builder factory** — `base_agent_builder()` with sensible defaults
- **Multiple renderers** — Terminal (rich), JSON stream, Null
- **CLI-ready** — REPL and one-shot modes out of the box
- **Session management** — auto-cleanup, file locking, turn logging
- **Tool-agnostic** — no built-in tools; register your own via `AgentBuilder`

## Quick Start

```rust
use phi_agent::{
    base_agent_builder, build_system_prompt, PhiAgent, PhiAgentConfig,
    OpenAiClient, SafetyConfig,
};
use std::sync::Arc;

// 1. Create LLM client
let llm_client = Arc::new(OpenAiClient::new(
    api_key, model, Some(base_url),
));

// 2. Build agent (register your tools here)
let builder = base_agent_builder(llm_client)
    .system_prompt(build_system_prompt())
    .register_tool(your_tool);

let agent = PhiAgent::build(builder, PhiAgentConfig {
    model: model.into(),
    enable_thinking: true,
    thinking_budget: None,
    thinking_effort: ReasoningEffort::Medium,
    safety: SafetyConfig::default(),
})?;

// 3. Run
let session = agent.create_session().await;
agent.run_turn(session, "Hello!", |event| {
    renderer.render(event)
}).await?;
```

## CLI

```bash
cargo install phi-agent
phi "What's in this directory?"
```

## License

MIT

[中文文档](README_CN.md)

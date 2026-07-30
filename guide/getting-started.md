# Getting Started

5 minutes to your first phi-agent.

## Prerequisites

- [Rust](https://rustup.rs) (stable, edition 2024)
- An LLM API key (OpenAI-compatible endpoint)

## 1. Install phi-agent

```bash
cargo install phi-agent
```

## 2. Create a project

```bash
phi init my-agent
cd my-agent
```

## 3. Set your API key

```bash
cp .env.example .env
# Edit .env with your actual API key
```

`.env.example` contains common configurations for OpenAI, Anthropic, DeepSeek, and other providers. See [Configuration](configuration.md) for all options.

## 4. Run

`phi init` already generated a working `src/main.rs`. Just run it:

```bash
cargo run
```

## What's Next

- [Custom Tools](custom-tool.md) — add your own tools to the agent
- [Focus](focus.md) — structured single-purpose LLM calls
- [Configuration](configuration.md) — understand all config options
- [Advanced](advanced.md) — middleware, sessions, event log

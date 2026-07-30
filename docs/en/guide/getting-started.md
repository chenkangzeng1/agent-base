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

Open `src/main.rs` — you'll see the full `ClockTool` implementation. Write your own tool the same way, register it with the agent, done.

See [Custom Tools](custom-tool.md) for details.

## Option 2: Library integration

Add phi-agent as a library to an existing project:

```bash
cargo add phi-agent
cargo add tokio --features full
cargo add anyhow
cargo add dotenvy
cargo add async-trait
cargo add serde_json
cargo add chrono
```

Then copy the `ClockTool` example into your `main.rs`.
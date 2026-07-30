# Getting Started

5 minutes to your first phi-agent.

## Prerequisites

- [Rust](https://rustup.rs) (stable, edition 2024)
- An LLM API key (OpenAI-compatible endpoint)

## Install

```bash
cargo install phi-agent
```

## Option 1: CLI (recommended)

Start an interactive REPL:

```bash
phi
```

```
phi> What time is it?
🔧 get_time
2025-07-30 19:30:00

phi> /exit
```

Built-in shell tools, metrics, and thinking mode. Great for most users.

## Option 2: Code integration

Scaffold a project to build your own Agent:

```bash
phi init my-agent
cd my-agent
```

Set your API key and run:

```bash
cp .env.example .env
# Edit .env with your API key
cargo run
```

`phi init` generates a REPL with a `ClockTool` example. Open `src/main.rs`, model your own tool after `ClockTool`, register it — done.

See [Configuration](configuration.md) for all config options and [Custom Tools](custom-tool.md) for more examples.
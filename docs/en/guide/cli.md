# CLI Usage

The `phi` binary supports REPL (interactive) and one-shot modes. It uses `OpenAiClient` — any OpenAI-compatible API works.

## Quick Start

```bash
# Build
cargo build --release

# One-shot
cargo run -- "What's 2+2?"

# REPL (interactive)
cargo run
```

## Configuration

Create a `.env` file or set environment variables:

```bash
LLM_API_KEY=sk-your-key-here
LLM_MODEL=gpt-4o
# LLM_BASE_URL defaults to https://api.openai.com/v1
```

See [Configuration](configuration.md) for all options.

## One-Shot Mode

```bash
phi "Explain this codebase"
phi "List all Rust files" --model opus
phi "Check the architecture" --json | jq '.type'
phi "Run silently" --quiet
```

## REPL Mode

```bash
phi
# Type queries, press Enter
# Ctrl+C to cancel current turn
# Ctrl+D to exit
```

## CLI Flags

| Flag | Description |
|------|-------------|
| `QUERY` | Positional — one-shot query (absent = REPL) |
| `-m, --model` | Override model name |
| `--base-url` | Override API base URL |
| `-s, --session` | Session ID (resume previous session) |
| `--no-thinking` | Disable extended thinking |
| `--json` | JSON stream output (pipe-friendly) |
| `--quiet` | No output |

## Session Persistence

Sessions auto-save to `~/.phi-agent/sessions/<id>/`. Resume with `--session`:

```bash
phi --session 20250728_a1b2c3d4
```

Inactive sessions (> 7 days) are auto-cleaned.

# phi-agent Design Document

## Overview

phi-agent is a general-purpose AI Agent framework built on [agent-base](../../agent-base) and [agent-works](../../agent-works).

**Core principle: phi-agent itself does not bundle any tools.** It only provides the agent builder factory, renderer, config resolution, and other infrastructure. Tools are implemented and injected by consumers (CLI, web backend, etc.).

```
agent-base          ← Pure runtime: AgentRuntime / Tool trait / RuntimeEvent
agent-works         ← Toolbox: Focus / Skills / Builtin tools / MCP
    ↑
phi-agent (lib)   ← Agent framework: wraps agent-base + agent-works + renderer + config
    ↑                 ← **Does not bundle any tools**
    ↑
forge-cli (bin)     ← CLI consumer — registers tools here (e.g. LocalShellTool)
web-server (future) ← Web consumer — registers tools here (may differ from CLI)
```

### Design Principles

- **Tools decoupled from framework**: phi-agent doesn't know what tools exist — it only knows the `Tool` trait. Tools are assembled by consumers.
- **Lib is the product**: All functionality is implemented and tested in the lib. CLI / Web are just different consumers.
- **Event-driven**: The Agent produces a `RuntimeEvent` stream; renderers format output. The two are fully decoupled.
- **Reuse agent-base**: LLM calls, tool orchestration, event streams, middleware — all reused, no wheel reinvention.

---

## Architecture Overview

```
┌────────────────────────────────────────────────────┐
│                    agent-base                       │
│  AgentRuntime / Tool trait / RuntimeEvent           │
│  Middleware / ApprovalHandler / AgentBuilder         │
└────────────────────┬───────────────────────────────┘
                     │
┌────────────────────▼───────────────────────────────┐
│                   agent-works                        │
│  Focus / Skills / Builtin tools / MCP / AgentHandle │
└────────────────────┬───────────────────────────────┘
                     │
┌────────────────────▼───────────────────────────────┐
│               phi-agent (lib)                      │
│                                                     │
│  Contains no tools. Wraps agent-base + agent-works: │
│  ├── agent/builder.rs   base_agent_builder() factory│
│  ├── agent/factory.rs   PhiAgent struct             │
│  ├── render/            EventRenderer trait + impls  │
│  ├── config/            Configuration resolution    │
│  ├── cli/               CLI helpers (approval, etc.)│
│  └── prompt.rs          System prompt builder       │
│                                                     │
├────────────────────────────────────────────────────┤
│               forge-cli (bin)                        │
│                                                     │
│  Register tools here:                                │
│  ├── LocalShellTool  ← implements Tool trait        │
│  └── ...               ← future extensions          │
│                                                     │
├────────────────────────────────────────────────────┤
│               web-server (future)                    │
│                                                     │
│  Register tools here:                                │
│  ├── LocalShellTool  ← reuse the same one           │
│  ├── HttpTool        ← web-specific                 │
│  └── ...                                            │
└────────────────────────────────────────────────────┘
```

**Key distinction**: `tools/` is not in the lib — it's in the bin (or a separate tool crate). Consumers decide what tools the agent gets.

---

## Project Structure

```
phi-agent/
├── Cargo.toml
├── .env.example
├── src/
│   ├── lib.rs              # Core library entry point (no tools)
│   ├── agent/
│   │   ├── mod.rs
│   │   ├── builder.rs      # base_agent_builder() generic factory
│   │   └── factory.rs      # PhiAgent struct
│   ├── render/
│   │   ├── mod.rs          # EventRenderer trait + OutputFormat
│   │   ├── terminal.rs     # Rich terminal renderer
│   │   └── json_stream.rs  # JSON stream renderer (one JSON per line)
│   ├── cli/
│   │   ├── mod.rs
│   │   └── approval.rs     # CliApprovalHandler (Auto / Interactive)
│   ├── config/
│   │   ├── mod.rs
│   │   └── llm.rs          # LLM config resolution
│   └── prompt.rs           # System prompt builder
├── src/bin/
│   └── forge/
│       ├── main.rs         # CLI entry point (register tools here)
│       ├── args.rs         # clap argument definitions
│       ├── approval.rs     # CLI approval handler
│       └── tools/          # Tool implementations (maintained by consumer)
│           ├── mod.rs
│           └── (re-exports from phi-tools)
└── tests/
    └── lib_tests.rs        # Integration tests
```

### Module Responsibilities

| Module | Responsibility | Depends On |
|--------|---------------|------------|
| `lib.rs` | Unified agent framework API export (re-exports agent-base + agent-works) | agent-base, agent-works |
| `agent/builder.rs` | `base_agent_builder()` with sensible defaults | agent-base |
| `agent/factory.rs` | `PhiAgent` — holds `AgentRuntime` + `PhiAgentConfig` | agent-base, agent-works |
| `render/` | `EventRenderer` trait + Terminal/JSON implementations | agent-base |
| `cli/approval.rs` | Approval handlers | agent-base |
| `config/` | Env var / arg resolution | dotenvy |
| `prompt.rs` | System prompt builder | — |
| `bin/forge/main.rs` | CLI entry point: assemble tools → inject into agent → run | lib |
| `bin/forge/tools/` | Tool re-exports (belongs to CLI consumer) | phi-tools |

---

## Core Component Design

### 1. PhiAgent — Does Not Hold Tools

```rust
/// phi-agent configuration (tool-agnostic)
pub struct PhiAgentConfig {
    pub model: String,
    pub enable_thinking: bool,
    pub thinking_budget: Option<u64>,
    pub thinking_effort: ReasoningEffort,
    pub safety: SafetyConfig,
}

/// A built Agent instance
pub struct PhiAgent {
    pub runtime: AgentRuntime,
    pub config: PhiAgentConfig,
}
```

**Key point**: `PhiAgentConfig` has no tool-related fields. `PhiAgent::build()` does not register any tools.

Tools are prepared externally by consumers and injected through agent-base's `AgentBuilder`:

```rust
impl PhiAgent {
    /// Takes an externally-prepared AgentBuilder and finalizes it.
    /// Tool registration, approval strategy, and middleware are all done
    /// by the caller on the builder beforehand.
    pub fn build(builder: AgentBuilder, config: PhiAgentConfig) -> Result<Self>;
}
```

Typical consumer (CLI) usage:

```rust
// src/bin/forge/main.rs
let llm_client = Arc::new(OpenAiClient::new(api_key, model, Some(base_url)));

let builder = base_agent_builder(llm_client)
    .system_prompt(system_prompt)
    .register_tool(LocalShellTool::new(30_000))   // ← CLI registers tools here
    .register_tool(another_tool)                   // ← add more tools here
    .approval_handler(approval);

let agent = PhiAgent::build(builder, config)?;
```

### 2. Tools Are Implemented by Consumers

Tools implement the `agent_base::Tool` trait and reside in consumer code:

```rust
use agent_base::{Tool, ToolContext, ToolOutput, ToolControlFlow, AgentResult};

pub struct LocalShellTool {
    timeout_ms: u64,
}

impl Tool for LocalShellTool {
    fn name(&self) -> &'static str { "execute_command" }

    fn definition(&self) -> Value {
        json!({
            "name": "execute_command",
            "description": "Execute a shell command locally",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory (current dir if omitted)"
                    }
                },
                "required": ["command"]
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        // tokio::process::Command + timeout + cancel_token
        // ...
    }
}
```

### 3. EventRenderer

```rust
pub trait EventRenderer: Send {
    fn render(&mut self, event: RuntimeEvent) -> AgentResult<()>;
    fn finish_turn(&mut self) -> AgentResult<()>;
    fn finish_session(&mut self) -> AgentResult<()> { Ok(()) }
}

pub enum OutputFormat {
    Terminal { show_thinking: bool, show_tool_args: bool, color: bool },
    Json,
    Quiet,
}
```

Three renderers:

| Renderer | Output | Use Case |
|----------|--------|----------|
| `TerminalRenderer` | Rich text (colors, emoji, formatting) | Human interaction |
| `JsonStreamRenderer` | One JSON event per line | Scripting / automation |
| `NullRenderer` | No output (tracing logs only) | Web backend mode |

Key event rendering:

| RuntimeEvent | TerminalRenderer | JsonStreamRenderer |
|---|---|---|
| `TextDelta` | Stream AI reply | `{"type":"text_delta","text":"..."}` |
| `ThoughtDelta` | Gray thinking content | `{"type":"thought_delta","text":"..."}` |
| `ToolCallStarted` | Tool name + args summary | `{"type":"tool_call_started","tool":"...","args":{}}` |
| `ToolCallFinished` | Result summary | `{"type":"tool_call_finished","tool":"...","summary":"..."}` |
| `TurnFinished` | Duration stats | `{"assistant_text":"...","duration_ms":5000,"type":"turn_finished"}` |

### 4. CLI Entry Point

```rust
// src/bin/forge/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args = CliArgs::parse();

    // 1. Resolve LLM config
    let llm_config = resolve_llm_config(args.model.as_deref(), args.base_url.as_deref())?;

    // 2. Create LLM client
    let llm_client = Arc::new(OpenAiClient::new(
        llm_config.api_key, llm_config.model, Some(llm_config.base_url),
    ));

    // 3. Build system prompt
    let system_prompt = build_system_prompt();

    // 4. Approval handler
    let approval = Arc::new(CliApprovalHandler::new());

    // 5. Assemble builder — register tools here
    let builder = base_agent_builder(llm_client)
        .system_prompt(system_prompt)
        // ─── Register tools here ───
        .register_tool(LocalShellTool::new(30_000))
        // ─── Add more tools here ───
        .approval_handler(approval)
        .middleware(TurnFactMiddleware::new())
        .middleware(TurnToolLimitMiddleware::from_config(&safety));

    // 6. Build and run
    let agent = PhiAgent::build(builder, config)?;

    if let Some(query) = args.query {
        run_one_shot(&agent, &query, format).await
    } else {
        run_repl(&agent, format).await
    }
}
```

CLI arguments:

| Argument | Description | Default |
|----------|-------------|---------|
| `[query]` | One-shot query text | None (enters REPL) |
| `--format` | Output format (terminal/json/quiet) | terminal |
| `--model` | Model name | Env var |
| `--base-url` | API Base URL | Env var |
| `--no-thinking` | Disable thinking | Enabled by default |
| `--session-id` | Specify session ID | Auto-generated |

### 5. Config Resolution

The lib layer only resolves LLM config. Tool-related config (e.g. shell timeout) is handled by the consumer:

```bash
# .env
LLM_API_KEY=sk-xxx
LLM_MODEL=opus
LLM_BASE_URL=https://api.anthropic.com
```

Priority: CLI arg > environment variable > `.env` > default

### 6. Session Management

- Auto-generate session_id (timestamp + random string)
- Logs written to `~/.phi-agent/sessions/<session_id>/`
- Support `--session-id` for manual specification (cross-process reuse)
- Auto-cleanup of expired sessions (7 days)

---

## Consumer Tool Strategies

Different consumers can configure different tool sets:

```rust
// CLI: local operations tools
builder
    .register_tool(LocalShellTool::new(30_000))
    .register_tool(GitTool::new())
    .build()

// Web: may add HTTP/DB tools, reduce direct shell access
builder
    .register_tool(LocalShellTool::new(10_000))   // shorter timeout
    .register_tool(HttpTool::new())
    .register_tool(DbQueryTool::new(pool))
    .build()

// CI: read-only access only
builder
    .register_tool(LocalShellTool::new(60_000).read_only())
    .build()
```

---

## Comparison with ops-agent

| Dimension | ops-agent | phi-agent |
|-----------|-----------|-------------|
| Target scenario | SSH remote ops | General-purpose agent framework |
| Tool location | Built into lib (8 SSH tools) | **No tools in lib — consumer injects** |
| External deps | sshr (SSH library) | None |
| Approval | Per-command risk level | Configurable (consumer-defined) |
| Renderers | Terminal / JSON | Terminal / JSON / Null |
| Middleware | AntiHallucination + 4 others | Minimal (consumer chooses) |

---

## Dependencies

```toml
[dependencies]
agent-base = { path = "../agent-base" }
agent-works = { path = "../agent-works" }
anyhow = "1"
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
dotenvy = "0.15"
fs2 = "0.4"
regex = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4"] }
```

CLI consumer additional dependencies (under `[[bin]]` or feature flag):

```toml
clap = { version = "4", features = ["derive", "env"] }
rustyline = "15"
```

---

## Implementation Plan

### Phase 1: Minimum Viable (MVP)

1. `lib.rs` + Cargo.toml — project skeleton
2. `agent/builder.rs` — `base_agent_builder()` generic factory
3. `agent/factory.rs` — `PhiAgent` struct
4. `render/` — `EventRenderer` trait + Terminal/Json/Null implementations
5. `cli/approval.rs` — Auto-approval handler
6. `config/llm.rs` — LLM config resolution
7. `prompt.rs` — Basic system prompt
8. `bin/forge/` — CLI entry point (assemble tools → inject → one-shot)
9. `bin/forge/args.rs` — clap arguments

### Phase 2: Polish

1. REPL interactive mode
2. Session management + logging
3. JSON stream output validation
4. Tests

### Phase 3: Future Extensions

1. WebSocket renderer (web version)
2. Knowledge base support
3. More consumers (CI mode, API mode)
4. More tools (Git, HTTP, DB, etc. — added to consumers as needed)

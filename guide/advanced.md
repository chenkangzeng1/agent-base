# Advanced Usage

Middleware, sessions, event logging, and more.

## Middleware

Middleware hooks into the agent loop before and after LLM calls:

```rust
use agent_base::{TurnFactMiddleware, TurnToolLimitMiddleware};

let builder = base_agent_builder(llm_client)
    .system_prompt(system_prompt)
    .middleware(TurnFactMiddleware::new())
    .middleware(TurnToolLimitMiddleware::from_config(&safety));
```

Built-in middleware:
- `TurnFactMiddleware` — injects facts/context at the start of each turn
- `TurnToolLimitMiddleware` — enforces `max_tool_calls_per_turn`

## Approval Handlers

Control which tool calls require human confirmation:

```rust
// Auto-approve everything (CI / automation)
use phi_agent::{AutoApprovalHandler, ApprovalMode};
builder = builder.approval_handler(Arc::new(
    AutoApprovalHandler::new(ApprovalMode::Auto)
));

// Deny all (read-only / preview mode)
builder = builder.approval_handler(Arc::new(
    AutoApprovalHandler::new(ApprovalMode::DenyAll)
));
```

For interactive CLI approval, see `CliApprovalHandler` in the forge binary.

## Session Management

Sessions persist conversation history and tool call results:

```rust
use phi_agent::session::{resolve_session, cleanup_expired_sessions};

// Create or reuse a session
let ctx = resolve_session(Some("my-session"), &base_dir)?;
println!("Session: {} (new: {})", ctx.session_id, ctx.is_new_session);

// Clean up old sessions (> 7 days)
let cleaned = cleanup_expired_sessions(&base_dir, 7)?;
println!("Cleaned {} expired sessions", cleaned);
```

Session directory layout:
```
~/.phi-agent/sessions/<id>/
├── session_id           # Session ID marker
├── session.lock         # Exclusive file lock
├── session_meta.json    # Created at, last active at
└── turn_001.jsonl       # Per-turn event log (JSONL)
```

## Event Logging

Every turn is persisted as JSONL for replay and analysis:

```rust
use phi_agent::{save_turn_log, event_to_jsonl};

// Save turn events
save_turn_log(&session_ctx, 1, &events, "user query")?;

// Convert a single event to JSONL
let line = event_to_jsonl(&event);
```

Event types in the log:
- `thought_delta` — LLM thinking content
- `text_delta` — Assistant text output
- `tool_call_started` / `tool_call_finished` — Tool invocations
- `approval_request` — When a tool needs approval
- `plan_updated` — Task plan changes
- `turn_finished` — Turn summary with duration and stats

## System Prompts

phi-agent provides two system prompt variants:

```rust
use phi_agent::{build_system_prompt, build_system_prompt_cn};

// Default (international)
let prompt = build_system_prompt();

// China-aware variant (prefers domestic services, handles GFW)
let prompt_cn = build_system_prompt_cn();
```

You can also pass a fully custom prompt via `builder.system_prompt(...)`.

## Reasoning / Thinking

Control the LLM's chain-of-thought behavior:

```rust
use agent_base::{ReasoningConfig, ReasoningEffort};

// Builder-level default
builder = builder.reasoning(ReasoningConfig {
    effort: Some(ReasoningEffort::High),
    ..Default::default()
});

// Per-turn override
agent.set_reasoning_effort(ReasoningEffort::XHigh).await;
```

Effort levels and when to use them:
- `Low` — simple tasks, fast responses
- `Medium` — default, balanced
- `High` — complex multi-step tasks
- `XHigh` — hardest problems, longest think time

## Programmatic Renderers

Use renderers outside the CLI:

```rust
use phi_agent::{
    TerminalRenderer, JsonStreamRenderer, NullRenderer, EventRenderer,
};
use std::io;

// Terminal
let mut renderer = TerminalRenderer::new(true, true, true, Box::new(io::stdout()));

// JSON stream (for IDE integration)
let mut renderer = JsonStreamRenderer::stdout();

// Silent (for web backends)
let mut renderer = NullRenderer;
```

## Error Recovery

phi-agent configures consecutive failure recovery by default:

```rust
use agent_base::ConsecutiveFailureRecovery;

// 3 consecutive failures → stop and explain
builder = builder.error_recovery(Arc::new(
    ConsecutiveFailureRecovery::new(3)
));
```

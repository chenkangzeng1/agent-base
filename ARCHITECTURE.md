# Architecture

`agent-base` is a lightweight Agent Runtime Kernel. This document explains its core abstractions, the runtime loop lifecycle, and how the pieces fit together.

## Core Abstractions

```
┌──────────────────────────────────────────────────┐
│                  AgentRuntime                      │
│  ┌─────────────┐  ┌──────────┐  ┌──────────────┐ │
│  │ LlmClient    │  │ Tool     │  │ Approval      │ │
│  │ (trait)      │  │ Registry │  │ Handler      │ │
│  │              │  │          │  │ (trait)      │ │
│  │ OpenAI       │  │ Tool A   │  │ AllowOnce    │ │
│  │ Anthropic    │  │ Tool B   │  │ Always/Deny  │ │
│  └─────────────┘  └──────────┘  └──────────────┘ │
│  ┌─────────────┐  ┌──────────┐  ┌──────────────┐ │
│  │ Middleware   │  │ Session  │  │ Event Bus    │ │
│  │ (trait)      │  │ Store    │  │ (broadcast)  │ │
│  │              │  │ (trait)  │  │              │ │
│  │ 3 hooks     │  │ InMemory │  │ AgentEvent   │ │
│  └─────────────┘  └──────────┘  └──────────────┘ │
│  ┌─────────────┐  ┌──────────┐                   │
│  │ ToolError   │  │ Context  │                   │
│  │ Recovery    │  │ Window   │                   │
│  │ (trait)     │  │ Manager  │                   │
│  └─────────────┘  └──────────┘                   │
└──────────────────────────────────────────────────┘
```

### LlmClient (trait)

Abstracts LLM provider interaction. Every provider implements:

- `chat()` — non-streaming completion
- `chat_stream()` — streaming completion returning `StreamChunk` events
- `capabilities()` — declares supported features (streaming, tools, vision, thinking)

**Built-in implementations:** `OpenAiClient`, `AnthropicClient`.

### Tool (trait)

Represents a capability the LLM can invoke. Each tool provides:

- `name()` — unique identifier
- `definition()` — JSON Schema exposed to the LLM
- `call()` — async execution

The `ToolRegistry` manages registration and lookup by name.

### AgentEvent (enum)

All runtime output is structured events:

| Event | Meaning |
|---|---|
| `TextDelta` | Incremental text from the LLM |
| `ThoughtDelta` | Reasoning/thinking tokens |
| `ToolCallStarted` | A tool is about to execute |
| `ToolCallFinished` | A tool has completed |
| `AwaitingApproval` | Blocked on user approval |
| `Checkpoint` | Execution snapshot for replay |
| `RunFinished` | The turn has ended |

Events are delivered through two paths:
1. **`tokio::sync::broadcast<AgentEvent>`** — for async consumers (UI, logging)
2. **`on_event` callback** — for synchronous processing in the run loop

### Approval Flow

```
ToolPolicy.evaluate_approval()
    │
    ├── Returns None → tool executes immediately
    │
    └── Returns Some(ApprovalRequest)
            │
            ├── Approved (AllowOnce/AllowAlways) → tool executes
            │
            └── Denied → tool skipped, result sent to LLM
```

- **`ToolPolicy`** — decides *if* a tool needs approval (stateless, sync)
- **`ApprovalHandler`** — executes the approval interaction (stateful, async)

### Extension Points

| Point | What you can do |
|---|---|
| **Middleware** | Modify user input before LLM, modify messages/tools before LLM, modify LLM output before processing |
| **ToolErrorRecovery** | Decide what happens when a tool fails: stop or retry |
| **LlmClient** | Add a new LLM provider |
| **SessionStore** | Persist sessions to database, filesystem, etc. |

## Runtime Loop

```
User Input
    │
    ▼
[Mid] on_user_message ─── modify input
    │
    ▼
[Session] Save user message
    │
    ▼
[Checkpoint] AfterUserInput
    │
    ┌─────────────────────────────────────────┐
    │  Turn Loop (max 50 turns by default)     │
    │                                         │
    │  [Mid] on_pre_llm ─── modify messages    │
    │      │                                   │
    │      ▼                                   │
    │  [Checkpoint] BeforeLlm                  │
    │      │                                   │
    │      ▼                                   │
    │  execute_llm_turn ─── stream + aggregate  │
    │      │                                   │
    │      ▼                                   │
    │  [Mid] on_post_llm ─── modify output     │
    │      │                                   │
    │      ├── Text response → save + break    │
    │      │                                   │
    │      └── ToolCall                        │
    │              │                           │
    │              ▼                           │
    │      [Checkpoint] BeforeToolCalls        │
    │              │                           │
    │              ▼                           │
    │      Approval: policy + handler          │
    │              │                           │
    │              ▼                           │
    │      handle_tool_calls ─── parallel exec │
    │              │                           │
    │              ├── All Break → RunFinished │
    │              ├── Any Continue → loop     │
    │              └── Error → error_recovery  │
    │                        ├── Stop → fail   │
    │                        └── Retry → loop  │
    └─────────────────────────────────────────┘
    │
    ▼
[Session] Save to store
    │
    ▼
Return Ok(RunOutcome)
```

## Module Map

```
src/
├── lib.rs                  ─── Public API exports
├── engine/                 ─── Runtime kernel
│   ├── runtime/mod.rs      ─── AgentRuntime, run_turn loop
│   ├── runtime/llm.rs      ─── LLM calling + streaming
│   ├── runtime/tool_exec.rs── Tool dispatch + execution
│   ├── runtime/approval_flow.rs ─── Approval orchestration
│   ├── builder.rs          ─── AgentBuilder (fluent API)
│   ├── approval.rs         ─── ApprovalHandler trait + built-in impls
│   ├── middleware.rs       ─── Middleware trait + contexts
│   ├── session.rs          ─── AgentSession (message history)
│   ├── session_store.rs    ─── SessionStore trait + InMemory
│   ├── context.rs          ─── ContextWindowManager (token trimming)
│   ├── recovery.rs         ─── ToolErrorRecovery trait + StopOnError/RetryOnError
│   └── tool_enforcement.rs ─── ToolEnforcementMiddleware (nudge LLM to call tools)
│
├── llm/                    ─── LLM provider abstraction
│   ├── mod.rs              ─── LlmClient trait, StreamChunk, LlmCapabilities
│   ├── openai.rs           ─── OpenAI / compatible API implementation
│   ├── anthropic.rs        ─── Anthropic API implementation
│   └── registry.rs         ─── LlmClientBuilder (from env or config)
│
├── tool/                   ─── Tool system
│   ├── mod.rs              ─── Tool trait, ToolRegistry, ToolOutput
│   ├── policy.rs           ─── ToolPolicy trait (approval evaluation)
│   ├── subagent.rs         ─── SubAgentTool + SubAgentSessionPolicy
│   └── auto_continue.rs    ─── AutoContinueTool
│
└── types/                  ─── Core types
    ├── events.rs           ─── AgentEvent enum
    ├── message.rs          ─── ChatMessage, Message, ToolCallMessage
    ├── session.rs          ─── SessionId
    ├── approval.rs         ─── ApprovalRequest, ApprovalDecision, RiskLevel
    ├── checkpoint.rs       ─── CheckpointData, CheckpointStep
    ├── config.rs           ─── AgentConfig, RetryConfig, ResponseFormat
    ├── error.rs            ─── AgentError enum + AgentResult
    └── outcome.rs          ─── RunOutcome
```

## Key Design Decisions

### Why `RunOutcome` instead of just `Result`?

`RunOutcome` separates "kernel errors" (connection failure, invalid config) from "run results" (task completed, task failed, max turns exceeded). This lets the upper layer handle business outcomes without filtering through error types.

### Why `ToolErrorRecovery` as a trait?

Different agents have fundamentally different recovery preferences. Ops-agents should stop on failure; code-agents should retry. Baking this into the runtime would make it opinionated. The trait keeps the kernel neutral.

### Why memory-first for sessions?

SessionStore is an optional persistence adapter. The runtime always operates on in-memory state. This keeps the kernel simple, testable, and suitable for both CLI and long-running server contexts. Persistence becomes an explicit concern for the upper layer.

### Why approval split into Policy + Handler?

- `ToolPolicy` is stateless and synchronous — cheap to evaluate on every call
- `ApprovalHandler` is stateful and async — can involve network, UI, or database

This split lets you implement approval without modifying tool code, and test policy logic independently of the approval UI.

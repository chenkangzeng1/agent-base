# Architecture

`agent-base` is a lightweight Agent Runtime Kernel written in Rust. It provides a trait-driven, event-based foundation for building autonomous agents.

Among agent runtime libraries, `agent-base` distinguishes itself through:

- **Pipeline composition over monolithic orchestration** — `DefaultPipeline` handles policy hooks, timeout, and truncation as composable layers. `EventEmittingPipeline` decorates without modifying core logic. A `ToolEngine::orchestrate()` method unifies approval + execution in one call, keeping the ReAct loop lean.
- **Ephemeral message lifecycle** — Messages can be marked as ephemeral (`ChatMessage::System { ephemeral: true }`). Ephemeral messages are visible to the LLM during the current turn but are automatically cleaned from the session after the turn ends and excluded from persistence. This enables pattern injection (reminders, plan hints) without polluting conversation history.
- **Extensible middleware** — `Middleware` hooks at `on_user_message`, `on_pre_llm`, and `on_post_llm` allow consumers to inject custom logic (routing, nudge, enforcement) without modifying the react loop.

## Layered Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Public API: AgentBuilder → AgentRuntime                        │
├─────────────────────────────────────────────────────────────────┤
│  Engine Layer                                                   │
│  ├─ LlmEngine        LLM call orchestration                    │
│  ├─ ToolEngine       Tool dispatch + approval + pipeline        │
│  ├─ SessionManager   Session lifecycle + LRU eviction           │
│  └─ EventBus         Internal event broadcast                   │
├─────────────────────────────────────────────────────────────────┤
│  Pipeline Layer                                                 │
│  ├─ DefaultPipeline       Policy hooks + timeout + truncation   │
│  └─ EventEmittingPipeline Event emission decorator              │
├─────────────────────────────────────────────────────────────────┤
│  Ports (traits)                                                 │
│  ├─ LlmClient            LLM provider abstraction               │
│  ├─ Tool / TypedTool     Agent capability abstraction           │
│  ├─ ToolPolicy           Approval evaluation                   │
│  ├─ ApprovalHandler      Approval interaction                  │
│  ├─ Middleware           ReAct loop hooks                       │
│  ├─ ToolErrorRecovery    Tool failure strategy                  │
│  ├─ SessionStore         Session persistence                   │
│  └─ ContextWindowManager Token-aware context trimming           │
├─────────────────────────────────────────────────────────────────┤
│  Adapters                                                       │
│  ├─ OpenAiClient         OpenAI / compatible APIs              │
│  ├─ AnthropicClient      Anthropic API                         │
│  └─ InMemorySessionStore Default in-memory sessions            │
└─────────────────────────────────────────────────────────────────┘
```

## Event System

Events flow from runtime to consumer:

```
RuntimeEvent (public)           Unified event stream — single source of truth
    │
    ├─ Framework events         TextDelta, ToolCallStarted, PlanUpdated, etc.
    └─ UserEvent              Wraps tool-produced user-space events

UserEvent (public)              Events emitted by tools during execution
    ├─ Progress                 Progress updates from long-running tools
    ├─ SubAgentEvent            Events from sub-agent tools
    └─ Structured               Arbitrary structured data
```

### RuntimeEvent Variants

| Category | Events |
|----------|--------|
| **Core** | `TextDelta`, `ThoughtDelta`, `ToolCallStarted`, `ToolCallFinished`, `AwaitingApproval`, `Checkpoint`, `RunFinished`, `RunCancelled` |
| **Plan** | `PlanUpdated` |
| **User** | `UserEvent` (wraps tool-produced events) |

## Extension Points

| Trait | Scope | Default | Purpose |
|-------|-------|---------|---------|
| `LlmClient` | LLM Provider | `OpenAiClient`, `AnthropicClient` | LLM interaction |
| `Tool` / `TypedTool` | Tool System | User-defined | Agent capabilities |
| `ToolPolicy` | Tool Approval | None | Sync approval evaluation + before/after hooks |
| `ApprovalHandler` | Tool Approval | `AllowAll`, `DenyAll` | Async approval interaction |
| `Middleware` | ReAct Loop | None | 3 hooks: `on_user_message`, `on_pre_llm`, `on_post_llm` |
| `ToolErrorRecovery` | ReAct Loop | `StopOnError` | Tool failure: stop or retry |
| `SessionStore` | Session | `InMemorySessionStore` | Session persistence |
| `ContextWindowManager` | Context | None | Token-aware message trimming |

## Execution Mode: ReAct Loop

Single-turn interactive execution. The agent processes user input, calls the LLM, executes tools, and returns when the LLM produces a text response or all tools signal `Break`.

```
runtime.run_turn(session_id, user_input, on_event)
```

## Runtime Loop (Turn Mode)

```
User Input
    │
    ▼
[Middleware] on_user_message ─── modify input
    │
    ▼
[Session] Save user message
    │
    ▼
[Checkpoint] AfterUserInput
    │
    ┌──────────────────────────────────────────────┐
    │  Turn Loop (configurable max_turns)           │
    │                                              │
    │  [Middleware] on_pre_llm ─── modify messages  │
    │      │                                       │
    │      ▼                                       │
    │  [Checkpoint] BeforeLlm                      │
    │      │                                       │
    │      ▼                                       │
    │  LlmEngine.execute ─── stream + aggregate    │
    │      │                                       │
    │      ▼                                       │
    │  [Middleware] on_post_llm ─── modify output   │
    │      │                                       │
    │      ├── Text response → save + break        │
    │      │                                       │
    │      └── ToolCall                            │
    │              │                               │
    │              ▼                               │
    │      [Checkpoint] BeforeToolCalls            │
    │              │                               │
    │              ▼                               │
    │      Approval: policy + handler              │
    │              │                               │
    │              ▼                               │
    │      ToolEngine.execute_tool                 │
    │        ├─ DefaultPipeline (policy+timeout)   │
    │        └─ UserEvent forwarding               │
    │              │                               │
    │              ├── All Break → RunFinished     │
    │              ├── Any Continue → loop         │
    │              └── Error → error_recovery      │
    │                        ├── Stop → fail       │
    │                        └── Retry → loop      │
    └──────────────────────────────────────────────┘
    │
    ▼
[Session] Clean up ephemeral messages
    │
    ▼
[Session] Save to store
    │
    ▼
Return Ok(RunOutcome)
```

## Pipeline System

Tool execution uses a composable pipeline pattern:

```
ToolExecutionPipeline (trait)
    │
    ├── DefaultPipeline
    │     1. before_call (ToolPolicy hook)
    │     2. tool.call with optional timeout
    │     3. Output truncation (max_output_chars)
    │     4. after_call (ToolPolicy hook)
    │
    └── EventEmittingPipeline<P>  (decorator)
          Wraps any pipeline to emit ToolCallStarted/Finished
          events on the internal EventBus.
```

ToolEngine delegates to `DefaultPipeline` for policy/timeout/truncation, and adds event emission + UserEvent forwarding on top.

## Module Map

```
src/
├── lib.rs                       ─── Public API facade (re-exports)
│
├── engine/                      ─── Core runtime kernel
│   ├── mod.rs                   ─── Module declarations + re-exports
│   ├── builder.rs               ─── AgentBuilder (fluent construction)
│   ├── pipeline.rs              ─── ToolExecutionPipeline trait, DefaultPipeline, EventEmittingPipeline
│   ├── middleware.rs            ─── Middleware trait + PreLlmCtx/PostLlmCtx/UserMessageCtx
│   ├── approval.rs              ─── ApprovalHandler trait + AllowAll/DenyAll
│   ├── session.rs               ─── AgentSession (message history + ephemeral lifecycle)
│   ├── session_store.rs         ─── SessionStore trait + InMemorySessionStore
│   ├── context.rs               ─── ContextWindowManager (token-aware trimming)
│   ├── recovery.rs              ─── ToolErrorRecovery trait + StopOnError/RetryOnError
│   ├── tool_enforcement.rs      ─── ToolEnforcementMiddleware (nudge LLM to use tools)
│   ├── circuit_breaker.rs       ─── CircuitBreaker (experimental)
│   ├── reflexion.rs             ─── ReflexionHandler trait (experimental)
│   │
│   └── runtime/                 ─── Core runtime execution
│       ├── mod.rs               ─── AgentRuntime struct + public API
│       ├── react_loop.rs        ─── ReAct loop + LLM turn + tool dispatch
│       ├── plan_runner.rs       ─── PlanRunner (thin orchestrator wrapper)
│       ├── llm_engine.rs        ─── LlmEngine (LLM call orchestration)
│       ├── tool_engine.rs       ─── ToolEngine (tool dispatch + approval + pipeline)
│       ├── session_manager.rs   ─── SessionManager (session lifecycle + LRU)
│       └── event_bus.rs         ─── EventBus (internal broadcast)
│
├── llm/                         ─── LLM provider abstraction
│   ├── mod.rs                   ─── LlmClient trait, StreamChunk, LlmCapabilities
│   ├── openai.rs                ─── OpenAI / compatible API implementation
│   ├── anthropic.rs             ─── Anthropic API implementation
│   └── registry.rs              ─── LlmClientBuilder (provider selection)
│
├── tool/                        ─── Tool system
│   ├── mod.rs                   ─── Tool trait, TypedTool, ToolRegistry, ToolOutput
│   ├── policy.rs                ─── ToolPolicy trait (approval evaluation)
│   ├── subagent.rs              ─── SubAgentTool + SubAgentSessionPolicy
│   ├── auto_continue.rs         ─── AutoContinueTool
│   └── update_plan.rs           ─── UpdatePlanTool (display-only checklist)
│
└── types/                       ─── Core domain types
    ├── events.rs                ─── RuntimeEvent, PlanEvent, UserEvent
    ├── message.rs               ─── ChatMessage (with ephemeral support), Message, MessageRole
    ├── config.rs                ─── AgentConfig, RetryConfig, Language, ResponseFormat
    ├── error.rs                 ─── AgentError, ErrorKind, AgentResult
    ├── outcome.rs               ─── RunOutcome
    ├── approval.rs              ─── ApprovalRequest, ApprovalDecision, RiskLevel
    ├── checkpoint.rs            ─── CheckpointData
    ├── session.rs               ─── SessionId, SessionIdGenerator
    └── plan_update.rs           ─── UpdatePlanArgs, PlanItem, PlanStepStatus
```

## Key Design Decisions

### Why `RunOutcome` instead of just `Result`?

`RunOutcome` separates "kernel errors" (connection failure, invalid config) from "run results" (task completed, task failed, max turns exceeded). This lets the upper layer handle business outcomes without filtering through error types.

### Why `ToolErrorRecovery` as a trait?

Different agents have fundamentally different recovery preferences. Ops-agents should stop on failure; code-agents should retry. Baking this into the runtime would make it opinionated. The trait keeps the kernel neutral.

### Why memory-first for sessions?

`SessionStore` is an optional persistence adapter. The runtime always operates on in-memory state. This keeps the kernel simple, testable, and suitable for both CLI and long-running server contexts. Persistence becomes an explicit concern for the upper layer.

### Why approval split into Policy + Handler?

- `ToolPolicy` is stateless and synchronous — cheap to evaluate on every call
- `ApprovalHandler` is stateful and async — can involve network, UI, or database

This split lets you implement approval without modifying tool code, and test policy logic independently of the approval UI.

### Why a Pipeline pattern for tool execution?

Tool execution has multiple cross-cutting concerns: policy hooks, timeout, output truncation, event emission. The `ToolExecutionPipeline` trait allows composable decorators (`EventEmittingPipeline`) and keeps each concern isolated.

### Why ephemeral messages?

Injected content (plan hints, reminders, nudges) should be visible to the LLM during the current turn but must not persist across turns or leak into chat history. The `ephemeral` flag on `ChatMessage::System` and `ChatMessage::User` enables this lifecycle without requiring each consumer to implement manual cleanup.

### Event layers

- `RuntimeEvent` (public) — unified event stream emitted directly on the broadcast channel and consumed via `on_event` callback or `subscribe_runtime_events()`
- `UserEvent` (public) — tool-produced events (progress, sub-agent status, structured data), wrapped in `RuntimeEvent::UserEvent`

This separation ensures internal framework evolution doesn't break the public API.

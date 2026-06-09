# Architecture

`agent-base` is a lightweight Agent Runtime Kernel written in Rust. It provides a trait-driven, event-based foundation for building autonomous agents with support for both single-turn (ReAct) and multi-step (Plan) execution modes.

## Layered Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Public API: AgentBuilder → AgentRuntime                        │
├─────────────────────────────────────────────────────────────────┤
│  Engine Layer                                                   │
│  ├─ PlanRunner       Plan generation, execution, recovery       │
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
│  ├─ Middleware            ReAct loop hooks                      │
│  ├─ ToolErrorRecovery    Tool failure strategy                  │
│  ├─ SessionStore         Session persistence                   │
│  ├─ PlanGenerator        Objective → ExecutionPlan              │
│  ├─ StepExecutor         Plan step execution                   │
│  ├─ RecoveryStrategy     Plan step failure strategy             │
│  └─ AdaptiveRecoveryStrategy  Intelligent retry/replan         │
├─────────────────────────────────────────────────────────────────┤
│  Adapters                                                       │
│  ├─ OpenAiClient         OpenAI / compatible APIs              │
│  ├─ AnthropicClient      Anthropic API                         │
│  ├─ InMemorySessionStore Default in-memory sessions            │
│  ├─ LlmPlanGenerator     LLM-driven plan generation            │
│  └─ ToolCallingStepExecutor  Tool-based step execution         │
└─────────────────────────────────────────────────────────────────┘
```

## Three-Layer Event System

Events flow through three layers, from internal to public:

```
AgentEvent (pub(crate))       20 internal framework events
    │                         (11 Plan-related, 9 core)
    ↓ From<AgentEvent>
RuntimeEvent (public)         Unified public event stream
    ├─ System(AgentEvent)     Framework events exposed to consumers
    └─ UserEvent              Tool-produced user events

UserEvent (public)            Events emitted by tools during execution
    ├─ Progress               Progress updates from long-running tools
    ├─ SubAgentEvent          Events from sub-agent tools
    └─ Structured             Arbitrary structured data
```

- **AgentEvent** is the internal bus event — not visible outside the crate
- **RuntimeEvent** is what consumers receive via `on_event` callback or `subscribe_runtime_events()`
- **UserEvent** is produced by tools via `ToolContext.user_event_tx` and forwarded as `RuntimeEvent::UserEvent`

### AgentEvent Variants

| Category | Events |
|----------|--------|
| **Core** | `TextDelta`, `ThoughtDelta`, `ToolCallStarted`, `ToolCallFinished`, `AwaitingApproval`, `Checkpoint`, `RunFinished` |
| **Plan Lifecycle** | `PlanGenerating`, `PlanGenerated`, `PlanFailed`, `PlanCompleted` |
| **Plan Steps** | `PlanStepStarted`, `PlanStepCompleted`, `PlanStepParsed`, `PlanStepWaitingConfirmation` |
| **Plan Recovery** | `PlanReplanning`, `PlanReplanned`, `PlanRecoveryExhausted` |

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
| `PlanGenerator` | Plan System | `LlmPlanGenerator` | Objective → ExecutionPlan |
| `StepExecutor` | Plan System | `ToolCallingStepExecutor` | Execute individual plan steps |
| `RecoveryStrategy` | Plan System | `AbortOnFailure` | Simple step failure: retry/skip/abort |
| `AdaptiveRecoveryStrategy` | Plan System | `LlmAdaptiveRecovery` | Intelligent retry/alternative/replan |
| `ReflexionHandler` | Self-reflection | None | Post-failure self-analysis (experimental) |

## Execution Modes

### Turn Mode (ReAct Loop)

Single-turn interactive execution. The agent processes user input, calls the LLM, executes tools, and returns when the LLM produces a text response or all tools signal `Break`.

```
runtime.run_turn(session_id, user_input, on_event)
```

### Plan Mode

Multi-step goal-driven execution. The agent generates a structured plan, executes steps sequentially or in phases, and applies progressive recovery on failures.

```
runtime.run_plan(session_id, plan, config, on_event)
runtime.run_plan_with_generator(session_id, objective, generator, config, on_event)
```

### Progressive Recovery (4-Level)

```
Level 0: Framework retry (max_retries, linear backoff)
    ↓ exhausted
Level 1: Alternative step (max_alternatives, AdaptiveRecoveryStrategy)
    ↓ exhausted
Level 2: Re-plan (max_replans, AdaptiveRecoveryStrategy)
    ↓ exhausted
Level 3: Fallback RecoveryStrategy (Retry / Skip / Abort)
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

- **ReAct path**: `ToolEngine::execute_tool` delegates to `DefaultPipeline` for policy/timeout/truncation, and adds event emission + UserEvent forwarding on top.
- **Plan path**: `ToolCallingStepExecutor` uses `DefaultPipeline` directly (no event emission, no UserEvent forwarding).

## Module Map

```
src/
├── lib.rs                       ─── Public API facade (re-exports)
├── config_manager.rs            ─── Runtime configuration from env/files
│
├── engine/                      ─── Core runtime kernel
│   ├── mod.rs                   ─── Module declarations + re-exports
│   ├── builder.rs               ─── AgentBuilder (fluent construction)
│   ├── pipeline.rs              ─── ToolExecutionPipeline trait, DefaultPipeline, EventEmittingPipeline
│   ├── middleware.rs             ─── Middleware trait + PreLlmCtx/PostLlmCtx/UserMessageCtx
│   ├── approval.rs              ─── ApprovalHandler trait + AllowAll/DenyAll
│   ├── session.rs               ─── AgentSession (dual message history)
│   ├── session_store.rs         ─── SessionStore trait + InMemorySessionStore
│   ├── context.rs               ─── ContextWindowManager (token-aware trimming)
│   ├── recovery.rs              ─── ToolErrorRecovery trait + StopOnError/RetryOnError
│   ├── tool_enforcement.rs      ─── ToolEnforcementMiddleware (nudge LLM to use tools)
│   ├── circuit_breaker.rs       ─── CircuitBreaker (experimental)
│   ├── reflexion.rs             ─── ReflexionHandler trait (experimental)
│   ├── plan_orchestrator.rs     ─── PlanOrchestrator + PlanExecTool
│   │
│   ├── runtime/                 ─── Core runtime execution
│   │   ├── mod.rs               ─── AgentRuntime struct + public API
│   │   ├── react_loop.rs        ─── ReAct loop + LLM turn + tool dispatch
│   │   ├── plan.rs              ─── Plan execution + progressive recovery (766 lines)
│   │   ├── plan_runner.rs       ─── PlanRunner (thin orchestrator wrapper)
│   │   ├── llm_engine.rs        ─── LlmEngine (LLM call orchestration)
│   │   ├── tool_engine.rs       ─── ToolEngine (tool dispatch + approval + pipeline)
│   │   ├── session_manager.rs   ─── SessionManager (session lifecycle + LRU)
│   │   └── event_bus.rs         ─── EventBus (internal broadcast)
│   │
│   └── plan/                    ─── Plan system
│       ├── mod.rs               ─── PlanConfig + built-in recovery strategies
│       ├── traits.rs            ─── PlanGenerator, StepExecutor, RecoveryStrategy traits
│       ├── executor.rs          ─── ToolCallingStepExecutor
│       ├── llm_generator.rs     ─── LlmPlanGenerator (LLM-driven plan generation, 975 lines)
│       ├── store.rs             ─── PlanStore trait + InMemoryPlanStore
│       ├── adaptive_recovery.rs ─── LlmAdaptiveRecovery (intelligent retry/replan)
│       └── streaming_parser.rs  ─── StreamingJsonParser (streaming plan parsing)
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
│   └── auto_continue.rs         ─── AutoContinueTool
│
└── types/                       ─── Core domain types
    ├── events.rs                ─── AgentEvent, RuntimeEvent, UserEvent
    ├── message.rs               ─── ChatMessage, Message, MessageRole
    ├── config.rs                ─── AgentConfig, RetryConfig, Language
    ├── error.rs                 ─── AgentError, ErrorKind, AgentResult
    ├── outcome.rs               ─── RunOutcome
    ├── approval.rs              ─── ApprovalRequest, ApprovalDecision, RiskLevel
    ├── checkpoint.rs            ─── CheckpointData
    ├── session.rs               ─── SessionId, SessionIdGenerator
    └── plan.rs                  ─── ExecutionPlan, PlanPhase, PlanStep, StepResult
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

Tool execution has multiple cross-cutting concerns: policy hooks, timeout, output truncation, event emission. The `ToolExecutionPipeline` trait allows composable decorators (`EventEmittingPipeline`) and keeps each concern isolated. Both the ReAct loop and the Plan step executor share the same pipeline implementation.

### Why three layers of events?

- `AgentEvent` (pub(crate)) — internal framework events, not exposed to users
- `RuntimeEvent` (public) — unified event stream for consumers (UI, logging, monitoring)
- `UserEvent` (public) — tool-produced events (progress, sub-agent status)

This separation ensures internal framework evolution doesn't break the public API.

### Why 4-level progressive recovery?

Simple retry is not enough for complex multi-step plans. Progressive recovery escalates from simple (retry the same step) to complex (generate an alternative approach, or re-plan entirely) before giving up. This maximizes the chance of success while avoiding infinite loops.

### Why `AgentRuntime` wraps `PlanRunner`?

Both turn mode and plan mode share the same underlying execution infrastructure (LLM engine, tool engine, session manager, event bus). `PlanRunner` is the internal orchestrator that owns these components. `AgentRuntime` is the public facade that delegates to it. This avoids duplicating execution logic between the two modes.

# Architecture

How phi-agent fits together with its dependencies, and why certain decisions were made.

## Dependency Chain

```
agent-base (runtime kernel + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← framework, no tools
    ↑
phi (bin) ← CLI, registers tools here
```

Each crate is a separate repository under [hibuka-labs](https://github.com/hibuka-labs).

## Crate Responsibilities

### agent-base
The runtime kernel:
- `AgentRuntime` — core event loop (LLM chat → tool calls → repeat)
- `Tool` trait — interface all tools implement
- `LlmClient` trait — abstraction over LLM providers
- `RuntimeEvent` — all events emitted during a turn
- `AgentBuilder` — builder pattern for assembling an agent

### agent-works
Built on agent-base:
- **MCP** — Model Context Protocol support
- **Skills** — plugin/skill system
- **Focus** — structured LLM calls with typed input/output

### phi-agent (this crate)
Framework layer — infrastructure only, no tools:
- `base_agent_builder()` — pre-configured builder factory
- `PhiAgent` — high-level wrapper around `AgentRuntime`
- `EventRenderer` — Terminal / JSON / Null output formats
- Config resolution, session management, system prompts

### phi-tools
Tool implementations. On `master`: `LocalShellTool`. Additional tools on other branches.

### phi (binary)
The CLI consumer. Wires everything together: creates `OpenAiClient`, registers tools, runs REPL or one-shot.

## Key Design Decisions

### No built-in tools
phi-agent knows nothing about specific tools. Tools are registered externally via `AgentBuilder::register_tool()`. This keeps the framework lean and consumers in control.

### No built-in memory
No vector DB, no embedding store, no hidden state. Every decision is traceable to what's in the prompt.

### OpenAI-compatible CLI
The CLI uses `OpenAiClient`. For Anthropic, swap to `AnthropicClient` — the framework supports both.

### Session isolation
Each session gets its own directory with file locking, preventing concurrent access from multiple processes. See [Advanced Usage](advanced.md) for details.

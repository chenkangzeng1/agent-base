# Good First Issues — Copy to GitHub

These are ready-to-publish. Copy each one as a new GitHub Issue, add the `good first issue` label.

---

### 1. Extract common API key resolution into `examples/common.rs`

**Labels:** `good first issue`, `refactor`

**Files:** `examples/common.rs` (new), `examples/hello-agent.rs`, `examples/custom-tool.rs`, `examples/multi-tool.rs`, `examples/focus-demo.rs`

**What to do:**

All 4 examples repeat the same ~10 lines to resolve API key/model/base_url from environment variables. Create `examples/common/mod.rs` with a helper struct:

```rust
pub struct LlmEnv {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

pub fn resolve_llm_env() -> LlmEnv {
    // same logic from any example
}
```

Then update all 4 examples to call `common::resolve_llm_env()` instead of inlining.

**How to verify:** `cargo build --examples` passes, each example still runs as before.

---

### 2. Add NullRenderer unit test

**Labels:** `good first issue`, `test`

**Files:** `src/render/mod.rs`

**What to do:**

The `NullRenderer` (in `src/render/null.rs`) has no dedicated test. Add a test in the existing `mod tests` block in `src/render/mod.rs` that:

1. Creates a NullRenderer
2. Feeds it a few `RuntimeEvent` variants (TextDelta, ToolCallStarted, RunFinished)
3. Verifies that `render()` and `finish_turn()` return `Ok(())`

**How to verify:** `cargo test render` passes.

---

### 3. Create an HTML EventRenderer example

**Labels:** `good first issue`, `example`

**Files:** `examples/html-renderer.rs` (new)

**What to do:**

Create an example that implements `EventRenderer` to output HTML instead of terminal text. This demonstrates the framework's extensibility — anyone can write their own renderer.

The HTML renderer should:
- Wrap output in `<html><body>` tags
- Convert `TextDelta` into `<p>` tags
- Convert `ToolCallStarted` into `<details><summary>` tags
- Output to stdout (can pipe to a file)

Look at `src/render/terminal.rs` for reference on how TerminalRenderer implements the trait.

**How to verify:** `cargo run --example html-renderer > output.html && open output.html` shows a readable page.

---

### 4. Add `#[derive(Default)]` to `PhiAgentConfig`

**Labels:** `good first issue`, `enhancement`

**Files:** `src/agent/factory.rs`

**What to do:**

Currently users must spell out all fields of `PhiAgentConfig`. Many have reasonable defaults already defined in `base_agent_builder()`. Add `#[derive(Default)]` to `PhiAgentConfig` and provide sensible `Default` impl values so users can write:

```rust
let config = PhiAgentConfig { model: "opus".into(), ..Default::default() };
```

**How to verify:** `cargo build` passes, existing code behavior unchanged.

---

### 5. Show tool count in `phi serve` startup log

**Labels:** `good first issue`, `enhancement`

**Files:** `src/bin/phi/serve.rs`

**What to do:**

When `phi serve` starts, it lists registered tools. Currently there's no summary count. Add a line like:

```
Bridge server ready. 3 tools registered. Listening on stdin...
```

**How to verify:** `cargo build`, then check `phi serve` output includes the tool count.

# phi-agent

<div class="hero">
<div class="hero-tagline">
General-purpose AI Agent Framework in Rust.<br>Simple. Pure. Predictable.
</div>
<div class="hero-actions">
<a href="./en/guide/getting-started.md" class="btn btn-primary">Get Started →</a>
<a href="./zh/guide/getting-started.md" class="btn btn-ghost">快速开始 →</a>
</div>
</div>

---

<div class="features">

<div class="feature-card">
<div class="icon">🧩</div>

### No Built-in Tools

phi-agent provides the infrastructure — builder factory, renderers, config, session management. You bring your own tools, and keep full control.

</div>

<div class="feature-card">
<div class="icon">⚡</div>

### Pure Rust

Single binary, no runtime dependency. Async, type-safe, zero-cost abstractions. Deploy anywhere — from cloud servers to edge devices.

</div>

<div class="feature-card">
<div class="icon">🧠</div>

### Focus Primitive

Structured single-purpose LLM calls outside the agent loop. Classification, judgment, extraction — one system prompt, one typed output.

</div>

<div class="feature-card">
<div class="icon">🎯</div>

### Your Rules

No built-in memory, no vector DB, no hidden state. The agent doesn't remember anything you don't tell it to. Data stays under your control.

</div>

<div class="feature-card">
<div class="icon">🖥️</div>

### CLI + Library

Use `phi` as a standalone CLI with REPL and one-shot modes. Or import `phi_agent` as a Rust library and build your own application.

</div>

<div class="feature-card">
<div class="icon">🌐</div>

### EN & 中文

Full bilingual documentation. English and Chinese tutorials, guides, and API references — maintained in sync.

</div>

</div>

---

## Architecture

<div class="index-arch">

```
agent-base (runtime kernel + Tool trait)
    ↑
agent-works (MCP · Skills · Focus)
    ↑
phi-agent (lib) ← framework, no tools
    ↑
Your App (CLI, web, etc.) ← you register tools here
```

</div>

## Links

<div class="quick-links">
<a href="https://github.com/hibuka-labs/phi-agent" class="quick-link">GitHub</a>
<a href="https://crates.io/crates/phi-agent" class="quick-link">crates.io</a>
<a href="https://docs.rs/phi-agent" class="quick-link">docs.rs</a>
<a href="./en/guide/getting-started.md" class="quick-link">Documentation →</a>
</div>

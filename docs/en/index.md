---
hide:
  - toc
  - navigation
---

<h1>
  <img src="favicon.svg" style="height: 1.2em; vertical-align: middle; margin-right: 0.3em;">
  phi-agent
</h1>

General-purpose AI Agent Framework in Rust — simple, pure, predictable.

<a href="guide/getting-started/" class="md-button md-button--primary" style="margin-right: 0.5rem">
  :octicons-arrow-right-24: &nbsp; Get Started
</a>
<a href="https://github.com/hibuka-labs/phi-agent" class="md-button" target="_blank">
  :octicons-mark-github-16: &nbsp; View on GitHub
</a>

---

<div class="grid cards" markdown>

-   :simple-rust:{ .lg .middle } **Pure Rust**

    ---

    Async, type-safe, zero-cost abstractions. Runs from cloud to edge — a single binary.

-   :material-lightbulb-on-outline:{ .lg .middle } **Simple**

    ---

    No hidden state, no vector DB, no magic. Everything is explicit and traceable.

-   :material-toy-brick-outline:{ .lg .middle } **Your Rules**

    ---

    The framework doesn't bundle tools. You register exactly what you need via the `Tool` trait.

</div>

---

## Architecture

``` title="Dependency Chain"
agent-base (runtime kernel + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← framework, no tools
    ↑
Your App (CLI, web, etc.) ← you register tools here
```

Each crate is a separate repository under [hibuka-labs](https://github.com/hibuka-labs). All built on Rust's async runtime with `Arc<dyn LlmClient>` for provider abstraction.

## Links

<div class="grid cards" markdown>

-   [:octicons-mark-github-16: GitHub](https://github.com/hibuka-labs/phi-agent)

    Source code, issues, discussions.

-   [:simple-rust: crates.io](https://crates.io/crates/phi-agent)

    Add with `cargo add phi-agent`.

-   [:material-bookshelf: API Docs](https://docs.rs/phi-agent)

    Full Rustdoc on docs.rs.

</div>

---

:material-email-outline: **Contact** &nbsp; [phiagent@hibuka.com](mailto:phiagent@hibuka.com)

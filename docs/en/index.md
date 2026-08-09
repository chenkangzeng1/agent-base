---
hide:
  - toc
  - navigation
---

<h1>
  <img src="favicon.svg" style="height: 1.2em; vertical-align: middle; margin-right: 0.3em;">
  phi-agent
</h1>

Not another AI Agent, but an open application framework for building Agents — built for embedded, edge, and vertical industries, equally suited for highly customizable, high-performance cloud and desktop AI applications. Simple, pure, predictable.

<a href="guide/getting-started/" class="md-button md-button--primary" style="margin-right: 0.5rem">
  :octicons-arrow-right-24: &nbsp; Get Started
</a>
<a href="https://github.com/hibuka-labs/phi-agent" class="md-button" target="_blank">
  :octicons-mark-github-16: &nbsp; View on GitHub
</a>

---

<div class="grid cards" markdown>

-   :material-target:{ .lg .middle } **Built for Vertical Scenarios**

    ---

    Not a generic chatbot, but an Agent framework for embedded, industrial, IoT, and other vertical domains — plus desktop and cloud applications that demand deep customization. Your scenario, your tools, your full control.

-   :material-rocket-launch-outline:{ .lg .middle } **Lightweight, Runs Anywhere**

    ---

    A single Rust binary with zero runtime dependencies — from embedded Linux and edge gateways to cloud containers and desktop applications, `cargo install` gets you started in seconds, deploy anywhere.

</div>

<div class="grid cards" markdown>

-   :material-puzzle-outline:{ .lg .middle } **Your Tools, Your Control**

    ---

    Kernel primitives (file I/O, shell, sub-agents) provide the foundation — everything else you define. A tool is just 3 methods: `name()`, `definition()`, `call()`. Register what you need, the Agent uses what you register. No platform lock-in, no hidden behavior.

-   :material-chart-line:{ .lg .middle } **Fully Observable, Every Step Explainable**

    ---

    Every decision is logged, every step is traceable, with built-in session logging, structured tracing, and session metrics at a glance — compliance and audit trails without the stress.

</div>

---

## Architecture

```mermaid
graph TB
    AB[agent-base<br/>Runtime kernel<br/>Tool trait · LLM clients]

    AB --> AW[agent-works<br/>MCP · Skills · Focus]
    AB --> PKT[phi-kernel-tools<br/>Kernel tools]
    AB --> YT[your-tools<br/>Custom Tool impls]

    AW --> PA
    PKT --> PA
    YT --> PA

    PA[phi-agent<br/>Builder factory · Renderers<br/>Config · Session · CLI]
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

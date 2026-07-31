---
hide:
  - toc
  - navigation
---

<h1>
  <img src="favicon.svg" style="height: 1.2em; vertical-align: middle; margin-right: 0.3em;">
  phi-agent
</h1>

Rust 通用 AI Agent 框架 — 简单、纯粹、可预测。

<a href="guide/getting-started/" class="md-button md-button--primary" style="margin-right: 0.5rem">
  :octicons-arrow-right-24: &nbsp; 快速开始
</a>
<a href="https://github.com/hibuka-labs/phi-agent" class="md-button" target="_blank">
  :octicons-mark-github-16: &nbsp; GitHub
</a>

---

<div class="grid cards" markdown>

-   :simple-rust:{ .lg .middle } **纯 Rust**

    ---

    异步、类型安全、零成本抽象。从云端到边缘，一个二进制文件。

-   :material-lightbulb-on-outline:{ .lg .middle } **简单**

    ---

    无隐藏状态、无向量数据库、无黑魔法。一切显式可控、可追溯。

-   :material-toy-brick-outline:{ .lg .middle } **你说了算**

    ---

    框架不内置工具。你通过 `Tool` trait 注册自己需要的工具，完全掌控。

</div>

---

## 架构

``` title="依赖链"
agent-base (运行时内核 + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← 框架层，不含工具
    ↑
你的应用 (CLI, Web 等) ← 在这里注册工具
```

每个 crate 都是 [hibuka-labs](https://github.com/hibuka-labs) 下的独立仓库。基于 Rust 异步运行时，通过 `Arc<dyn LlmClient>` 实现 LLM 提供商抽象。

## 链接

<div class="grid cards" markdown>

-   [:octicons-mark-github-16: GitHub](https://github.com/hibuka-labs/phi-agent)

    源码、Issues、讨论。

-   [:simple-rust: crates.io](https://crates.io/crates/phi-agent)

    `cargo add phi-agent` 即可引入。

-   [:material-bookshelf: API 文档](https://docs.rs/phi-agent)

    完整的 Rustdoc 文档。

</div>

---

:material-email-outline: **联系** &nbsp; [phiagent@hibuka.com](mailto:phiagent@hibuka.com)

---
hide:
  - toc
  - navigation
---

<h1>
  <img src="favicon.svg" style="height: 1.2em; vertical-align: middle; margin-right: 0.3em;">
  phi-agent
</h1>

Rust AI Agent 框架。Agent 的调度、会话、流式输出——框架搞定。你只写三样东西：你的工具、你的提示词、你的领域知识。

<a href="guide/getting-started/" class="md-button md-button--primary" style="margin-right: 0.5rem">
  :octicons-arrow-right-24: &nbsp; 快速开始
</a>
<a href="https://github.com/hibuka-labs/phi-agent" class="md-button" target="_blank">
  :octicons-mark-github-16: &nbsp; GitHub
</a>

---

<div class="grid cards col-3" markdown>

-   :material-target:{ .lg .middle } **你的领域，你做主**

    ---

    Agent 调度循环、会话管理、流式事件、工具路由、审批拦截——框架全做了。你不用写胶水代码，不用重做状态管理。专注你的领域逻辑。

-   :material-rocket-launch-outline:{ .lg .middle } **单一二进制，零依赖**

    ---

    不需要 Node.js。不需要 Python。编译出来就一个文件，丢过去就跑。`cargo install`，十秒起步。

-   :material-chart-line:{ .lg .middle } **每一步可审计**

    ---

    每一次 LLM 调用、每一次工具执行，JSONL 全记录。会话可快照、行为可追踪，问题可定位。

</div>

---

## 架构

```mermaid
graph TB
    AB[agent-base<br/>运行时内核<br/>Tool trait · LLM 客户端]

    AB --> AW[agent-works<br/>MCP · Skills · Focus]
    AB --> PKT["phi-kernel-tools<br/>内核工具<br/>（默认关闭）"]
    AB --> YT[your-tools<br/>自定义工具实现]

    AW --> PA
    PKT --> PA
    YT --> PA

    PA[phi-agent<br/>Builder 工厂 · 渲染器<br/>配置 · 会话 · CLI]
```

每个 crate 都是 [hibuka-labs](https://github.com/hibuka-labs) 下的独立仓库。基于 Rust 异步运行时，通过 `Arc<dyn LlmClient>` 实现 LLM 提供商抽象。`phi-kernel-tools`（内核工具）**默认关闭** — 详见[内核工具](guide/tools/file-tools/)了解如何启用。

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

# phi-agent

**Rust 通用 AI Agent 框架 — 简单、纯粹、可预测。**

---

## 什么是 phi-agent？

phi-agent 是一个用 Rust 构建 AI Agent 的框架。它提供基础设施 — Builder 工厂、渲染器、配置、会话管理 — 但**不捆绑任何工具**。你自带工具，完全掌控。

## 为什么选择 phi-agent？

- **简单** — 无隐藏状态，无向量数据库，无黑魔法。一切显式可控。
- **纯 Rust** — 异步、类型安全、零成本抽象。从云端到边缘皆可运行。
- **你说了算** — 框架不决定 Agent 拥有什么工具。你来决定。

## 架构

```
agent-base (运行时内核 + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← 框架，无工具
    ↑
你的应用 (CLI, Web 等) ← 在这里注册工具
```

## 链接

- [GitHub](https://github.com/hibuka-labs/phi-agent)
- [crates.io](https://crates.io/crates/phi-agent)
- [API 文档 (docs.rs)](https://docs.rs/phi-agent)

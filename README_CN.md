# phi-agent

[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

通用 AI Agent 框架，构建在 [agent-base](https://crates.io/crates/agent-base) 之上。

提供 builder 工厂、渲染器、配置解析、session 管理等基础设施。**不内置任何工具**，工具由消费方注入。

## 特性

- **Builder 工厂** — `base_agent_builder()` 提供合理默认配置
- **多种渲染器** — 终端（富文本）、JSON 流、空渲染器
- **CLI 开箱即用** — 支持 REPL 和 one-shot 模式
- **Session 管理** — 自动清理、文件锁、turn 日志
- **工具无关** — 不内置工具，通过 `AgentBuilder` 自行注册

## 快速开始

```rust
use phi_agent::{
    base_agent_builder, PhiAgent, PhiAgentConfig,
    OpenAiClient, SafetyConfig, ReasoningEffort,
};
use std::sync::Arc;

// 1. 创建 LLM 客户端
let llm_client = Arc::new(OpenAiClient::new(
    api_key, model, Some(base_url),
));

// 2. 构建 agent（在此注册你的工具）
let builder = base_agent_builder(llm_client)
    .system_prompt(build_system_prompt())
    .register_tool(your_tool);

let agent = PhiAgent::build(builder, PhiAgentConfig {
    model: model.into(),
    enable_thinking: true,
    thinking_budget: None,
    thinking_effort: ReasoningEffort::Medium,
    safety: SafetyConfig::default(),
})?;

// 3. 运行
let session = agent.create_session().await;
agent.run_turn(session, "你好！", |event| {
    renderer.render(event)
}).await?;
```

## CLI

```bash
cargo install phi-agent
phi "当前目录有什么文件？"
```

## License

MIT

[English](README.md)

# 快速开始

5 分钟跑起你的第一个 phi-agent。

## 前置条件

- [Rust](https://rustup.rs)（stable，edition 2024）
- 一个 LLM API Key（兼容 OpenAI 接口）

## 1. 创建项目

```bash
cargo new my-agent
cd my-agent
```

## 2. 添加依赖

```bash
cargo add phi-agent
cargo add tokio --features full
cargo add anyhow
```

## 3. 配置 API Key

```bash
cp .env.example .env
# 编辑 .env，填入你的真实 API Key
```

`.env.example` 中包含了 OpenAI、Anthropic、DeepSeek 等常见提供商的配置示例。详见[配置详解](configuration.md)。

## 4. 编写代码

```rust
// src/main.rs
use std::sync::Arc;
use phi_agent::{
    base_agent_builder, build_system_prompt, create_stdout_renderer,
    PhiAgent, PhiAgentConfig, OpenAiClient, OutputFormat,
    ReasoningEffort, SafetyConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 从环境变量获取 API Key
    let api_key = std::env::var("LLM_API_KEY")?;
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "opus".into());
    let base_url = std::env::var("LLM_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".into());

    // 创建 LLM 客户端
    let llm_client = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    // 构建 Agent
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt());

    let agent = PhiAgent::build(builder, PhiAgentConfig {
        model,
        enable_thinking: true,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
    })?;

    // 运行
    let session = agent.create_session().await;
    let renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true, show_tool_args: true, color: true,
    });

    agent.run_turn(session, "你好，世界！", |event| {
        renderer.render(event)
    }).await?;

    Ok(())
}
```

## 5. 运行

```bash
cargo run
```

## 下一步

- [自定义工具](custom-tool.md) — 为 Agent 添加你自己的工具
- [Focus 专注判断](focus.md) — 结构化单任务 LLM 调用
- [配置详解](configuration.md) — 了解所有配置选项
- [高级用法](advanced.md) — 中间件、会话、事件日志

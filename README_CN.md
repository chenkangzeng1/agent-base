# phi-agent

[![CI](https://github.com/hibuka-labs/phi-agent/workflows/CI/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

通用 AI Agent 框架，基于 Rust 构建，底层依赖 [agent-base](https://crates.io/crates/agent-base) 和 [agent-works](https://crates.io/crates/agent-works)。

**phi-agent 提供基础设施。工具由你来定义。**

## 为什么选择 phi-agent

**简单。** 一个工具只需实现 3 个方法：`name()`、`definition()`、`call()`。无需学习框架，没有抽象概念需要消化。

**Rust。** 单二进制文件，无运行时依赖。`cargo install` 即可使用。内存安全，不易崩溃，性能出色。

**纯粹。** 不内置记忆存储、不捆绑向量数据库、没有隐藏状态。Agent 不会记住你没告诉它的东西。行为可预测、可调试，数据去向由你掌控。

**你的工具，你的规则。** phi-agent 不知道有哪些工具存在。你提供工具，你拥有完全控制权。没有供应商锁定。

## 特性

- **Builder 工厂** — `base_agent_builder()` 提供合理默认值（thinking、recovery、limits）
- **三种渲染器** — Terminal（彩色、流式）、JSON stream（JSONL）、Null（静默）
- **CLI 开箱即用** — REPL 和 one-shot 模式，30+ 可配置参数
- **会话管理** — 自动清理、文件锁、JSONL 对话日志
- **工具无关** — 不内置任何工具，通过 `AgentBuilder` 注册你自己的
- **可扩展** — 中间件、审批处理器、自定义渲染器

## 快速开始

```rust
use phi_agent::{
    base_agent_builder, build_system_prompt, PhiAgent, PhiAgentConfig,
    OpenAiClient, SafetyConfig, ReasoningEffort,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 创建 LLM 客户端
    let llm_client = Arc::new(OpenAiClient::new(
        std::env::var("LLM_API_KEY")?,
        "opus".into(),
        Some("https://api.openai.com/v1".into()),
    ));

    // 2. 构建 Agent（在这里注册你的工具）
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt())
        .register_tool(your_tool);

    let agent = PhiAgent::build(builder, PhiAgentConfig {
        model: "opus".into(),
        enable_thinking: true,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
    })?;

    // 3. 运行
    let session = agent.create_session().await;
    let renderer = phi_agent::create_stdout_renderer(
        &phi_agent::OutputFormat::Terminal {
            show_thinking: true,
            show_tool_args: true,
            color: true,
        }
    );

    agent.run_turn(session, "你好！", |event| {
        renderer.render(event)
    }).await?;

    Ok(())
}
```

更多示例参见 [examples/](examples/)。

## CLI

```bash
cargo install phi-agent
phi "这个目录下有什么文件？"
```

```bash
# REPL 模式
phi

# JSON 输出（方便脚本处理）
phi --format json "列出文件"
```

## 自定义工具示例

```rust
use agent_base::{Tool, ToolContext, ToolOutput, ToolControlFlow, AgentResult};
use serde_json::{Value, json};
use async_trait::async_trait;

struct HelloTool;

#[async_trait]
impl Tool for HelloTool {
    fn name(&self) -> &'static str { "hello" }

    fn definition(&self) -> Value {
        json!({
            "name": "hello",
            "description": "对某人打招呼",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "要打招呼的对象" }
                },
                "required": ["name"]
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("世界");
        Ok(ToolOutput {
            summary: format!("你好，{}！", name),
            control_flow: ToolControlFlow::Continue,
            raw: None,
            truncation: None,
        })
    }
}
```

完整教程：[guide/custom-tool_CN.md](guide/custom-tool_CN.md)

## 文档

| 文档 | 说明 |
|------|------|
| [快速开始](guide/getting-started_CN.md) | 5 分钟上手 |
| [自定义工具](guide/custom-tool_CN.md) | 如何编写 Tool |
| [配置详解](guide/configuration_CN.md) | 配置参考 |
| [高级用法](guide/advanced_CN.md) | 中间件、会话、事件日志 |
| [架构设计](docs/design.md) | 完整架构文档 |

## 常见问题

**Q: phi-agent 和 agent-base 有什么区别？**

agent-base 是运行时内核（LLM 调用、工具编排、事件流）。phi-agent 在其上封装了 builder 工厂、渲染器、配置解析和会话管理，并提供 CLI 二进制文件。

**Q: 可以不用 CLI，只用库吗？**

可以。引入 `phi_agent` 作为依赖，通过 `PhiAgent::build()` 编程式使用。CLI 只是其中一个消费方。

**Q: 怎么添加自己的工具？**

实现 `agent-base` 中的 `Tool` trait，然后通过 `builder.register_tool(...)` 注册。phi-agent 本身对工具有哪些一无所知。

**Q: 支持 Anthropic / 其他模型提供商吗？**

支持。agent-base 提供了 `AnthropicClient` 和 `OpenAiClient`。任何实现了 `LlmClient` 的客户端都能使用。

## 参与贡献

参见 [CONTRIBUTING.md](CONTRIBUTING.md) 了解开发环境搭建和 PR 流程。

## 许可证

MIT — 详见 [LICENSE](LICENSE)。

[English](README.md)

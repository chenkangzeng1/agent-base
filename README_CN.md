# <picture><source media="(prefers-color-scheme: dark)" srcset="assets/logo.svg"><img alt="phi-agent" src="assets/logo.svg" height="60"></picture>

[![CI](https://github.com/hibuka-labs/phi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-book-green.svg)](https://docs.phi-agent.dev)

通用 AI Agent 框架，基于 Rust 构建，底层依赖 [agent-base](https://crates.io/crates/agent-base) 和 [agent-works](https://crates.io/crates/agent-works)。

**phi-agent 提供基础设施。工具由你来定义。**

## 生态

phi-agent 是一组独立 crate 的成员：

| Crate | crates.io | 说明 |
|-------|-----------|------|
| `agent-base` | [![Crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base) | 轻量运行时内核 — LLM 客户端、Tool trait、事件流 |
| `agent-works` | [![Crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works) | 功能工具箱 — MCP、Skills、Focus |
| `phi-agent` | [![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent) | 完整框架 — builder 工厂、渲染器、配置、CLI 二进制 |

**只需要运行时？** `cargo add agent-base`。**需要完整框架？** `cargo add phi-agent`。

## 架构

```
                      ┌─────────────────────┐
                      │     agent-base       │
                      │  Tool trait · 运行时  │
                      │  LLM 客户端 · 事件     │
                      └──────────┬──────────┘
                                 │
          ┌──────────────────────┼──────────────────────┐
          │                      │                      │
┌─────────▼─────────┐  ┌────────▼────────┐  ┌──────────▼──────────┐
│    agent-works     │  │   phi-tools     │  │     你的工具         │
│  MCP · Skills      │  │ LocalShellTool  │  │  自定义 Tool 实现    │
│  Focus             │  │                 │  │                     │
└─────────┬─────────┘  └────────┬────────┘  └──────────┬──────────┘
          │                      │                      │
          └──────────────────────┼──────────────────────┘
                                 │
                      ┌──────────▼──────────┐
                      │     phi-agent        │
                      │  Builder 工厂        │
                      │  渲染器 (3 种)       │
                      │  配置 · 会话管理     │
                      │  CLI (phi)            │
                      └──────────┬──────────┘
                                 │
                    ┌────────────┼────────────┐
                    │            │            │
              ┌─────▼────┐ ┌────▼─────┐ ┌────▼─────┐
              │ Terminal  │ │  JSON    │ │   Web    │
              │   REPL    │ │  Stream  │ │  后端     │
              └───────────┘ └──────────┘ └──────────┘
```

**核心理念**：phi-agent 本身**不内置**任何工具。它提供 agent builder 工厂、渲染器、配置解析和会话管理——工具由消费方注入。

## 为什么选择 phi-agent

**Rust。** 单二进制文件，无运行时依赖。`cargo install` 即可使用。内存安全，不易崩溃，性能出色。随处部署——从云服务器到边缘设备。

**简单。** 无隐藏状态，无黑魔法。显式控制流，可读、可追踪、可信赖。一个工具只需 3 个方法 — `name()`、`definition()`、`call()`。

**你的工具，你的规则。** phi-agent 零内置工具。你提供工具，你拥有完全控制权。没有供应商锁定。

**可观测。** 内置对话日志、会话指标、链路追踪。每个决策有记录，每次结果可度量。Agent 的每一步都有据可查。

## 特性

- **Builder 工厂** — `base_agent_builder()` 提供合理默认值（thinking、recovery、limits）
- **三种渲染器** — Terminal（彩色、流式）、JSON stream（JSONL）、Null（静默）
- **CLI 开箱即用** — REPL 和 one-shot 模式，20+ 可配置参数
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

## 浏览器自动化

phi-agent 内置 21 个浏览器工具（通过 `browser` Cargo feature 控制），支持网页浏览、表单交互、数据提取，基于 Chrome DevTools Protocol。

### 快速开始

```bash
# 编译并启用浏览器功能
cargo run --features browser -- --enable-browser "上网查今天天气"

# Headed 模式（可见浏览器窗口，便于调试）
cargo run --features browser -- --enable-browser --headed "打开淘宝搜索机械键盘"

# 连接已有的 Chrome 实例
# 首先启动 Chrome 并开启远程调试：
#   /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --remote-debugging-port=9222
cargo run --features browser -- --connect-ws ws://localhost:9222 "在当前页面查找..."
```

### 浏览器工具列表（21 个）

| 类别 | 工具 |
|---|---|
| **导航** | `browser_navigate`, `browser_go_back`, `browser_go_forward`, `browser_wait` |
| **交互** | `browser_click`, `browser_hover`, `browser_input_fill`, `browser_select`, `browser_press_key`, `browser_scroll` |
| **查看** | `browser_snapshot`, `browser_screenshot`, `browser_get_markdown`, `browser_read_links`, `browser_evaluate` |
| **标签页** | `browser_new_tab`, `browser_tab_list`, `browser_switch_tab`, `browser_close_tab` |
| **控制** | `browser_close`, `browser_extract_content` |

### 工作原理

1. `--enable-browser` 启动一个无头 Chrome 实例
2. `browser_navigate` 打开网页并返回 ARIA 无障碍快照，可交互元素带有数字索引
3. AI 通过索引点击元素（如 `browser_click index=5`），无需编写脆弱的 CSS 选择器
4. `browser_screenshot` 截取页面截图；`browser_get_markdown` 提取可读内容

### 环境要求

- 需安装 Chrome 或 Chromium
- 编译需带 `--features browser`（`browser` feature 控制 `headless_chrome` 等重量级依赖）

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

📖 **完整文档站**: [docs.phi-agent.dev](https://docs.phi-agent.dev)

| 文档 | 说明 |
|------|------|
| [快速开始](guide/getting-started_CN.md) | 5 分钟上手 |
| [自定义工具](guide/custom-tool_CN.md) | 如何编写 Tool |
| [CLI 使用](guide/cli_CN.md) | CLI 参数、REPL、one-shot |
| [配置详解](guide/configuration_CN.md) | 配置参考 |
| [Focus 专注判断](guide/focus_CN.md) | 结构化单任务 LLM 调用 |
| [架构设计](guide/architecture_CN.md) | 设计决策与内部原理 |
| [可观测性](guide/observability_CN.md) | 日志、追踪、指标 |
| [高级用法](guide/advanced_CN.md) | 中间件、会话、事件日志 |

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

## 联系

:material-email-outline: [phiagent@hibuka.com](mailto:phiagent@hibuka.com)

[English](README.md)

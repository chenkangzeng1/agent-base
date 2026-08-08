# <picture><source media="(prefers-color-scheme: dark)" srcset="assets/logo.svg"><img alt="phi-agent" src="assets/logo.svg" height="60"></picture>

[![CI](https://github.com/hibuka-labs/phi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/hibuka-labs/phi-agent/actions)
[![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent)
[![Docs.rs](https://docs.rs/phi-agent/badge.svg)](https://docs.rs/phi-agent)
[![codecov](https://codecov.io/gh/hibuka-labs/phi-agent/branch/master/graph/badge.svg)](https://codecov.io/gh/hibuka-labs/phi-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-book-green.svg)](https://docs.phi-agent.dev)
[![PyPI](https://img.shields.io/pypi/v/phi-agent.svg)](https://pypi.org/project/phi-agent/)

不是又一个 AI Agent，而是构建 Agent 应用的开放基座 — 专为嵌入式、边缘及垂直行业打造，同样适合高定制、高性能的云端和桌面 AI 应用，简单、纯粹、可控。

> **与 LangChain、CrewAI、AutoGen 不同，phi-agent 不内置任何工具。** 没有预设的工具集，没有隐藏的 prompt 工程，没有黑盒的 workflow 引擎 — 只是一个干净的 Rust 运行时。每个工具由你定义，所有行为由你掌控。

基于 [agent-base](https://crates.io/crates/agent-base) 和 [agent-works](https://crates.io/crates/agent-works) 构建。**phi-agent 提供基础设施，工具由你来定义。**

## 生态

phi-agent 是一组独立 crate 的成员：

| Crate | crates.io | 说明 |
|-------|-----------|------|
| `agent-base` | [![Crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base) | 轻量运行时内核 — LLM 客户端、Tool trait、事件流 |
| `agent-works` | [![Crates.io](https://img.shields.io/crates/v/agent-works.svg)](https://crates.io/crates/agent-works) | 功能工具箱 — MCP、Skills、Focus |
| `phi-agent` | [![Crates.io](https://img.shields.io/crates/v/phi-agent.svg)](https://crates.io/crates/phi-agent) | 完整框架 — builder 工厂、渲染器、配置、CLI 二进制 |

**只需要运行时？** `cargo add agent-base`。**需要完整框架？** `cargo add phi-agent`。

## SDK

更喜欢 Python？phi-agent 支持多语言 — 用你喜欢的语言编写工具，同一套 Rust 运行时驱动。

| 语言 | 安装 | 版本 |
|------|------|------|
| Python | `pip install phi-agent` | [![PyPI](https://img.shields.io/pypi/v/phi-agent.svg)](https://pypi.org/project/phi-agent/) |

### Python

```bash
pip install phi-agent
```

```python
from phi_agent import Agent, tool

@tool
async def search(query: str) -> str:
    """搜索网页。"""
    return f"搜索结果: {query}"

agent = Agent(model="gpt-4o")
agent.register(search)

async for event in agent.run("今天有什么新闻?"):
    print(event)
```

Python SDK 通过 stdio 与 `phi` Rust 二进制通信 — 你用 Python 写工具，Rust 运行时负责 Agent 循环、LLM 调用和事件流。

📖 [Python SDK 文档 →](https://pypi.org/project/phi-agent/)

## 架构

```mermaid
graph TB
    AB[agent-base<br/>Tool trait · 运行时<br/>LLM 客户端 · 事件]

    AB --> AW[agent-works<br/>MCP · Skills · Focus]
    AB --> PT[phi-tools<br/>LocalShellTool]
    AB --> YT[你的工具<br/>自定义 Tool 实现]

    AW --> PA
    PT --> PA
    YT --> PA

    PA[phi-agent<br/>Builder 工厂<br/>渲染器 · 配置 · 会话管理<br/>CLI binary]

    PA --> Terminal[Terminal REPL]
    PA --> JSON[JSON Stream]
    PA --> Web[Web 后端]
```

**核心理念**：phi-agent **不内置**任何工具。工具由你定义、由你注册，phi-agent 在运行时自动发现、管理和调度——工具列表可查、调用可追踪。

## 为什么选择 phi-agent

**为垂直场景而生。** 不是通用 chatbot，而是面向嵌入式、工业、IoT 等垂直领域，以及桌面、云端等高定制场景的 Agent 构建框架 — 你的场景，你定义工具，你掌控行为。

**极致轻量，哪里都能跑。** Rust 单二进制，无运行时依赖，从嵌入式 Linux、边缘网关到云端容器、桌面应用，`cargo install` 即用，随地部署。

**零内置，全定制。** 不预设任何工具，不绑定任何平台，一个工具只需 3 个方法 — `name()`、`definition()`、`call()`，你注册什么，Agent 就用什么，只带你的场景真正需要的东西，LLM 自由，精准、干净、可控。

**全程可观测，每一步可解释。** 每次决策有记录，每个步骤可追踪，内置会话日志与结构化追踪，会话指标一目了然，垂直场景合规审计无压力。

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

更多示例参见 [examples/](examples/)，按类别组织：
- [minimal/](examples/minimal/) — 最简示例
- [tools/](examples/tools/) — 自定义工具与审批策略
- [mcp/](examples/mcp/) — MCP 客户端与动态连接管理
- [session/](examples/session/) — 会话持久化与生命周期
- [observability/](examples/observability/) — 事件日志、中间件钩子
- [advanced/](examples/advanced/) — 滑动窗口记忆、摘要记忆、专注判断

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

### 从源码构建

```bash
git clone https://github.com/hibuka-labs/phi-agent.git
cd phi-agent

# phi 二进制需要 shell、mcp、telemetry、logging 四个 feature
cargo run --features full

# 或单独指定：
cargo run --features shell,mcp,telemetry,logging
```

## ✅ 适合做什么

- **嵌入式 & 边缘应用** — 单二进制，零系统依赖，可在 ARM Linux 和 IoT 网关上运行
- **工业 & 合规场景** — 全程可观测，每步记录到 JSONL，开箱即用的审计追踪
- **桌面 & 云端 AI 应用** — `cargo install` 即可作为 CLI/后端使用，也可作为库嵌入
- **垂直领域定制 Agent** — 每个工具由你定义，每句 prompt 由你掌控，零供应商锁定
- **高性能工作流** — Rust 运行时，异步 I/O，亚毫秒级工具调度
- **Python + Rust 混合开发** — Python 写工具，Rust 跑引擎

## ⚠️ 不提供什么

phi-agent **框架本身**刻意保持精简。以下功能**明确不内建到框架中**（但部分可能以独立可选 crate 形式在生态中提供）：

- **内建工具** — 框架不内建任何工具，不提供网页搜索、文件系统、代码执行器、数据库连接器。所有工具由你定义和注册。生态提供了可选配套 crate（如 `phi-tools`、规划中的 `phi-extra`），后续也可能随发展贡献更多工具库 — 全部按需引入，不捆绑。
- **内建记忆 / 向量数据库** — 不集成 Pinecone/Chroma/Weaviate，不自动做 embedding。状态管理由你自己掌控。
- **预设 Agent 类型** — 不提供"研究 Agent""编程 Agent""客服 Agent"等模板。你自行组合。
- **工作流引擎** — 不提供 DAG 执行、条件分支引擎、LangGraph 式图编译器。Agent 行为由 LLM tool-choice 驱动。
- **Prompt 模板** — 不提供 langchain 式的 prompt 链、自动上下文填充。system prompt 由你控制。
- **HTTP 服务器** — phi-agent 是库 + CLI。服务器层（Actix/Axum/Warp）由你自行搭建。
- **多 Agent 编排** — 暂不在当前版本范围内（v0.4.0 路线图中，但会作为独立可选 crate）。

如果需要上述功能，可将 phi-agent 与以下组件结合使用：
- **记忆**：自选向量数据库（Qdrant、pgvector、LanceDB）
- **工作流**：使用 [LangGraph](https://www.langchain.com/langgraph) 或 [Temporal](https://temporal.io/) 做编排
- **工具**：使用 [phi-tools](https://crates.io/crates/phi-tools) 获取常用工具，或自行构建
- **HTTP**：搭配 [axum](https://crates.io/crates/axum) 或 [actix-web](https://crates.io/crates/actix-web)

## 🧩 phi-agent + LangGraph

phi-agent 和 **LangGraph** 解决不同层面的问题，可以很好地协同：

| | phi-agent | LangGraph |
|---|---|---|
| **做什么** | 单 Agent 运行时 | 多步骤工作流引擎 |
| **优势** | 快速工具调度、事件流、嵌入式部署 | 基于图的控制流、检查点、人机协同 |
| **如何配合** | 作为 LangGraph 图中的 Agent 节点 | 作为 phi-agent 之上的编排层 |

**推荐模式**：用 LangGraph 管理工作流级别的控制流（路由、分支、重试），用 phi-agent 作为单个 Agent 节点的执行引擎。phi-agent Agent → LangGraph 节点，phi-agent 工具 → LangChain 工具。

## 🔒 安全提醒

**phi-agent 不会对 LLM 进行沙箱隔离，也不会对工具调用做安全过滤。** Agent 会执行你注册的任何工具，工具拥有什么权限，LLM 就能使用什么权限。你需要自行负责：

- **工具权限** — 如果你注册了 shell 工具，LLM 就能执行任意命令。请考虑使用允许列表、沙箱或操作系统级限制。
- **Prompt 注入** — 用户输入直接进入 prompt，框架不做输入过滤或清洗。
- **网络访问** — LLM 客户端会向你配置的 API 端点发起 HTTP 请求，框架不做流量检查。
- **会话数据** — 会话日志以明文 JSONL 存储在 `~/.phi-agent/sessions/`，可能包含对话中的敏感信息。

**生产环境建议**：遵循最小权限原则 — 只注册 Agent 真正需要的工具，并以最小必要的操作系统权限运行 Agent 进程。

安全漏洞请报告至 **[phiagent@hibuka.com](mailto:phiagent@hibuka.com)**。详见 [SECURITY.md](SECURITY.md)。

## 常见问题

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

## 贡献者

感谢所有为这个项目做出贡献的人：

<!-- ALL-CONTRIBUTORS-LIST:START - Do not remove or modify this section -->
<!-- prettier-ignore-start -->
<!-- markdownlint-disable -->
<table>
  <tr>
    <td align="center"><a href="https://github.com/shard872"><img src="https://github.com/shard872.png" width="100px;" alt=""/><br /><sub><b>shard872</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/7" title="Code">💻</a></td>
    <td align="center"><a href="https://github.com/Krshs90"><img src="https://github.com/Krshs90.png" width="100px;" alt=""/><br /><sub><b>Krish Shah</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/8" title="Code">💻</a></td>
    <td align="center"><a href="https://github.com/slegarraga"><img src="https://github.com/slegarraga.png" width="100px;" alt=""/><br /><sub><b>Sebastian Legarraga</b></sub></a><br /><a href="https://github.com/hibuka-labs/phi-agent/pull/9" title="Code">💻</a></td>
  </tr>
</table>
<!-- ALL-CONTRIBUTORS-LIST:END -->

([emoji key](https://allcontributors.org/docs/en/emoji-key)) — 本项目遵循 [all-contributors](https://github.com/all-contributors/all-contributors) 规范。

## 许可证

MIT — 详见 [LICENSE](LICENSE)。

## 联系

:material-email-outline: [phiagent@hibuka.com](mailto:phiagent@hibuka.com)

[English](README.md)

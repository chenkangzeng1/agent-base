# 快速开始

5 分钟跑起你的第一个 phi-agent。

## 前置条件

- [Rust](https://rustup.rs)（stable，edition 2024）
- 一个 LLM API Key（兼容 OpenAI 接口）

## 安装

```bash
cargo install phi-agent
```

## 方式一：一键生成（推荐）

用 `phi init` 生成完整项目，包含一个示例工具和 REPL：

```bash
phi init my-agent
cd my-agent
cp .env.example .env   # 编辑 .env 填入 API Key
cargo run
```

```
phi> 现在几点了？
🔧 get_time
当前时间：2025-07-30 19:30:00

phi> /exit
```

打开 `src/main.rs`，你会看到 `ClockTool` 的完整代码。照着它写你自己的工具，注册到 Agent 就行。

详见 [自定义工具](custom-tool.md)。

## 方式二：库集成

把 phi-agent 作为库加入已有项目：

```bash
cargo add phi-agent tokio --features full anyhow dotenvy async-trait serde_json chrono
```

完整示例 `src/main.rs`：

```rust
use phi_agent::{
    base_agent_builder, build_system_prompt,
    PhiAgent, PhiAgentConfig, OpenAiClient,
    SafetyConfig, ReasoningEffort,
    OutputFormat, create_stdout_renderer,
    AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

// 1. 定义你的工具
struct ClockTool;

#[async_trait]
impl Tool for ClockTool {
    fn name(&self) -> &'static str { "get_time" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_time",
                "description": "获取当前日期和时间",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        Ok(ToolOutput {
            summary: format!("当前时间：{}", now),
            control_flow: ToolControlFlow::Continue,
            raw: None, truncation: None,
        })
    }
}

// 2. 注册工具，构建 Agent
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".into());
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("LLM_API_KEY")?,
        model.clone(),
        std::env::var("LLM_BASE_URL").ok(),
    ));

    let agent = PhiAgent::build(
        base_agent_builder(llm)
            .system_prompt(build_system_prompt())
            .register_tool(ClockTool),      // 注册你的工具
        PhiAgentConfig {
            model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    // 3. 运行
    let session = agent.create_session().await;
    let mut renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true, show_tool_args: true, color: true,
    });
    agent.run_turn(session, "现在几点了？", |event| renderer.render(event)).await?;
    Ok(())
}
```

三步：定义 Tool → 注册到 Agent → 运行。更多工具示例见 [自定义工具](custom-tool.md)。
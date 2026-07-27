# 自定义工具

phi-agent 不内置任何工具 — 你通过实现 `Tool` trait 来创建自己的工具。

## Tool Trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> Value;
    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput>;
}
```

需要实现三个方法：

| 方法 | 作用 |
|------|------|
| `name()` | LLM 调用此工具时使用的唯一标识 |
| `definition()` | JSON Schema 描述参数（发送给 LLM） |
| `call()` | 实际逻辑 — 接收解析后的参数，返回结果 |

## 示例：天气工具

```rust
use agent_base::{AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};
use async_trait::async_trait;
use serde_json::{Value, json};

struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &'static str {
        "get_weather"
    }

    fn definition(&self) -> Value {
        json!({
            "name": "get_weather",
            "description": "获取指定城市的当前天气",
            "parameters": {
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string",
                        "description": "城市名称，例如 '北京'"
                    }
                },
                "required": ["city"]
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let city = args["city"].as_str().unwrap_or("unknown");
        // 生产环境中，这里调用真实的天气 API
        Ok(ToolOutput {
            summary: format!("{} 天气：22°C，晴", city),
            control_flow: ToolControlFlow::Continue,
            raw: None,
            truncation: None,
        })
    }
}
```

## 注册工具

```rust
let builder = base_agent_builder(llm_client)
    .system_prompt(build_system_prompt())
    .register_tool(WeatherTool);   // ← 在这里注册

let agent = PhiAgent::build(builder, config)?;
```

## ToolOutput

`ToolOutput` 结构体字段说明：

```rust
ToolOutput {
    id: None,                          // 自动生成
    summary: "Done".into(),            // 展示给用户的摘要
    raw: json!({"temp": 22}),          // 完整数据（可以很大）
    control_flow: ToolControlFlow::Continue,  // Continue 或 Break
    truncation: None,                  // 如果输出被截断，在此标记
}
```

## 最佳实践

1. **一个文件一个工具** — 保持工具实现聚焦、可测试
2. **校验参数** — 不要信任 LLM 提供的类型一定正确
3. **优雅处理错误** — 返回有意义的错误信息，让 LLM 能据此调整
4. **保持 `definition()` 准确** — 如果 LLM 的理解和实际行为不一致，工具调用会失败
5. **为长操作设置超时** — 对网络调用使用 `tokio::time::timeout`

## 完整示例

参见 [`examples/custom-tool.rs`](/examples/custom-tool.rs) 了解一个带计算器工具的完整可运行示例。

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, ChatMessage, LlmCapabilities, LlmClient,
    ResponseFormat, StreamChunk, SubAgentTool,
};
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::Value;

type ChunkStream = Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>;

struct MockLlmClient {
    responses: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
}

impl MockLlmClient {
    fn new(scripted: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            responses: Mutex::new(scripted.into_iter()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _enable_thinking: Option<bool>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _enable_thinking: Option<bool>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<ChunkStream> {
        let chunks: Vec<AgentResult<StreamChunk>> = self
            .responses
            .lock()
            .unwrap()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(Ok)
            .collect();
        let stream = futures_util::stream::iter(chunks);
        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== agent-base SubAgent Demo ===\n");

    println!("[1] 创建子 Agent (数据分析专家) ...");

    let sub_llm = Arc::new(MockLlmClient::new(vec![
            vec![
                StreamChunk::Text("数据分析结果：".to_string()),
                StreamChunk::Text("本月销售额 120 万，环比增长 15%，".to_string()),
                StreamChunk::Text("其中线上渠道占比 60%，线下渠道占比 40%。".to_string()),
                StreamChunk::Stop,
            ],
        ],
    ));

    let sub_runtime = AgentBuilder::new(sub_llm)
        .system_prompt("你是一个数据分析专家，根据用户提供的任务描述进行分析，返回详细结果。")
        .build();

    println!("    子 Agent 已就绪\n");

    println!("[2] 创建 SubAgentTool 包装子 Agent ...");
    let sub_agent_tool = SubAgentTool::new(
        "analyze_data",
        "将数据分析任务委托给专家子 Agent 执行，返回详细分析结果",
        sub_runtime,
    );
    println!("    工具名称: analyze_data\n");

    println!("[3] 创建父 Agent 并注册子 Agent 工具 ...");

    let parent_llm = Arc::new(MockLlmClient::new(vec![
            vec![
                StreamChunk::ToolCall(serde_json::json!({
                    "delta": {
                        "tool_calls": [{
                            "id": "call_1",
                            "function": {
                                "name": "analyze_data",
                                "arguments": "{\"task\": \"分析本月销售数据，重点关注线上线下渠道占比\"}"
                            }
                        }]
                    }
                })),
                StreamChunk::Stop,
            ],
            vec![
                StreamChunk::Text(
                    "根据子 Agent 的分析结果，本月销售表现良好。".to_string(),
                ),
                StreamChunk::Text("结论：建议继续加大线上渠道投入，同时优化线下门店布局。".to_string()),
                StreamChunk::Stop,
            ],
        ],
    ));

    let mut parent_runtime = AgentBuilder::new(parent_llm)
        .system_prompt("你是销售主管，负责汇总分析报告。你可以将具体分析任务委托给子 Agent。")
        .register_tool(sub_agent_tool)
        .build();

    let session_id = parent_runtime.create_session();

    println!("    父 Agent 已就绪\n");
    println!("--- 开始执行 ---\n");

    let (events, _outcome) = parent_runtime
        .run_turn_stream(session_id, "请分析一下本月的销售情况")
        .await?;

    println!();
    for event in &events {
        match event {
            AgentEvent::TextDelta { text, .. } => print!("{text}"),
            AgentEvent::ToolCallStarted { tool_name, args_json, .. } => {
                println!();
                println!(">>> 父 Agent 调用工具: {tool_name}");
                println!("    参数: {args_json}");
            }
            AgentEvent::ToolCallFinished { tool_name, summary, .. } => {
                println!();
                println!("<<< 工具返回 ({tool_name}):");
                println!("    {summary}");
            }
            AgentEvent::Custom { payload, .. } => {
                if let Some(event_type) = payload
                    .get("type")
                    .and_then(Value::as_str)
                {
                    if event_type == "subagent_event" {
                        if let Some(inner) = payload.get("event") {
                            if let Some(inner_type) = inner.get("type").and_then(Value::as_str)
                            {
                                match inner_type {
                                    "TextDelta" => {
                                        if let Some(t) = inner.get("text").and_then(Value::as_str)
                                        {
                                            print!("  [子Agent] {t}");
                                        }
                                    }
                                    "RunFinished" => {
                                        println!();
                                        println!("  [子Agent] 分析完成");
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            AgentEvent::RunFinished { .. } => {
                println!();
                println!();
                println!("--- 执行完成 ---");
            }
            _ => {}
        }
    }

    println!();
    println!("=== Demo 完成 ===");

    println!();
    println!("说明:");
    println!("  1. 父 Agent 收到用户请求后，决定将数据分析任务委托给子 Agent");
    println!("  2. 子 Agent 独立运行，产生分析结果");
    println!("  3. 子 Agent 的事件（TextDelta 等）通过 Custom 事件桥接到父 Agent");
    println!("  4. 父 Agent 收到子 Agent 结果后，汇总并给出最终结论");

    Ok(())
}

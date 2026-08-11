use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentBuilder, AgentResult, ChatMessage, LlmCapabilities, LlmClient, ResponseFormat,
    RuntimeEvent, StreamChunk, SubAgentTool,
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
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
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

    println!("[1] Creating sub-agent:  Agent (Data Analysis Expert) ...");

    let sub_mock = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::Text("Data analysis results：".to_string()),
        StreamChunk::Text("Monthly sales 120 10K，MoM growth 15%，".to_string()),
        StreamChunk::Text("Online channel share 60%，Offline channel share 40%。".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]]));
    let sub_llm = agent_base::llm::adapt(sub_mock);

    let sub_runtime = AgentBuilder::new(sub_llm)
        .system_prompt("You are a Data Analysis Expert. Analyze based on the task description provided. Return detailed results.")
        .build().unwrap();

    println!("    sub-agent Agent is ready\n");

    println!("[2] Creating SubAgentTool to wrap the sub-agent ...");
    let sub_agent_tool = SubAgentTool::new(
        "analyze_data",
        "Delegate data analysis tasks to an expert sub-agent. Execute and return detailed analysis results",
        sub_runtime,
    );
    println!("    tool name: analyze_data\n");

    println!("[3] Creating parent agent and registering sub-agent tool ...");

    let parent_mock = Arc::new(MockLlmClient::new(vec![
            vec![
                StreamChunk::ToolCall(serde_json::json!({
                    "delta": {
                        "tool_calls": [{
                            "id": "call_1",
                            "function": {
                                "name": "analyze_data",
                                "arguments": "{\"task\": \"Analyze this month's sales data, focusing on online vs offline channel share\"}"
                            }
                        }]
                    }
                })),
                StreamChunk::Stop { finish_reason: Some("stop".to_string()) },
            ],
            vec![
                StreamChunk::Text(
                    "Based on sub-agent analysis results, this month's sales performance is good.".to_string(),
                ),
                StreamChunk::Text("Conclusion：Recommend increasing online channel investment，while optimizing offline store layout。".to_string()),
                StreamChunk::Stop { finish_reason: Some("stop".to_string()) },
            ],
        ],
    ));
    let parent_llm = agent_base::llm::adapt(parent_mock);

    let parent_runtime = AgentBuilder::new(parent_llm)
        .system_prompt("You are a sales manager. Responsible for compiling analysis reports. You can delegate specific analysis tasks to the sub-agent.agent Agent。")
        .register_tool(sub_agent_tool)
        .build().unwrap();

    let session_id = parent_runtime.create_session().await;

    println!("    Parent agent Agent is ready\n");
    println!("--- Starting execution ---\n");

    let (events, _outcome) = parent_runtime
        .run_turn_collect(session_id, "Please analyze this month's sales performance")
        .await?;

    println!();
    for event in &events {
        match event {
            RuntimeEvent::TextDelta { text, .. } => print!("{text}"),
            RuntimeEvent::ToolCallStarted {
                tool_name,
                args_json,
                ..
            } => {
                println!();
                println!(">>> Parent agent calling tool: {tool_name}");
                println!("    with args: {args_json}");
            }
            RuntimeEvent::ToolCallFinished {
                tool_name, summary, ..
            } => {
                println!();
                println!("<<< Tool result ({tool_name}):");
                println!("    {summary}");
            }
            RuntimeEvent::UserEvent {
                event: agent_base::UserEvent::SubAgentEvent { subagent, event },
                ..
            } => {
                print!("  [sub-agent {subagent}] ");
                match event.as_ref() {
                    RuntimeEvent::TextDelta { text, .. } => {
                        print!("{text}");
                    }
                    RuntimeEvent::RunFinished { .. } => {
                        println!();
                        println!("  [Sub-agent] analysis done");
                    }
                    _ => {}
                }
            }
            RuntimeEvent::RunFinished { .. } => {
                println!();
                println!();
                println!("--- Executedone ---");
            }
            _ => {}
        }
    }

    println!();
    println!("=== Demo done ===");

    println!();
    println!("Note:");
    println!("  1. Parent agent receives user request and delegates analysis to the sub-agent");
    println!("  2. sub-agent Agent runs independently，generates analysis results");
    println!(
        "  3. Sub-agent events (TextDelta, etc.) bridge to the parent agent via Custom events"
    );
    println!(
        "  4. Parent agent receives sub-agent results, summarizes, and provides the final conclusion"
    );

    Ok(())
}

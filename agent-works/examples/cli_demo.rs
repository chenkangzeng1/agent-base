use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ResponseFormat,
    RuntimeEvent, SessionId, StreamChunk, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use agent_works::{
    AgentBuilder,
    cli::{CliEventPrinter, CliRepl},
};
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{Value, json};

type ChunkStream = Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>;

struct MockLlmClient {
    responses: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
}

impl MockLlmClient {
    fn new(responses: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
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
        Ok(Box::pin(futures_util::stream::iter(chunks)))
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

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "echo",
                "description": "Echo back the message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let msg = args["message"].as_str().unwrap_or("");
        Ok(ToolOutput {
            summary: format!("echo: {msg}"),
            raw: Some(json!({"echo": msg})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== agent-works CLI Demo ===\n");

    let llm = Arc::new(MockLlmClient::new(vec![]));

    let runtime = AgentBuilder::new(llm)
        .system_prompt("You are a helpful assistant.")
        .register_tool(EchoTool)
        .build()
        .unwrap();

    println!("[1] AgentRuntime created with AgentBuilder");
    println!("    - MockLlmClient (for demo purposes)");
    println!("    - EchoTool registered");
    println!();

    let mut repl = CliRepl::new(runtime);
    println!("[2] CliRepl created from AgentRuntime");
    println!();

    repl.register_shell_command(
        "time",
        Box::new(|_input: &str| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            println!(">>> Current Unix timestamp: {now}");
            true
        }),
    );
    println!("[3] Registered custom .time shell command");
    println!("    - Prints current Unix timestamp");
    println!();

    repl.register_shell_command(
        "hello",
        Box::new(|_input: &str| {
            println!(">>> Hello from custom command!");
            true
        }),
    );
    println!("[4] Registered custom .hello shell command");
    println!();

    let mut printer = CliEventPrinter::new();
    println!("[5] CliEventPrinter created");
    println!("    - Handles TextDelta, ThoughtDelta, ToolCallStarted, etc.");
    println!();

    println!("[6] Demo: CliEventPrinter handling events manually");
    printer.handle(RuntimeEvent::TextDelta {
        session_id: SessionId::new(0),
        text: "Hello, world!".to_string(),
    })?;
    println!();

    printer.handle(RuntimeEvent::ToolCallStarted {
        session_id: SessionId::new(0),
        tool_name: "echo".to_string(),
        args_json: r#"{"message": "hello"}"#.to_string(),
    })?;

    printer.handle(RuntimeEvent::ToolCallFinished {
        session_id: SessionId::new(0),
        tool_name: "echo".to_string(),
        summary: "echo: hello".to_string(),
    })?;
    println!();

    println!("=== Demo Complete ===");
    println!("(REPL loop not started - this is an API demonstration)");
    Ok(())
}

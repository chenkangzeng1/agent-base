use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, ChatMessage, LlmCapabilities, LlmClient,
    McpToolRegistry, ResponseFormat, StreamChunk,
};
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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

async fn start_mock_mcp_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}", addr);

    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }

                let request_str = String::from_utf8_lossy(&buf[..n]);
                let body_start = request_str.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                let body_str = &request_str[body_start..];

                let request: Value = serde_json::from_str(body_str).unwrap_or(Value::Null);
                let method = request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let result = match method {
                    "tools/list" => json!({
                        "tools": [
                            {
                                "name": "get_weather",
                                "description": "Get weather information for a specified city",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "city": {
                                            "type": "string",
                                            "description": "City name"
                                        }
                                    },
                                    "required": ["city"]
                                }
                            },
                            {
                                "name": "search_docs",
                                "description": "Search technical documentation",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": {
                                            "type": "string",
                                            "description": "Search keyword"
                                        }
                                    },
                                    "required": ["query"]
                                }
                            }
                        ]
                    }),
                    "tools/call" => {
                        let params = request.get("params").unwrap_or(&Value::Null);
                        let tool_name = params
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        let args = params.get("arguments").unwrap_or(&Value::Null);

                        if tool_name == "get_weather" {
                            let city = args.get("city").and_then(Value::as_str).unwrap_or("Beijing");
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("{city} Today's weather：Sunny，22°C ~ 30°C，Light breeze")
                                }]
                            })
                        } else {
                            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Searching \"{query}\" Results：Found 3 relevant documents")
                                }]
                            })
                        }
                    }
                    _ => json!({}),
                };

                let response_json = json!({
                    "jsonrpc": "2.0",
                    "id": request.get("id").unwrap_or(&json!(1)),
                    "result": result,
                });

                let response_body = serde_json::to_string(&response_json).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    response_body.len(),
                    response_body,
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    (server_url, handle)
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== agent-base MCP Demo ===\n");

    println!("[1] Starting mock MCP server MCP Server ...");
    let (server_url, _server_handle) = start_mock_mcp_server().await;
    println!("    MCP Server Running on: {server_url}\n");

    println!("[2] Connecting to MCP Server and discovering tools ...");
    let registry = McpToolRegistry::new(server_url.clone());
    let discovered_tools = registry.discover_tools().await?;
    println!("    Discovered {} tool:", discovered_tools.len());
    for tool in &discovered_tools {
        let def = tool.definition();
        let name = def
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let desc = def
            .get("function")
            .and_then(|f| f.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("");
        println!("    - {name}: {desc}");
    }

    println!("\n[3] Registering MCP tools into Agent and running ...\n");

    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Shenzhen\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("According to the query results, Shenzhen is sunny today, great for going out.".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    // Register MCP-discovered tools into AgentBuilder
    let mut builder = AgentBuilder::new(llm)
        .system_prompt("You are an assistant. You can use MCP tools to answer user questions.");
    for tool in discovered_tools {
        let name = tool.name().to_string();
        builder = builder.register_tool_arc(tool);
        println!("    Registeredtool: {name}");
    }
    let mut runtime = builder.build();

    let session_id = runtime.create_session();

    println!("\n--- Agent Running ---\n");
    let (events, _outcome) = runtime
        .run_turn_stream(session_id, "What is the weather in Shenzhen today?")
        .await?;

    for event in &events {
        match event {
            AgentEvent::TextDelta { text, .. } => print!("{text}"),
            AgentEvent::ToolCallStarted { tool_name, args_json, .. } => {
                println!("[Tool call] {tool_name}({args_json})");
            }
            AgentEvent::ToolCallFinished { summary, .. } => {
                println!("[toolResults] {summary}");
            }
            AgentEvent::RunFinished { .. } => println!("\n[Run finished]"),
            AgentEvent::Custom { payload, .. } => {
                println!("[Custom event] {payload}");
            }
            _ => {}
        }
    }

    println!("\n=== Demo done ===");
    Ok(())
}

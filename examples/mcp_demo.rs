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
                                "description": "获取指定城市的天气信息",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "city": {
                                            "type": "string",
                                            "description": "城市名称"
                                        }
                                    },
                                    "required": ["city"]
                                }
                            },
                            {
                                "name": "search_docs",
                                "description": "搜索技术文档",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": {
                                            "type": "string",
                                            "description": "搜索关键词"
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
                            let city = args.get("city").and_then(Value::as_str).unwrap_or("北京");
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("{city} 今日天气：晴，22°C ~ 30°C，微风")
                                }]
                            })
                        } else {
                            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("搜索 \"{query}\" 结果：找到 3 篇相关文档")
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

    println!("[1] 启动模拟 MCP Server ...");
    let (server_url, _server_handle) = start_mock_mcp_server().await;
    println!("    MCP Server 运行在: {server_url}\n");

    println!("[2] 连接 MCP Server 并发现工具 ...");
    let registry = McpToolRegistry::new(server_url.clone());
    let discovered_tools = registry.discover_tools().await?;
    println!("    发现 {} 个工具:", discovered_tools.len());
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

    println!("\n[3] 将 MCP 工具注册到 Agent 并运行 ...\n");

    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"深圳\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("根据查询结果，深圳今天天气晴朗，适合出行。".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    // Register MCP-discovered tools into AgentBuilder
    let mut builder = AgentBuilder::new(llm)
        .system_prompt("你是一个助手，可以使用 MCP 提供的工具来回答用户问题");
    for tool in discovered_tools {
        let name = tool.name().to_string();
        builder = builder.register_tool_arc(tool);
        println!("    已注册工具: {name}");
    }
    let mut runtime = builder.build();

    let session_id = runtime.create_session();

    println!("\n--- Agent 运行 ---\n");
    let (events, _outcome) = runtime
        .run_turn_stream(session_id, "深圳今天天气怎么样？")
        .await?;

    for event in &events {
        match event {
            AgentEvent::TextDelta { text, .. } => print!("{text}"),
            AgentEvent::ToolCallStarted { tool_name, args_json, .. } => {
                println!("[工具调用] {tool_name}({args_json})");
            }
            AgentEvent::ToolCallFinished { summary, .. } => {
                println!("[工具结果] {summary}");
            }
            AgentEvent::RunFinished { .. } => println!("\n[运行完成]"),
            AgentEvent::Custom { payload, .. } => {
                println!("[自定义事件] {payload}");
            }
            _ => {}
        }
    }

    println!("\n=== Demo 完成 ===");
    Ok(())
}

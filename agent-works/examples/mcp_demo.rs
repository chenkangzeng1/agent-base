use agent_works::mcp::{McpClient, McpHub, McpServerConfig, McpToolInfo, McpTransport};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn start_mock_mcp_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind TcpListener");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };

                let request = String::from_utf8_lossy(&buf[..n]);
                let body = match request.split("\r\n\r\n").nth(1) {
                    Some(b) => b.to_string(),
                    None => return,
                };

                let req: Value = match serde_json::from_str(&body) {
                    Ok(v) => v,
                    Err(_) => return,
                };

                let id = req.get("id").cloned().unwrap_or(Value::Null);
                let method = req.get("method").and_then(Value::as_str).unwrap_or("");

                let result = match method {
                    "tools/list" => json!({
                        "tools": [
                            {
                                "name": "get_weather",
                                "description": "Get current weather information for a city",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "city": {
                                            "type": "string",
                                            "description": "City name, e.g. Beijing, London"
                                        }
                                    },
                                    "required": ["city"]
                                }
                            },
                            {
                                "name": "search_docs",
                                "description": "Search technical documentation by keyword",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": {
                                            "type": "string",
                                            "description": "Search keyword or phrase"
                                        },
                                        "limit": {
                                            "type": "integer",
                                            "description": "Max number of results to return"
                                        }
                                    },
                                    "required": ["query"]
                                }
                            }
                        ]
                    }),
                    "tools/call" => {
                        let name = req
                            .pointer("/params/name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        let args = req.pointer("/params/arguments").unwrap_or(&Value::Null);

                        match name {
                            "get_weather" => {
                                let city = args
                                    .get("city")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown");
                                json!({
                                    "content": [
                                        {
                                            "type": "text",
                                            "text": format!(
                                                "Weather in {city}: sunny, 22°C, humidity 45%, wind 12km/h"
                                            )
                                        }
                                    ]
                                })
                            }
                            "search_docs" => {
                                let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                                json!({
                                    "content": [
                                        {
                                            "type": "text",
                                            "text": format!(
                                                "Search results for '{query}':\n\
                                                1. Getting Started Guide - Introduction and setup\n\
                                                2. API Reference - Complete API documentation\n\
                                                3. FAQ - Frequently asked questions"
                                            )
                                        }
                                    ]
                                })
                            }
                            _ => json!({
                                "content": [
                                    {
                                        "type": "text",
                                        "text": format!("Unknown tool: {name}")
                                    }
                                ]
                            }),
                        }
                    }
                    _ => Value::Null,
                };

                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });

                let body = serde_json::to_string(&response).unwrap();
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n\
                     {}",
                    body.len(),
                    body,
                );

                let _ = stream.write_all(http_response.as_bytes()).await;
            });
        }
    });

    port
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== agent-works MCP Demo ===\n");

    println!("[Step 1] Starting mock MCP server...");
    let port = start_mock_mcp_server().await;
    let server_url = format!("http://127.0.0.1:{port}");
    println!("  Mock server is running on {server_url}\n");

    println!("[Step 2] Creating McpTransport and McpServerConfig...");
    let transport = McpTransport::Http {
        url: server_url.clone(),
    };
    let config = McpServerConfig {
        name: "mock-server".to_string(),
        transport: transport.clone(),
        auto_reconnect: false,
    };
    println!(
        "  ServerConfig {{ name: {:?}, auto_reconnect: {} }}",
        config.name, config.auto_reconnect
    );
    println!("  Transport: Http {{ url: {server_url} }}\n");

    println!("[Step 3] Creating McpClient and listing tools...");
    let client = McpClient::new(transport).await?;
    let tools: Vec<McpToolInfo> = client.list_tools().await?;
    for tool in &tools {
        println!("  - {}: {}", tool.name, tool.description);
    }
    println!();

    println!("[Step 4] Calling tools via McpClient directly...");
    let weather = client
        .call_tool("get_weather", &json!({"city": "Beijing"}))
        .await?;
    println!("  get_weather(\"Beijing\") =>");
    if let Some(content) = weather.get("content") {
        for item in content.as_array().unwrap_or(&vec![]) {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                println!("    {text}");
            }
        }
    }

    let docs = client
        .call_tool("search_docs", &json!({"query": "rust async"}))
        .await?;
    println!("  search_docs(\"rust async\") =>");
    if let Some(content) = docs.get("content") {
        for item in content.as_array().unwrap_or(&vec![]) {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                for line in text.lines() {
                    println!("    {line}");
                }
            }
        }
    }
    println!();

    println!("[Step 5] Using McpHub: add_server + connect_all + discover_all...");
    let hub_transport = McpTransport::Http {
        url: server_url.clone(),
    };
    let hub_config = McpServerConfig {
        name: "mock-server-via-hub".to_string(),
        transport: hub_transport,
        auto_reconnect: false,
    };

    let mut hub = McpHub::new();
    hub.add_server(hub_config);
    println!("  Added 'mock-server-via-hub' to hub");

    hub.connect_all().await?;
    println!("  connect_all() succeeded");

    let discovered = hub.discover_all().await?;
    for (server_name, server_tools) in &discovered {
        println!(
            "  discover_all() => server '{server_name}' has {} tools:",
            server_tools.len()
        );
        for tool in server_tools {
            println!(
                "    - {} (schema keys: {:?})",
                tool.name,
                tool.input_schema
                    .as_object()
                    .map(|o| o.keys().collect::<Vec<_>>())
            );
        }
    }
    println!();

    println!("[Step 6] Registering tools into ToolRegistry via hub.register_all()...");
    use agent_base::ToolRegistry;
    let mut registry = ToolRegistry::default();
    hub.register_all(&mut registry);
    println!("  ToolRegistry now has {} tool(s)", registry.len());
    for def in registry.definitions() {
        if let Some(name) = def.pointer("/function/name").and_then(Value::as_str) {
            let desc = def
                .pointer("/function/description")
                .and_then(Value::as_str)
                .unwrap_or("");
            println!("    - {name}: {desc}");
        }
    }
    println!();

    println!("[Step 7] Health check...");
    hub.health_check().await?;
    println!("  health_check() passed — all servers responsive\n");

    println!("=== Demo Complete ===");
    Ok(())
}

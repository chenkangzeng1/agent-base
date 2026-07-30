use agent_base::{AgentError, AgentResult, Language, SessionId, Tool, ToolContext};
use agent_works::builtin::{
    FileExistsTool, ListDirectoryTool, ReadFileTool, SearchReplaceTool, WriteFileTool,
};
use serde_json::json;

fn make_tool_context() -> ToolContext {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    ToolContext {
        session_id: SessionId::new(1),
        user_event_tx: tx,
        llm_client: None,
        session_store: None,
        language: Language::En,
        cancel_token: tokio_util::sync::CancellationToken::new(),
    }
}

fn print_definition(tool: &dyn Tool) {
    let def = tool.definition();
    let desc = def["function"]["description"].as_str().unwrap_or("N/A");
    println!("  Tool: {}", tool.name());
    println!("    Description: {desc}");
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== agent-works Builtin Tools Demo ===\n");

    let tmp = tempfile::tempdir()
        .map_err(|e| AgentError::internal(format!("failed to create temp dir: {e}")))?;
    let workspace = tmp.path().to_path_buf();
    println!("[0] Temp workspace: {}\n", workspace.display());

    let read_tool = ReadFileTool {
        workspace: workspace.clone(),
    };
    let write_tool = WriteFileTool {
        workspace: workspace.clone(),
    };
    let list_tool = ListDirectoryTool {
        workspace: workspace.clone(),
    };
    let exists_tool = FileExistsTool {
        workspace: workspace.clone(),
    };
    let search_replace_tool = SearchReplaceTool {
        workspace: workspace.clone(),
    };

    println!("[1] Tool definitions:");
    print_definition(&read_tool);
    print_definition(&write_tool);
    print_definition(&list_tool);
    print_definition(&exists_tool);
    print_definition(&search_replace_tool);
    println!();

    let ctx = make_tool_context();

    let test_file = "hello.txt";
    let test_content = "Hello, World!\nThis is a test file.\nLine three.";

    let result = write_tool
        .call(&json!({"path": test_file, "content": test_content}), &ctx)
        .await?;
    println!("[2] WriteFileTool -> {}: {}", test_file, result.summary);

    let result = exists_tool.call(&json!({"path": test_file}), &ctx).await?;
    println!("[3] FileExistsTool -> {}: {}", test_file, result.summary);

    let result = read_tool.call(&json!({"path": test_file}), &ctx).await?;
    println!("[4] ReadFileTool -> {}:", test_file);
    for line in result.summary.lines() {
        println!("    {line}");
    }

    let sub_dir = "sub";
    let sub_file = "sub/nested.txt";
    let _ = write_tool
        .call(
            &json!({"path": sub_file, "content": "nested content"}),
            &ctx,
        )
        .await?;

    let result = list_tool.call(&json!({"path": "."}), &ctx).await?;
    println!("[5] ListDirectoryTool -> root:");
    for line in result.summary.lines() {
        println!("    {line}");
    }

    let result = list_tool.call(&json!({"path": sub_dir}), &ctx).await?;
    println!("[6] ListDirectoryTool -> {sub_dir}:");
    for line in result.summary.lines() {
        println!("    {line}");
    }

    let result = search_replace_tool
        .call(
            &json!({
                "path": test_file,
                "old_str": "World",
                "new_str": "Rustacean"
            }),
            &ctx,
        )
        .await?;
    println!("[7] SearchReplaceTool -> {}: {}", test_file, result.summary);

    let result = read_tool.call(&json!({"path": test_file}), &ctx).await?;
    println!("[8] Verified replacement in {}:", test_file);
    for line in result.summary.lines() {
        println!("    {line}");
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}

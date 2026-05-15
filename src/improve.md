# agent-core 改进 Plan

> 目标：将 `agent-core` 打造为通用智能体核心库

## Phase 6: 高级特性（按需推进）

### 6.1 MCP（Model Context Protocol）支持
- 新建 `src/tool/mcp.rs`
- `McpToolRegistry` 实现 `Tool` trait，自动发现 MCP server 的能力

### 6.2 多模态消息
- `ChatMessage::User` 增加 `images: Vec<ImageAttachment>`（base64 / URL）
- `OpenAiClient` 构造 `content` 为 `[{"type": "text", ...}, {"type": "image_url", ...}]` 数组格式

### 6.3 子 Agent / 委托
- `Tool` 实现可以嵌套 `AgentRuntime`，实现"分派任务给子 agent"模式
- 依赖 Phase 4.3 的 `ToolContext` 增强

### 6.4 Human-in-the-Loop 恢复
- 支持 `AgentEvent::Checkpoint { session_id, checkpoint_data }` 实现暂停/恢复

---

## 排序建议（按优先级）

| 批次 | 阶段 | 说明 |
|------|------|------|
| 第 1 批 | Phase 6 | 高级特性 |

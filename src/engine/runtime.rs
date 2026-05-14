use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::llm::LlmClient;
use crate::tool::{ToolContext, ToolOutput, ToolControlFlow, ToolPolicy, ToolRegistry};
use crate::types::{AgentConfig, ChatMessage, MessageRole, AgentError};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{AgentResult, AgentEvent, SessionId};
use super::approval::ApprovalHandler;
use super::AgentSession;

const DEFAULT_MAX_TURNS: u32 = 50;

pub struct AgentRuntime {
    pub(crate) client: Arc<dyn LlmClient>,
    pub(crate) config: AgentConfig,
    pub(crate) tools: ToolRegistry,
    pub(crate) approval_handler: Option<Arc<dyn ApprovalHandler>>,
    pub(crate) tool_policy: Option<Arc<dyn ToolPolicy>>,
    pub(crate) event_bus: broadcast::Sender<AgentEvent>,
    pub(crate) next_session_id: AtomicU64,
    pub(crate) sessions: HashMap<SessionId, AgentSession>,
}

struct StreamAggregator {
    is_tool_call: bool,
    full_text: String,
    tool_call_id: String,
    tool_name: String,
    tool_args_json: String,
}

impl StreamAggregator {
    fn new() -> Self {
        Self {
            is_tool_call: false,
            full_text: String::new(),
            tool_call_id: String::new(),
            tool_name: String::new(),
            tool_args_json: String::new(),
        }
    }

    fn into_parts(self) -> (String, bool, String, String, String) {
        (
            self.full_text,
            self.is_tool_call,
            self.tool_call_id,
            self.tool_name,
            self.tool_args_json,
        )
    }
}

impl AgentRuntime {
    pub fn create_session(&mut self) -> SessionId {
        let id = SessionId(self.next_session_id.fetch_add(1, Ordering::Relaxed));
        let mut session = AgentSession::new(id);
        if let Some(system_prompt) = self.config.system_prompt.as_deref() {
            session.push_message(MessageRole::System, system_prompt);
        }
        self.sessions.insert(id, session);
        id
    }

    pub fn session(&self, session_id: SessionId) -> Option<&AgentSession> {
        self.sessions.get(&session_id)
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn client(&self) -> &Arc<dyn LlmClient> {
        &self.client
    }

    pub fn approval_handler(&self) -> Option<&Arc<dyn ApprovalHandler>> {
        self.approval_handler.as_ref()
    }

    pub fn tool_policy(&self) -> Option<&Arc<dyn ToolPolicy>> {
        self.tool_policy.as_ref()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_bus.subscribe()
    }

    fn cached_approval(&self, session_id: SessionId, action_key: &str) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| session.is_action_allowed(action_key))
    }

    fn cache_approval(&mut self, session_id: SessionId, action_key: String) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.allow_action(action_key);
        }
    }

    fn emit_event(&self, event: AgentEvent) {
        let _ = self.event_bus.send(event);
    }

    fn session_or_err(&self, session_id: SessionId) -> AgentResult<&AgentSession> {
        self.sessions
            .get(&session_id)
            .ok_or_else(|| AgentError::session_not_found(session_id.0))
    }

    fn session_mut_or_err(&mut self, session_id: SessionId) -> AgentResult<&mut AgentSession> {
        self.sessions
            .get_mut(&session_id)
            .ok_or_else(|| AgentError::session_not_found(session_id.0))
    }

    fn drain_async_events<F>(
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        loop {
            match event_rx.try_recv() {
                Ok(event) => on_event(event)?,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }

    /// 单次用户输入的编排入口。
    ///
    /// 编排循环的职责是"决策 + 委托"：决定什么时候调用 LLM、什么时候执行工具、
    /// 什么时候重试、什么时候结束，但不关心具体怎么做。
    ///
    /// # 控制流
    ///
    /// ```text
    /// 用户输入 → 推入 session
    ///   ↓
    /// loop {
    ///   检查 max_turns（防止死循环）
    ///   execute_llm_turn() → StreamAggregator { text, tool_call? }
    ///   ├─ 空响应（text 为空且无 tool_call）→ continue（让 LLM 再试）
    ///   ├─ 纯文本                         → 写入 session → RunCompleted → break
    ///   └─ 有工具调用                     → handle_tool_call()
    ///        ├─ Continue → continue（工具执行完继续让 LLM 思考）
    ///        ├─ Break    → break（工具要求终止）
    ///        └─ Err      → 注入恢复消息 → continue（让 LLM 纠正错误）
    /// }
    /// ```
    ///
    /// # 错误恢复策略
    ///
    /// 工具调用失败时，编排器会向 session 注入 Assistant + User 两条恢复消息，
    /// 通知 LLM 出错了并请它简化计划重试。这样 LLM 可以自行纠正，不需要外部干预。
    /// 只有 `AgentError::Cancelled` 是不可恢复的——它会被透传出去终止整个流程。
    pub async fn run_turn_with_handler<F>(
        &mut self,
        session_id: SessionId,
        user_input: &str,
        mut on_event: F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tools.definitions();

        // 将用户输入推入 session，成为对话上下文的组成部分
        {
            let session = self.session_mut_or_err(session_id)?;
            session.push_message(MessageRole::User, user_input);
        }

        let max_turns = self.config.max_turns.unwrap_or(DEFAULT_MAX_TURNS);
        let mut turn_count: u32 = 0;

        loop {
            turn_count += 1;

            // 防止 LLM 陷入工具调用死循环
            if turn_count > max_turns {
                self.emit_event(AgentEvent::RunFailed {
                    session_id,
                    error: format!("达到最大轮次限制（{max_turns}次），强制停止"),
                });
                Self::drain_async_events(&mut event_rx, &mut on_event)?;
                break;
            }

            // 每轮开始前清空积压的外部事件（如取消信号）
            Self::drain_async_events(&mut event_rx, &mut on_event)?;

            // 每轮重新获取完整对话历史，确保包含上一轮新增的消息
            let messages: Vec<_> = self.session_or_err(session_id)?.chat_messages().to_vec();

            // 委托：调用 LLM 并消费流式响应
            let aggregator = self
                .execute_llm_turn(session_id, &messages, &tool_definitions, &mut event_rx, &mut on_event)
                .await?;

            let (full_text, is_tool_call, tool_call_id, tool_name, tool_args_json) =
                aggregator.into_parts();

            // 空响应：LLM 既没输出文本也没调用工具 → 重试
            if full_text.is_empty() && !is_tool_call {
                continue;
            }

            // 有文本回复 → 写入会话历史
            if !full_text.is_empty() {
                let session = self.session_mut_or_err(session_id)?;
                session.push_message(MessageRole::Assistant, full_text);
            }

            // 有工具调用 → 委托 handle_tool_call 完成审批 + 执行 + 回写
            if is_tool_call && !tool_name.is_empty() {
                match self
                    .handle_tool_call(
                        session_id,
                        &tool_call_id,
                        &tool_name,
                        &tool_args_json,
                        &mut event_rx,
                        &mut on_event,
                    )
                    .await
                {
                    // Continue: 工具要求 LLM 继续思考（例如获取了关键数据后，让 LLM 基于结果回答）
                    Ok(ToolCallResult::Continue) => continue,
                    // Break: 工具认为任务已达成，停止对话
                    Ok(ToolCallResult::Break) => break,
                    Err(e) => {
                        // Cancelled 是不可恢复的，直接透传出去
                        if e.is_cancelled() {
                            return Err(e);
                        }
                        // 其他错误（审批拒绝、执行失败等）：注入恢复消息，让 LLM 重试
                        let session = self.session_mut_or_err(session_id)?;
                        session.push_message(
                            MessageRole::Assistant,
                            format!("(尝试调用工具 {tool_name} 失败)"),
                        );
                        session.push_message(
                            MessageRole::User,
                            "你刚才尝试调用工具时出现了错误。请简化你的计划，然后重新调用工具。",
                        );
                        continue;
                    }
                }
            }

            // 正常结束：文本回复完成且无后续工具调用
            self.emit_event(AgentEvent::RunCompleted { session_id });
            Self::drain_async_events(&mut event_rx, &mut on_event)?;
            break;
        }

        Ok(())
    }

    /// 调用 LLM 并消费其流式响应，将结果聚合为 `StreamAggregator`。
    ///
    /// 这是与 LLM 交互的唯一入口。它发起 `chat_stream` 请求，然后在一个
    /// `tokio::select!` 循环中同时处理两件事：
    ///
    /// - **SSE 流消费**：逐帧接收 chunk，按类型分发
    ///   - `Text`: 累积文本内容，实时发射 `TextDelta` 事件
    ///   - `Thought`: 仅当 `enable_thought` 配置开启时发射 `ThoughtDelta`
    ///   - `ToolCall`: 解析 OpenAI 格式的 `delta.tool_calls`，提取 id/name/arguments
    ///   - `Stop`: 流结束
    /// - **外部事件监听**：通过 `event_rx.recv()` 接收取消信号等外部事件，
    ///   确保 LLM 调用期间不会被阻塞住
    ///
    /// # `tokio::select!` 为什么不能拆
    ///
    /// 如果把这一个循环拆成"流消费"和"事件监听"两个独立函数，`select!`
    /// 的并发语义就会丢失——你必须用 `tokio::spawn` 代替，引入不必要的
    /// 任务调度开销。保持在一个 `select!` 中是当前规模下最简洁的方案。
    ///
    /// # 关于 ToolCall chunk 的解析
    ///
    /// OpenAI 的流式工具调用通过多帧 delta 增量传递，arguments 可能分多次到达，
    /// 因此这里用 `push_str` 拼接而非直接赋值。`is_tool_call` 标记一旦置为 true
    /// 就不会回头处理文本 chunk，避免 text 和 tool_call 混合污染。
    async fn execute_llm_turn<F>(
        &self,
        session_id: SessionId,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<StreamAggregator>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let mut stream = self
            .client
            .chat_stream(messages, tool_definitions, self.config.enable_thinking)
            .await?;

        let mut aggregator = StreamAggregator::new();

        loop {
            tokio::select! {
                // 分支 1：处理外部事件（取消信号、审批响应等）
                // 这是非阻塞的分发——如果有外部事件积压就先处理
                recv_result = event_rx.recv() => {
                    match recv_result {
                        Ok(event) => on_event(event)?,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                // 分支 2：等待下一个 SSE chunk
                maybe_chunk = stream.next() => {
                    let Some(chunk_result) = maybe_chunk else {
                        break; // 流自然结束
                    };
                    let chunk = chunk_result?;
                    match chunk {
                        crate::llm::StreamChunk::Text(text) => {
                            // 只在非 tool_call 模式下累积文本（避免 tool_call 和 text 混合）
                            if !text.is_empty() && !aggregator.is_tool_call {
                                aggregator.full_text.push_str(&text);
                                self.emit_event(AgentEvent::TextDelta { session_id, text });
                            }
                        }
                        crate::llm::StreamChunk::Thought(text) => {
                            if !text.is_empty() && !aggregator.is_tool_call && self.config.enable_thought {
                                self.emit_event(AgentEvent::ThoughtDelta { session_id, text });
                            }
                        }
                        crate::llm::StreamChunk::ToolCall(choice) => {
                            // 标记进入工具调用模式，后续 chunk 不再作为纯文本处理
                            aggregator.is_tool_call = true;
                            // 解析 OpenAI 标准 delta.tool_calls 结构
                            if let Some(tool_calls) = choice
                                .get("delta")
                                .and_then(|d| d.get("tool_calls"))
                                .and_then(Value::as_array)
                            {
                                for tool_call in tool_calls {
                                    if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                                        aggregator.tool_call_id = id.to_string();
                                    }
                                    if let Some(func) = tool_call.get("function") {
                                        if let Some(name) = func.get("name").and_then(Value::as_str) {
                                            aggregator.tool_name = name.to_string();
                                        }
                                        if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                                            aggregator.tool_args_json.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                        crate::llm::StreamChunk::Stop => break,
                    }
                    // 每处理一个 chunk 后清空外部事件，避免积压
                    Self::drain_async_events(event_rx, on_event)?;
                }
            }
        }

        Ok(aggregator)
    }

    /// 工具调用的完整管道：参数解析 → 审批 → 执行 → 事件发射 → 结果回写。
    ///
    /// # 执行步骤
    ///
    /// ```text
    /// 1. 解析 args_json → Value           （JSON 反序列化，错误即 ToolArgsInvalid）
    /// 2. process_approval()               （需要审批时阻断等待决策）
    /// 3. ToolPolicy::on_pre_call()         （策略前置钩子，如日志/限流）
    /// 4. emit ToolCallStarted              （通知外部工具开始执行）
    /// 5. session.push_assistant_tool_call()（写入 assistant 消息，tool_calls 格式）
    /// 6. tool.call(&args, &ctx)           （真正执行工具，工具不存在时返回摘要）
    /// 7. ToolPolicy::on_post_call()        （策略后置钩子）
    /// 8. emit ToolCallFinished              （通知外部工具执行完毕）
    /// 9. session.push_tool_result()        （写入 tool 消息，含执行摘要）
    /// 10. 返回 Continue / Break             （控制编排循环的后续行为）
    /// ```
    ///
    /// # 工具不存在
    ///
    /// 如果 LLM 请求了未注册的工具，不会报错——而是返回一个摘要为
    /// `"Tool xxx not found"` 的虚拟 `ToolOutput`，让编排器将"工具不存在"
    /// 的事实反馈给 LLM，LLM 可以自行调整。
    ///
    /// # ToolControlFlow 的语义
    ///
    /// - `Continue`: 工具执行完毕，应让 LLM 基于结果继续思考/回答
    /// - `Break`: 工具认为对话已完成（例如文件已保存），编排器终止循环
    async fn handle_tool_call<F>(
        &mut self,
        session_id: SessionId,
        tool_call_id: &str,
        tool_name: &str,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<ToolCallResult>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        // Step 1: 解析工具参数 JSON
        let args: Value =
            serde_json::from_str(tool_args_json).map_err(|_| AgentError::ToolArgsInvalid {
                name: tool_name.to_string(),
                raw: tool_args_json.to_string(),
            })?;

        // Step 2: 审批检查（需要审批时会阻断等待）
        self.process_approval(session_id, tool_name, &args, tool_args_json, event_rx, on_event)
            .await?;

        // 构建工具执行上下文
        let tool_ctx = ToolContext {
            session_id,
            event_bus: self.event_bus.clone(),
        };

        // Step 3: 策略前置钩子（不阻塞执行，用于日志/审计/限流等旁路逻辑）
        if let Some(policy) = self.tool_policy.as_ref() {
            policy.on_pre_call(tool_name, &args, &tool_ctx);
        }

        // Step 4: 发射工具调用开始事件
        self.emit_event(AgentEvent::ToolCallStarted {
            session_id,
            tool_name: tool_name.to_string(),
            args_json: tool_args_json.to_string(),
        });
        Self::drain_async_events(event_rx, on_event)?;

        // Step 5: 将 assistant(tool_calls) 消息写入会话历史
        {
            let session = self.session_mut_or_err(session_id)?;
            session.push_assistant_tool_call(tool_call_id, tool_name, tool_args_json);
        }

        // Step 6: 执行工具——查找注册表，找到就调用，找不到返回摘要
        let tool_result = if let Some(tool) = self.tools.get(tool_name) {
            tool.call(&args, &tool_ctx)
                .await
                .map_err(|e| AgentError::ToolExecution {
                    name: tool_name.to_string(),
                    source: Box::new(e),
                })?
        } else {
            ToolOutput {
                summary: format!("Tool {} not found", tool_name),
                raw: None,
                control_flow: ToolControlFlow::Break,
            }
        };

        // Step 7: 策略后置钩子
        if let Some(policy) = self.tool_policy.as_ref() {
            policy.on_post_call(tool_name, &args, &tool_result, &tool_ctx);
        }

        // Step 8: 发射工具调用完成事件
        self.emit_event(AgentEvent::ToolCallFinished {
            session_id,
            tool_name: tool_name.to_string(),
            summary: tool_result.summary.clone(),
        });
        Self::drain_async_events(event_rx, on_event)?;

        // Step 9: 将工具执行结果写入会话历史
        {
            let session = self.session_mut_or_err(session_id)?;
            session.push_tool_result(tool_call_id, &tool_result.summary);
        }

        // Step 10: 将工具层面的 control_flow 映射为编排器层面的 Continue/Break
        match tool_result.control_flow {
            ToolControlFlow::Continue => Ok(ToolCallResult::Continue),
            ToolControlFlow::Break => Ok(ToolCallResult::Break),
        }
    }

    /// 工具调用的审批流程。
    ///
    /// # 决策树
    ///
    /// ```text
    /// ToolPolicy::evaluate_approval() → 需要审批？
    ///   ├─ 不需要 → Ok(())  直接放行
    ///   └─ 需要
    ///        ├─ 已缓存 AllowAlways → Ok(())  跳过审批
    ///        └─ 未缓存
    ///             ├─ 发射 AwaitingApproval 事件
    ///             ├─ 调用 ApprovalHandler::approve()
    ///             └─ 根据决策：
    ///                  ├─ AllowOnce   → Ok(())  仅本次放行
    ///                  ├─ AllowAlways → 缓存 action_key → Ok(())  永久放行
    ///                  └─ Deny       → 写入拒绝摘要到 session
    ///                                 → 发射 ToolCallFinished
    ///                                 → 返回 ApprovalDenied 错误
    /// ```
    ///
    /// # Deny 的处理
    ///
    /// 拒绝时仍会向 session 写入 assistant+tool 消息对，让 LLM 在后续轮次中
    /// 看到"请求被拒绝"的事实。返回 `ApprovalDenied` 错误由编排器捕获，
    /// 编排器注入恢复消息后继续循环，LLM 可以调整策略重新尝试。
    async fn process_approval<F>(
        &mut self,
        session_id: SessionId,
        tool_name: &str,
        args: &Value,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        // Step 1: 询问 ToolPolicy 是否需要审批
        let approval_request = self.tool_policy.as_ref().and_then(|policy| {
            policy.evaluate_approval(tool_name, args, tool_args_json)
        });

        // 无需审批 → 直接放行
        let Some(request) = approval_request else {
            return Ok(());
        };

        // Step 2: 检查是否已有 AllowAlways 缓存（per-session 维度）
        let approved = request
            .action_key
            .as_deref()
            .is_some_and(|key| self.cached_approval(session_id, key));

        if approved {
            return Ok(());
        }

        // Step 3: 发射 AwaitingApproval 事件，等待外部决策
        self.emit_event(AgentEvent::AwaitingApproval {
            session_id,
            request: request.clone(),
        });
        Self::drain_async_events(event_rx, on_event)?;

        // Step 4: 调用 ApprovalHandler 获取决策
        // 如果没有注册 handler，默认拒绝
        let decision = match self.approval_handler() {
            Some(handler) => handler
                .approve(request.clone())
                .await
                .map_err(|e| AgentError::internal(format!("审批处理失败: {e}")))?,
            None => crate::types::ApprovalDecision::Deny,
        };

        // Step 5: 根据决策行事
        match decision {
            crate::types::ApprovalDecision::AllowOnce => {
                // 仅本次放行，不缓存
            }
            crate::types::ApprovalDecision::AllowAlways => {
                // 写入缓存，当前 session 内后续同 action_key 的调用自动通过
                if let Some(action_key) = request.action_key.clone() {
                    self.cache_approval(session_id, action_key);
                }
            }
            crate::types::ApprovalDecision::Deny => {
                // 将拒绝结果写入 session，让 LLM 知道请求被拒绝了
                let denial_summary =
                    format!("[Action Denied]: 审批拒绝执行工具 {tool_name}。");
                let session = self.session_mut_or_err(session_id)?;
                session.push_assistant_tool_call("", tool_name, tool_args_json);
                session.push_tool_result("", denial_summary.clone());
                self.emit_event(AgentEvent::ToolCallFinished {
                    session_id,
                    tool_name: tool_name.to_string(),
                    summary: denial_summary,
                });
                Self::drain_async_events(event_rx, on_event)?;
                return Err(AgentError::ApprovalDenied {
                    tool_name: tool_name.to_string(),
                });
            }
        }

        Ok(())
    }

    pub async fn run_turn_stream(
        &mut self,
        session_id: SessionId,
        user_input: &str,
    ) -> AgentResult<Vec<AgentEvent>> {
        let mut events = Vec::new();
        self.run_turn_with_handler(session_id, user_input, |event| {
            events.push(event);
            Ok(())
        })
        .await?;
        Ok(events)
    }
}

enum ToolCallResult {
    Continue,
    Break,
}

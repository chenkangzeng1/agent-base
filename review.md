# agent-core review

## 总体结论

`agent-core` 目前已经具备通用 Agent Runtime 的核心骨架，方向是正确的。作为后续衍生 `ops-agent`、`db-agent`、`browser-agent`、`code-agent` 等定制 Agent 的底座，这个工程是成立的。

当前阶段更接近“能跑通的一版通用 Agent 框架”，而不是“可长期稳定承载多种 Agent 的成熟内核”。

整体判断：

- 架构方向正确，抽象层次基本合理
- 主循环清晰，具备多轮、工具、审批、事件流能力
- 具备平台化潜力
- 但运行时语义、错误恢复、状态持久化、可扩展控制流等方面还需要继续收紧

## 当前设计的优点

### 1. 核心抽象边界比较清晰

目前已经拆出了几个关键抽象：

- `LlmClient`
- `Tool`
- `ToolPolicy`
- `ApprovalHandler`
- `Middleware`
- `Skill`
- `SessionStore`

这些边界符合一个通用 Agent Runtime 的基本形态，说明项目目标不是 demo，而是朝底层框架在设计。

### 2. Runtime 主循环完整

`AgentRuntime::run_turn_with_handler` 已经覆盖了一个通用 Agent 执行周期中的关键步骤：

- 用户输入进入 session
- middleware 前处理
- LLM 调用
- ToolCall 处理
- 审批流
- 多轮 continuation
- checkpoint / event 外抛
- session save

这条链路作为运行时主流程是成立的，后续演进空间也比较大。

### 3. 事件模型有平台化价值

`AgentEvent` 这层设计是加分项。未来无论是：

- UI 展示
- 日志采集
- 审计
- 回放
- 调试器
- 运行时可观测性

都可以围绕事件流继续扩展。

### 4. ToolPolicy / ApprovalHandler 分层合理

工具是否需要审批、审批如何执行，被拆成了两个层：

- `ToolPolicy`：负责判定是否需要审批
- `ApprovalHandler`：负责执行审批决策

这种设计适合未来接入 CLI 审批、Web 审批、自动审批、RBAC 审批等不同场景。

### 5. Skill 的方向是对的

Skill 不是简单 prompt 片段，而是能力包，包含：

- brief description
- detailed description
- 自带 tool 集合

这是比“纯 prompt 注入”更适合平台化演进的建模方式。

## 主要问题与不足

### 1. RunCompleted / RunFailed 的运行语义不够稳定

这是目前最核心的问题之一。

当前 `run_turn_with_handler` 里：

- 普通文本回复结束时会触发 `RunCompleted`
- 工具执行返回 `Break` 也会触发 `RunCompleted`
- 超过最大轮次时会 emit `RunFailed`
- 但函数本身不一定返回 `Err`

这会导致上层使用者对“本轮是成功完成、异常收敛、还是失败结束”产生歧义。

当前更像是：

- `RunCompleted` = 运行结束了
- 而不是 `RunCompleted` = 成功完成了

这对未来上层 Agent 编排、UI、监控、重试策略都会造成影响。

### 2. 返回值语义与事件语义存在分裂

运行结果目前同时存在两套语义：

- 函数返回值 `AgentResult<()>`
- 事件流中的 `RunCompleted / RunFailed`

但两者没有完全一致。

例如超过最大轮次时，事件里是失败，但函数仍可能返回 `Ok(())`。这种设计会让上层调用方很难统一处理。

### 3. ToolControlFlow 过于粗粒度

当前只有：

- `Break`
- `Continue`

对于简单 ReAct loop 够用，但如果未来要承载：

- planner / executor agent
- browser agent
- code agent
- verifier agent
- approval-heavy workflow

这个控制流表达力会不足。

真实场景里往往还需要表达：

- 重新规划
- 请求人工确认
- 终止运行
- 强制进入下一轮 LLM
- 跳过部分回灌

因此 `Break / Continue` 更像初版控制流，而不是最终形态。

### 4. Tool 执行失败后的恢复策略写死在 runtime 中

当前当工具调用失败且不是 cancel 时，runtime 会自动向 session 追加一段 assistant/user 消息，引导模型重新规划。

这个策略在 demo 阶段是友好的，但作为通用内核存在问题：

- ops-agent 失败后可能应该直接停止
- db-agent 失败后可能需要确认或回滚
- code-agent 可以适度自恢复
- browser-agent 则可能适合自动重试

也就是说，工具失败后的默认恢复策略不应该被固定在 runtime 主循环里，后续应考虑做成策略化或 hook 化。

### 5. SessionStore 目前更像“预留接口”，还不是真正的状态主路径

当前 runtime 内部有自己的内存态 `sessions: HashMap<SessionId, AgentSession>`，同时又暴露了 `SessionStore`。

但目前 `SessionStore` 更多是：

- 结束时 save
- 接口上支持 load/list/delete

而不是 runtime 真正依赖的主状态来源。

这会带来几个问题：

- 内存 session 与持久化 session 存在双状态
- 会话恢复能力未真正打通
- 不适合未来多实例或服务化
- 状态一致性模型不清晰

如果未来这个项目要进一步平台化，需要尽早决定：

- runtime 是纯内存执行器
- 还是持久状态驱动的运行器

现在处在中间态。

### 6. Checkpoint 设计到了，但没有完全闭环

`CheckpointStep::AfterToolCalls` 已经包含 `results: Vec<ToolResultData>`，说明设计时考虑过工具结果快照。

但当前 runtime emit `AfterToolCalls` 时，`results` 实际上传的是空数组。

这会导致 checkpoint 更像“半成品抽象”，对未来的：

- 回放
- 调试
- 审计
- 故障恢复
- 运行过程可视化

支持不完整。

### 7. Skill 自动注入机制存在小的边界问题

#### 7.1 `skill_detail_tool_name` 的冲突没有完整校验

当前只校验了 skill 自带 tools 与已有工具的名字冲突，但没有完整校验自动注入的 `get_skill_detail` 工具名是否与已有工具重复。

#### 7.2 使用 `Box::leak` 生成 `'static` 名称

在 `SkillDetailTool` 和 MCP tool adapter 中，都通过 `Box::leak` 把动态字符串转成 `'static`。

短生命周期 CLI 场景问题不大，但作为长期演进的通用内核，不太建议把这种方式作为常态实现。

### 8. LLM 抽象已经有了，但 provider 差异仍然偏“适配层兼容”

`LlmClient` trait 本身没有问题，但当前 OpenAI / Anthropic 的兼容更多是通过 adapter 转换字段完成。

这是正常的第一步，但未来问题会逐渐显现：

- tool call schema 差异
- reasoning / thinking 差异
- response_format 支持差异
- vision 支持差异
- token usage 语义差异
- 错误模型差异

如果后续 provider 增多，当前适配方式会越来越难统一管理。

### 9. `LlmCapabilities` 已定义，但尚未真正驱动运行策略

现在 capability 已经有：

- supports_streaming
- supports_tools
- supports_vision
- supports_thinking
- context / output 上限

但 runtime 还没有显式利用这些能力信息做策略控制。

例如：

- 不支持 thinking 时是否应自动忽略
- 不支持 tools 时是否应报错
- 不支持 response_format 时是否应 fallback

这些目前还没有形成统一行为。

### 10. ToolOutput 建模偏轻，不利于复杂 Agent

当前 `ToolOutput` 主要是：

- `summary: String`
- `raw: Option<Value>`
- `control_flow`
- `truncated`

问题在于，对 LLM 真正回灌的是 `summary`。这意味着工具结果的主语义仍依赖自然语言文本，而不是结构化结果。

对于简单场景可以接受，但未来如果要做：

- planner / executor
- verifier
- structured post-processing
- 复杂多步工具链

会逐渐暴露出问题。

后续可能需要区分：

- 给 LLM 的文本内容
- 给系统/调用方的结构化 payload
- 给 UI 的展示层 payload

### 11. SubAgentTool 的 session 语义需要明确

当前 `SubAgentTool` 在初始化时创建一个固定子 session，并在后续调用中持续复用。

这意味着：

- 子 Agent 会累积历史上下文
- 多次调用同一个子 Agent 并不是“全新任务”
- 行为会受到历史执行影响

这未必一定是错的，但必须明确这是设计选择，否则上层很容易误以为每次调用都是全新执行。

后续建议把子 Agent 的 session 策略做成可配置。

### 12. build 阶段直接 panic 不适合作为框架默认行为

当前在 skill 工具重名时会直接 `panic!`。

对于 demo 或内部工具问题不大，但对通用框架来说不够友好。更合理的方式是：

- `build()` 返回 `Result<AgentRuntime, AgentError>`
- 由上层决定是中断、降级，还是提示配置错误

### 13. 文档与实现存在轻微偏差

README 中提到“内置 OpenAI / DashScope 实现”，但代码中当前内置的是 OpenAI / Anthropic。

这类问题不大，但说明项目文档还没有完全跟上实现演进。对于通用内核来说，文档应尽量保持为架构契约的一部分。

## 风险优先级建议

### P0：建议优先处理

1. 统一运行结果语义
   - 明确 `RunCompleted` / `RunFailed` / 返回值的关系
   - 避免事件成功但返回值成功/失败不一致

2. 收紧错误恢复语义
   - 不要把工具失败后的恢复策略固定写死在 runtime 主循环中

3. build 阶段避免 panic
   - 将配置冲突改成显式错误返回

4. 补齐 checkpoint 的 tool results
   - 让 checkpoint 成为真正可回放、可调试的数据结构

### P1：建议尽快演进

1. 强化 ToolOutput 建模
2. 让 SessionStore 真正进入主路径
3. 用 `LlmCapabilities` 驱动运行时决策
4. 明确并配置化 SubAgent session 策略
5. 补齐 Skill 自动注入时的名字冲突校验

### P2：中长期架构演进

1. 从简单 ReAct loop 逐步演进到可配置执行图 / 状态机
2. 增强 observability：trace、usage、latency、replay
3. 逐步形成 memory / state 的专门抽象
4. 将 provider 差异从“字段兼容”升级为更规范的能力模型

## 建议的项目定位

建议将 `agent-core` 的定位明确为：

> 一个偏“运行时编排层”的 Agent Kernel，而不是完整的 Agent Platform。

即它主要负责：

- turn loop
- tool dispatch
- approval orchestration
- event streaming
- middleware hooks
- session lifecycle

而不是试图在这一层同时承载：

- 全部 memory 机制
- 全部持久化策略
- 全部 workflow 编排
- 全部可观测性系统
- 全部 UI / 人机交互

这样项目边界会更健康，也更容易做出稳定 API。

## 当前成熟度判断

如果以“通用 Agent 内核”为标准：

- 架构方向：7/10
- 框架稳定度：5.5/10

含义是：

- 方向是正确的
- 已经具备比较好的起步骨架
- 适合继续沿当前方向演进
- 但暂时还不建议过早冻结 API 或让太多业务 Agent 强依赖当前实现细节

## v1 改造完成记录

基于上述 review，v1 改造已实施完成，具体变更包括：

### 已实施

1. **统一 run result 语义**
   - 新增 `RunOutcome::Completed` / `RunOutcome::Failed`
   - `AgentEvent::RunCompleted` / `RunFailed` 合并为 `RunFinished`
   - `run_turn_*` 返回 `AgentResult<RunOutcome>`
   - `run_turn_stream` 返回 `AgentResult<(Vec<AgentEvent>, RunOutcome)>`

2. **工具错误恢复策略外置**
   - 新增 `ToolErrorRecovery` trait + `ToolErrorAction`
   - 默认 `StopOnError`（保守默认）
   - 可选 `RetryOnError`（回灌错误让模型继续）
   - 移除 runtime 中硬编码的恢复提示逻辑

3. **收敛 session/store 边界**
   - `SessionStore` 明确为 persistence adapter
   - `AgentRuntime` 是 live session state 的 source of truth
   - 新增 `restore_session` 显式恢复入口

4. **SubAgentTool 默认 session 策略**
   - 新增 `SubAgentSessionPolicy::Ephemeral`（默认）
   - 新增 `SubAgentSessionPolicy::Persistent`
   - 默认每次调用创建新 session

5. **文档收口**
   - README 更新为轻内核定位
   - provider 说明修正（OpenAI / Anthropic）
   - 新增 v1 语义约定章节

### 未实施（按计划不做）

- workflow graph / DAG 引擎
- memory framework
- 多 agent orchestration
- checkpoint replay 系统
- 重型 persistence / transaction
- provider 大一统重构
- observability 平台化

## 建议后续优先讨论的 5 个议题

1. 一次 run 的最终状态语义如何定义
2. tool error recovery 是 runtime 默认行为，还是策略注入
3. session 的 source of truth 在 runtime 还是 store
4. SubAgentTool 默认复用 session 是否符合预期
5. `agent-core` 最终是轻内核，还是更完整的执行框架

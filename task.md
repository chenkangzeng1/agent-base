# agent-base v1 实施清单

## 目标

本次实施以 `agent-base` 作为“轻内核”前提进行，不追求扩展更多能力，而是优先完成语义收敛、状态边界收敛和默认行为保守化。

核心目标：

1. 语义清晰
2. 状态简单
3. 默认保守
4. 策略外置

本轮改造聚焦于让 `agent-base` 从“能跑的 Agent Runtime”收敛为“语义清晰、默认保守、状态简单、策略外置的轻内核”。

---

## 实施范围

本次只做以下 5 个主题：

1. 统一 run result 语义
2. 将 tool error recovery 从 runtime 默认行为改为策略外置
3. 收敛 session / store 的职责边界
4. 调整 SubAgentTool 默认 session 策略
5. 更新 README / review / tests / examples，使设计与实现一致

明确不做：

- workflow graph / DAG 引擎
- memory framework
- 多 agent orchestration 平台
- 复杂 checkpoint replay 系统
- 重型 persistence / transaction 设计
- provider 抽象的大一统重构
- observability 平台化建设

---

## Phase 1：统一 run result 语义

### 目标

解决当前一次 run 的最终状态表达不清晰的问题，避免：

- `RunCompleted` 同时表示“结束”和“成功”
- `RunFailed` 与返回值语义不一致
- 上层调用方无法准确判断本次 run 的真实结果

### 实施任务

#### Task 1.1 设计统一的运行结果模型

新增显式的运行结果类型，例如概念上包含：

- `Completed`
- `Failed`
- `Cancelled`
- `MaxTurnsExceeded`
- `AwaitingApproval`
- `AwaitingExternalInput`

要求：

- 运行结果表达“最终态”
- 事件流只表达“过程态”
- “结束”和“成功”必须分离

#### Task 1.2 调整 `run_turn_*` 系列 API 返回语义

目标：

- 不再仅返回 `AgentResult<()>`
- 改为返回明确的运行结果
- runtime 自身无法继续执行时才返回 `Err`

涉及范围：

- `run_turn_with_handler`
- `run_turn_stream`
- 相关测试与上层依赖调用

#### Task 1.3 收敛 `AgentEvent` 中的结束事件语义

明确：

- 结束事件是否保留 `RunCompleted / RunFailed`
- 或者改为更语义化的结束事件
- 事件必须与最终返回值保持一致，不允许冲突

#### Task 1.4 补测试

新增或调整测试覆盖：

- 普通文本完成
- 工具成功完成
- tool not found
- tool error
- cancelled
- max turns exceeded
- approval 等待/拒绝/继续

### 交付结果

- 一套明确的 run outcome 模型
- API 与事件语义一致
- 上层调用方可稳定判断一次 run 的最终结果

---

## Phase 2：将 tool error recovery 改为策略外置

### 目标

移除 runtime 当前对工具失败的固定恢复策略，使内核默认行为更保守、更中立。

### 当前问题

现在 runtime 在工具执行失败后，会自动向 session 注入 assistant/user 消息，引导模型重新规划。

这个行为过于 opinionated，不适合作为轻内核默认行为。

### 实施任务

#### Task 2.1 定义工具失败后的恢复策略抽象

新增轻量策略接口或 hook，用来决定工具失败后的处理方式。

建议至少支持这些方向：

- 结束当前 run
- 将错误作为工具结果回灌给模型
- 请求外部处理
- 使用默认保守策略

注意：

- 不要设计成重型 workflow 机制
- 只聚焦“tool error 后如何决策下一步”

#### Task 2.2 移除 runtime 主循环中的硬编码恢复提示逻辑

目标：

- runtime 不再自动拼接“请简化计划再重试”之类的提示
- runtime 只负责结构化记录失败事实
- 恢复行为由策略决定

#### Task 2.3 提供默认保守策略

默认策略建议：

- emit tool error event
- 记录错误到 session / tool result
- 结束 run 或返回失败态
- 不默认自动重新规划

#### Task 2.4 补测试

覆盖：

- 默认保守策略
- 将错误回灌给模型继续推理的策略
- approval deny 与 tool execution error 的区别处理
- cancel 与普通失败的区别处理

### 交付结果

- tool error recovery 从 runtime 行为变为可插拔策略
- 默认行为更符合轻内核设计原则

---

## Phase 3：收敛 session / store 的职责边界

### 目标

明确 session 的 source of truth 在 runtime，而 `SessionStore` 是 persistence adapter。

### 设计原则

- runtime 内存态是运行中的权威状态
- store 负责可选持久化
- 不让 store 介入每一步执行控制流

### 实施任务

#### Task 3.1 明确 runtime 与 store 的职责边界

需要在代码与文档层面统一以下语义：

- `AgentRuntime.sessions` = live session state
- `SessionStore` = load/save/list/delete adapter
- run 期间以内存态为准

#### Task 3.2 梳理并视情况补充辅助 API

根据当前接口情况，评估是否需要增加更明确的 session 操作能力，例如：

- restore session
- persist session
- 显式初始化已有 session

要求：

- 能提升语义清晰度
- 不把内核做重

#### Task 3.3 更新实现中的持久化边界

检查并调整：

- run 前是否需要显式恢复 session
- run 后保存时机是否更明确
- 保存失败是否影响最终 run outcome

#### Task 3.4 补测试

覆盖：

- 纯内存 session 正常执行
- session save 成功/失败
- store load 后恢复执行
- runtime 内存态与 store 语义保持一致

### 交付结果

- session / store 边界清晰
- state ownership 明确
- 更符合轻内核的 memory-first 设计

---

## Phase 4：调整 SubAgentTool 默认 session 策略

### 目标

避免子 agent 默认复用 session 导致隐式上下文污染，使默认行为更可预测。

### 建议结论

- 默认策略改为 `Ephemeral`
- 每次调用创建新 session
- session 复用改为显式配置能力

### 实施任务

#### Task 4.1 设计 SubAgent session policy

增加子 agent session 策略类型，建议至少支持：

- `Ephemeral`：每次新 session
- `Persistent`：固定复用 session

默认值设为 `Ephemeral`。

#### Task 4.2 调整 `SubAgentTool` 实现

修改当前固定复用 session 的实现方式，使其根据 policy 决定：

- 新建 session
- 或复用已有 session

#### Task 4.3 更新示例与测试

同步调整：

- `examples/subagent_demo.rs`
- 相关测试用例
- 必要的 README 说明

覆盖：

- ephemeral 模式下多次调用互不污染
- persistent 模式下上下文连续

### 交付结果

- SubAgent 默认行为更保守
- 上层若需要长期子 Agent，可显式开启 persistent

---

## Phase 5：文档、示例、测试统一收口

### 目标

保证本次语义与设计变化在文档、示例、测试中保持一致，避免架构漂移。

### 实施任务

#### Task 5.1 更新 README

重点更新：

- `agent-base` 的轻内核定位
- provider 说明与实际代码一致
- session / store 边界说明
- SubAgent 默认行为说明
- run result 语义说明

#### Task 5.2 更新 review 文档

将本次讨论确认的“轻内核设计主张”和“v1 改造落地方式”同步到 review 文档，作为后续架构演进依据。

#### Task 5.3 更新测试

统一收口所有新增语义：

- run outcome
- tool error recovery
- session/store boundary
- subagent policy

#### Task 5.4 更新 example

检查并调整：

- `repl.rs`
- `subagent_demo.rs`
- 其他受运行语义影响的 example

确保对外示例符合 v1 设计。

### 交付结果

- 文档、测试、示例、实现一致
- 后续维护成本降低

---

## 推荐执行顺序

建议按以下顺序落地：

1. 统一 run outcome 语义
2. 抽出 tool error recovery 策略
3. 调整 SubAgentTool session policy
4. 收敛 session/store 边界
5. 统一 README / review / tests / examples

说明：

- 第 1、2 步决定 runtime 的语义纯度，必须先做
- 第 3 步是行为默认值修正，影响面适中
- 第 4 步偏状态模型收敛，适合在主要语义稳定后处理
- 第 5 步用于最终收口

---

## 需要 review 时重点确认的决策点

### 决策点 1：是否接受 `run_turn_*` API 变化

本次最核心的接口变化，是让 `run_turn_*` 返回明确运行结果，而不是简单 `AgentResult<()>`。

这会影响：

- 测试
- examples
- 上层依赖 crate

需要先确认方向。

### 决策点 2：是否接受 runtime 默认不再自动“帮模型恢复”

也就是移除当前硬编码的“失败后让模型重新规划”的默认逻辑。

如果确认这一点，runtime 将更接近中立轻内核。

### 决策点 3：是否确认 runtime 是 session live state 的 source of truth

如果确认，则：

- store 不升格为执行期主状态源
- 内核保持 memory-first

### 决策点 4：是否接受 SubAgent 默认改成 ephemeral

这会改变当前默认行为，需要明确是否接受。

### 决策点 5：是否本次同步收口 README / review / tests / examples

建议一起做，否则设计与实现容易再次漂移。

---

## 最终目标状态

本次实施完成后，希望 `agent-base` 具备以下特征：

- 一次 run 的结果语义清晰
- 事件与返回值不冲突
- tool error recovery 策略外置
- runtime 默认行为保守
- session / store 边界明确
- SubAgent 默认无隐式上下文污染
- 文档、示例、测试与设计保持一致

---

## 实施完成记录

> **执行日期**: 2026-05-15

### Phase 1: 统一 run result 语义 ✓

- [x] 新增 `src/types/outcome.rs` — `RunOutcome::{Completed, Failed}`
- [x] `AgentEvent::RunCompleted` / `RunFailed` → `RunFinished`
- [x] `run_turn_with_handler` 返回 `AgentResult<RunOutcome>`
- [x] `run_turn_stream` 返回 `AgentResult<(Vec<AgentEvent>, RunOutcome)>`
- [x] `resume_from_checkpoint` 返回 `AgentResult<RunOutcome>`
- [x] 所有 example / test 同步更新

### Phase 2: tool error recovery 策略外置 ✓

- [x] 新增 `src/engine/recovery.rs` — `ToolErrorRecovery` trait + `StopOnError` / `RetryOnError`
- [x] `AgentRuntime` 增加 `error_recovery` 字段（默认 `StopOnError`）
- [x] `AgentBuilder` 增加 `error_recovery()` 方法
- [x] runtime 主循环中 3 处硬编码恢复逻辑改为策略调用
- [x] 测试中需要 retry 行为的注入 `RetryOnError`

### Phase 3: 收敛 session/store 边界 ✓

- [x] `SessionStore` trait 增加文档注释，明确为 persistence adapter
- [x] `AgentRuntime` 新增 `restore_session()` 方法
- [x] 明确 runtime 内存态 = source of truth

### Phase 4: SubAgentTool 默认 session 策略 ✓

- [x] 新增 `SubAgentSessionPolicy::{Ephemeral, Persistent}`
- [x] `SubAgentTool` 默认 `Ephemeral`（每次新 session）
- [x] 新增 `SubAgentTool::with_persistent()` 构造方法
- [x] 导出 `SubAgentSessionPolicy`

### Phase 5: 文档收口 ✓

- [x] README 更新为轻内核定位、设计原则、v1 语义约定
- [x] review.md 追加 v1 改造完成记录
- [x] agent-base 15 tests + 4 unit tests = 19 passed
- [x] ops-agent 4 tests passed

---

## 一句话总结

本次 v1 改造的本质是：

> 把 `agent-base` 从“能跑的 Agent Runtime”收敛成“语义清晰、默认保守、状态简单、策略外置的轻内核”。

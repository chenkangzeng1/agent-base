# phi-agent 框架路线图

> 状态: 已完成 (Phase 1-7)
> 日期: 2026-08-09
> 版本: v0.9.0

---

## 定位

**Rust 生态 MCP-First 的高性能单 Agent 运行底座。**

不做 LangGraph 替代品，不做图编排引擎。内核保持精简——只提供 Agent 运行时基础设施。复杂编排、多 Agent 协作、人在回路等高层能力交给上层。

> 配套生态产品路线图: [`ecosystem-roadmap.md`](./ecosystem-roadmap.md)

---

## 现状评估 (v0.9.0)

Phase 1-7 全部完成。以下为各阶段已交付内容汇总。

### 已完成 ✅

- **内核**: Agent 运行时、`AgentError` 强类型错误 (17 variants) + `ErrorKind` 分类 + `is_retryable()`/`is_rate_limited()`、`Middleware` trait (3 个 hook)、`ToolPolicy` trait (`evaluate_approval`/`before_call`/`after_call`)、Pipeline (before→execute→after→truncation)、`UserEvent` 系统 (Progress/SubAgentEvent/Structured)、plan 追踪事件
- **MCP**: `McpClient` (HTTP + Stdio)、`EnhancedMcpHub` (连接池 + 健康检查 + 自动重连 + 状态订阅 + 运行时 `connect_one`/`disconnect_one`)
- **渲染**: Terminal / JsonStream / Null 三种 Renderer
- **CLI**: REPL + one-shot，AutoApprovalHandler
- **Session**: 持久化、文件锁、JSONL 事件日志
- **Bridge**: `phi serve` NDJSON 协议
- **Browser**: 21 个 CDP 工具（`browser` feature gate）
- **测试**: 108+ 测试 + CI
- **文档**: docs.rs + mdBook + guide (advanced.md 含 ToolPolicy/Middleware 示例、observability.md) + 6 个示例 (含 `custom-policy.rs` 覆盖 ToolPolicy/Middleware)

### 当前短板 ❌

| 短板 | 说明 | 优先级 |
|------|------|--------|
| phi-agent 框架层用 `anyhow::Result` 包装了内核的 `AgentResult` | agent-base 已有完善的强类型错误体系，但 `src/config/llm.rs`、`src/session.rs`、`src/agent/factory.rs`、CLI 大量使用 `anyhow::Result`，上层无法 `match` 错误类型做分类处理 | ✅ 已完成 |
| MCP 运行时管理无法从 `PhiAgent` 使用 | `EnhancedMcpHub` 已有 `connect_one`/`disconnect_one` 等全套运行时管理能力，但 `PhiAgent` 没有暴露对应方法，使用模式只能构建时一次性注入。注意：此能力不能直接加到 `AgentRuntime`，因为 agent-base 零 MCP 依赖，需要在 phi-agent 层 (`PhiAgent`) 实现 | ✅ 已完成 |
| 示例组织扁平、缺 MCP/session 示例 | 6 个 example 扁平放在根目录，虽有 `custom-policy.rs` 覆盖 ToolPolicy/Middleware，但缺少 MCP/session 相关示例 | ✅ 已完成 |
| README 未管理预期 | 无"不做什么"/安全警告章节，容易产生无效 issue | ✅ 已完成 |
| ToolPolicy trait 缺少 Rust doc 注释 | `agent-base/src/tool/policy.rs` 的 trait 定义无 doc string，用户需要翻 guide 才知道用法 | ✅ 已完成 |
| `ToolPolicy`/`Middleware` 文档可见性不足 | 能力存在于 `guide/advanced.md`，但不在主文档链路中，新用户容易错过 | ✅ 已完成 |
| 事件缺 `agent_id`/`trace_id` | `RuntimeEvent` 有 `session_id`，但缺少分布式追踪所需字段 | ✅ 已完成 |
| CONTRIBUTING 未列不接受的功能 | PR 方向可能跑偏 | ✅ 已完成 |

---

---

## 架构分层

依赖链共 4 层：agent-base → agent-works → phi-kernel-tools → phi-agent（phi-tools 旁路 agent-base）。

```
agent-base                         ← 裸内核（AgentRuntime 引擎）
    ↑
agent-works                        ← 纯基础设施（trait + 默认实现 + registry + prompter）
    ↑                                MemoryStore, FileMemoryStore
    │                                AgentRegistry, Mailbox, AgentPath
    │                                SkillRegistry, SkillPrompter
    │
    ├── phi-kernel-tools            ← 内核工具（Tool 实现）
    │                                   remember/recall/forget
    │                                   spawn_agent/send_message/wait_agent/...
    │                                   list_skills/get_skill_detail/apply_skill
    │
    ├── phi-tools                   ← 业务工具市场（只依赖 agent-base）
    │                                   LocalShellTool, browser tools
    │                                   社区贡献的工具
    │
    ▼
phi-agent                           ← 发行版（CLI + 渲染 + 配置 + Session + phi-kernel-tools）
```

| 层级 | 默认带了什么 | 适用场景 |
|------|------------|---------|
| **agent-base** | 什么都没有。裸 AgentRuntime | 自定义 runtime、嵌入式、不想引入任何框架能力 |
| **agent-works** | 纯基础设施（trait + 默认实现） | 需要基础设施但定制内核工具 |
| **phi-kernel-tools** | 内核工具（Tool 实现） | 框架开发者、需要按需编排内核工具 |
| **phi-agent** | CLI + 渲染 + Session + phi-kernel-tools（全默认开） | 开箱即用的发行版，`cargo add phi-agent` 就能跑 |
| **phi-tools** | 业务工具（LocalShellTool, browser） | 需要业务工具，支持社区贡献 |

**默认开启策略：**

| 能力 | agent-base | agent-works | phi-kernel-tools | phi-agent | feature gate |
|------|-----------|-------------|-----------------|-----------|-------------|
| File | ❌ | ❌ | ❌ | ✅ 默认开 | `file` |
| Shell | ❌ | ❌ | ❌ | ✅ 默认开 | `shell` |
| Multi-Agent | ❌ | ❌ | ❌ | ❌ | `multi-agent` |
| Skills | ❌ | ❌ | prompt 注入 | prompt 注入 | — |
| Memory | ❌ | ❌ | prompt 注入 | prompt 注入 | — |
| MCP | ❌ | ❌ | ❌ | ❌ | `mcp` |

**默认哲学：file + shell 是 agent 的"眼睛和手"，基础感知/执行能力默认开。multi-agent 等进阶能力 opt-in。**

编译期排除：`cargo build --no-default-features` 或按需排除特定 feature gate。

用户如何关闭：

```rust
// phi-agent 发行版 — 默认有 file + shell，能读能执行
phi_agent::base_agent_builder(client).build()

// 进阶能力 opt-in
// cargo run --features multi-agent    → spawn 子 Agent
// cargo run --features mcp            → MCP 客户端 + Server

// 裸内核 — 什么都没有
agent_base::AgentBuilder::new(client).build()
```

**两类"工具"的区分：**

phi-agent 把 Agent 可用的能力分为两类：

| | 内核工具（phi-kernel-tools） | 业务工具（phi-tools） |
|------|------|------|
| 示例 | memory、multi-agent、skills、MCP | 搜索、文件读写、代码执行 |
| 依赖 | agent-works | agent-base |
| 谁提供 | phi-kernel-tools crate，随 phi-agent 默认注入 | 用户注册 + 社区贡献 |
| 原则 | 内核工具是框架能力的一部分，agent-works 只做纯基础设施 | 用户注册什么就用什么，不预设任何业务工具 |

对外一句话：**不预设应用工具，内核工具可按需开关，LLM 自由，精准、干净、可控。**

---

## Phase 1: 修框架层倒退（v0.3.1）✅ 已完成

> **目标:** 不新增基础设施，消除 phi-agent 层代码问题 + 暴露已有 MCP 能力。**工期: 3-5 天。**

### 1.1 消除 anyhow 倒退

agent-base 已有完善强类型错误体系，但 phi-agent 层多处反向包装成 `anyhow::Result`：

- `src/agent/factory.rs`: `PhiAgent::build()` → 返回 `AgentResult<Self>`
- `src/config/llm.rs`: `resolve_llm_config()` → 返回 `AgentResult`，用 `AgentError::ConfigError`
- `src/session.rs`: → 用 `AgentResult` 替代 `anyhow::Result`
- `src/event_log.rs`: → IO 操作返回 `AgentResult`
- CLI binary：入口层保留 `anyhow::Result` 可接受，内部传播 `AgentError`

### 1.2 PhiAgent 暴露 MCP 运行时管理

`EnhancedMcpHub` 已有 `connect_one`/`disconnect_one` 等 API，但 `AgentRuntime` (agent-base) 零 MCP 依赖。在 `PhiAgent` 层暴露：

- `phi_agent.attach_mcp(config)` / `phi_agent.detach_mcp(name)` — 委托给 `EnhancedMcpHub`，自动更新 ToolRegistry
- 方法需 `#[cfg(feature = "mcp")]` 守卫，phi-agent 新增 `"mcp"` feature 透传 `agent-works/mcp`

**产出:** `AgentResult` 全链路传播 + `PhiAgent` MCP 运行时 API + feature-flag 策略

---

## Phase 2: 文档 & 示例补齐（v0.3.2）✅ 已完成

> **目标:** 降低新用户上手门槛，管理社区预期。**工期: 3-5 天。**

### 2.1 examples 目录重组

```
examples/
├── minimal/
│   └── hello_agent.rs
├── tools/
│   ├── custom_tool.rs
│   └── custom_policy.rs          ← 已有，移入
├── mcp/
│   ├── mcp_client.rs             ← 新增
│   └── mcp_dynamic_attach.rs     ← 新增
├── session/
│   └── session_persist.rs        ← 新增
├── observability/
│   ├── event_log.rs              ← 新增
│   └── middleware_hooks.rs       ← 新增
└── advanced/
    ├── window_memory.rs          ← 新增
    └── summary_memory.rs         ← 新增
```

### 2.2 README 重写

新增章节：
- ✅ 适合做什么 / ⚠️ 不提供什么 / 🧩 与 LangGraph 协同 / 🔒 安全提醒

### 2.3 CONTRIBUTING.md 补充

加入"明确不会接受的功能"清单。

**产出:** 分类 examples + 新 README + CONTRIBUTING 预期管理

---

## Phase 3: Multi-Agent（v0.4.0）✅ 已完成

> **目标:** LLM 能动态 spawn 子 Agent 并行执行任务。agent-works 新增。**工期: 2-3 周。**

> 参考 Codex MultiAgentV2 (`codex-rs/core/src/tools/handlers/multi_agents_v2/`)。

> **状态: 全部完成 — 基础设施 + 6 个 Tool + Builder 集成 + 展示层 + 测试。**

### 3.1 基础设施（agent-works）

| 组件 | 职责 | 状态 |
|------|------|------|
| `AgentRegistry` | 跟踪活跃子 Agent，限制总数和 spawn 深度 | ✅ |
| `Mailbox` | Agent 间异步通信 (`async_channel`)，支持发送/接收 | ✅ |
| `AgentPath` | 树形路径标识 (`root/searcher`)，用于路由消息 | ✅ |
| `MultiAgentRuntime` | JoinSet + CancellationToken 树 + 事件桥接 | ✅ |

### 3.2 暴露给 LLM 的 6 个工具

| 工具 | 作用 | 状态 |
|------|------|------|
| `spawn_agent` | 动态创建子 Agent（继承父 Agent tools + 定制 system prompt） | ✅ |
| `send_message` | 发消息，不触发执行 | ✅ |
| `followup_task` | 发任务并立即触发执行 | ✅ |
| `wait_agent` | 阻塞等待子 Agent 消息 | ✅ |
| `list_agents` | 列出活跃子 Agent | ✅ |
| `close_agent` | 关闭指定子 Agent | ✅ |

### 3.3 设计原则

- **不进 agent-base。** 多 Agent 是跨 Agent 概念，不在单 Agent 运行时职责范围。
- **不和 `update_plan` 联动。** 经审计 Codex 确认：plan 和 multi-agent 无框架层连接。LLM 看到两个独立工具，自己决定何时 spawn。
- **子 Agent 并发执行。** tokio 独立 task + `JoinSet` 管理生命周期。

### 3.4 与现有 `SubAgentTool` 的关系

agent-base 已有 `SubAgentTool`，定位不同：它是"把 Agent 当同步函数调用"，不支持并发。Phase 3 的 spawn+mailbox 是"动态创建并发协作者"。两者不冲突但层次不同——Phase 3 完成后评估是否废弃 `SubAgentTool`。

### 3.5 事件转发：复用 `UserEvent::SubAgentEvent`

子 Agent 的工具调用/思考/文本输出通过现有的 `UserEvent::SubAgentEvent { subagent, event }` 转发到父 Agent 事件流。`Mailbox` 只传 LLM 级的任务消息（分发、结果回传），不复制事件通道。后续可把 `subagent: String` 升级为树形 `AgentPath`。

### 3.6 失败模式

| 场景 | 处理 |
|------|------|
| 子 Agent 崩溃 | 父 Agent 收到错误事件，`JoinSet` 自动清理，不阻塞其他子 Agent |
| 父 Agent 取消 (Ctrl+C) | 取消信号沿 `AgentPath` 树传播到所有子孙 Agent |
| `close_agent` 时子 Agent 仍在执行 | 默认 force-kill，后续可加 graceful timeout |
| 多子 Agent 事件交织 | 通过 `SubAgentEvent.subagent` 字段区分来源，UI 按 Agent 分组展示 |
| 子 Agent spawn 深度过大 | `AgentRegistry` 限制最大深度（默认 2 层），超限拒绝 |

### 3.7 Token 预算 / 资源治理（后续增强）

Phase 3 第一版不内置复杂资源治理，但保留扩展点：

- `AgentRegistry` 记录每子 Agent 累计 token 消耗，父 Agent 可查询
- 后续版本可加 `max_tokens_per_sub_agent` / `total_token_budget` 配置
- Rate-limit 背压机制留到 Phase 6 压测后根据实际数据设计

### 3.8 Builder 集成 ✅

**决策：`base_agent_builder()` 已切换到 `agent_works::AgentBuilder`。**

`base_agent_builder()` 现在返回 `agent_works::AgentBuilder`，默认开启 multi-agent：

```rust
// phi-agent/src/agent/builder.rs
pub fn base_agent_builder(llm_client: Arc<dyn agent_base::LlmClient>) -> agent_works::AgentBuilder {
    let mut builder = agent_works::AgentBuilder::new(llm_client)
        .language(Language::En)
        // ... 其他默认配置
    ;

    #[cfg(feature = "multi-agent")]
    {
        builder = builder
            .with_multi_agent(MultiAgentConfig::default())
            .with_multi_agent_tool_factory(Arc::new(|runtime| {
                phi_kernel_tools::multi_agent::create_all_tools(runtime)
            }));
    }
    builder
}
```

- `agent_works::AgentBuilder::build()` 在 `build()` 阶段初始化 `MultiAgentRuntime` + 事件桥接 + 注册 6 个多 Agent 工具
- 需要裸内核的用户直接用 `agent_base::AgentBuilder::new(client)`
- 运行时关闭：`.without_multi_agent()`
- 编译期排除：`cargo build --no-default-features` 去掉 `multi-agent` feature

**产出:** agent-works `multi_agent` 基础设施 + 6 个 Tool + phi-agent Builder 集成 ✅

---

## Phase 4: MCP Server + Skills 对齐标准 + 事件增强（v0.4.1）✅ 已完成

> **目标:** Agent 可被外部编排框架调用，Skills 对齐 Agent Skills 开放标准（agentskills.io），补齐分布式追踪字段。**工期: 3-4 周。**

### 4.1 Agent → MCP Server

**模式 B：暴露 Agent，不暴露工具列表。**

和 Codex/Claude Code 一致——phi-agent 暴露一个入口让外部编排器把任务交给自己，内部跑完整 ReAct，流式返回过程。

```
外部编排器 (LangGraph/CrewAI/自研)          phi-agent (MCP Server)
       │                                          │
       │  tools/list                               │
       │──────────────────────────────────────────▶│
       │  [{ name: "run",                          │
       │     description: "Execute a task",         │
       │     inputSchema: { prompt, model? } }]     │
       │◀──────────────────────────────────────────│
       │                                          │
       │  tools/call { name: "run",                │
       │               arguments: {                │
       │                 prompt: "找出所有 SQL 注入" │
       │               }}                          │
       │──────────────────────────────────────────▶│
       │                                          │  创建 session
       │                                          │  agent.run_turn(session, prompt)
       │  progress: "正在搜索代码..."              │
       │◀──────────────────────────────────────────│  ReAct 步骤 1
       │  progress: "发现 15 个可疑文件"           │
       │◀──────────────────────────────────────────│  ReAct 步骤 2
       │  progress: "确认 3 处漏洞"                │
       │◀──────────────────────────────────────────│  ReAct 步骤 N
       │  result: "发现 3 处 SQL 注入..."          │
       │◀──────────────────────────────────────────│  最终结果
```

#### 为什么只暴露一个"工具"

| 做法 | 暴露内容 | 问题 |
|------|---------|------|
| ❌ 暴露工具列表 | phi-agent 注册的 search/code_exec/db_query | phi-agent 成工具容器了，自身的推理编排全没用上 |
| ✅ 暴露 Agent 自己 | 一个 `run` 入口 | 外部编排 + phi-agent 执行，各司其职。和 Codex `codex()` / Claude Code `claude_code()` 一致 |

#### 实现方案

**放在 agent-works**（`src/mcp/server.rs`），复用现有 `mcp` feature gate。不新开 crate。

```
agent-works/src/mcp/
├── client.rs          ← 已有（phi-agent 接入外部 MCP）
├── hub.rs             ← 已有
├── enhanced_hub.rs    ← 已有
├── server.rs          ← 新增（对外暴露 phi-agent）
└── mod.rs
```

**桥接 RuntimeEvent → MCP progress：**

```
RuntimeEvent                    MCP Progress Notification
  Thought { text }          →   正在思考：{text}
  ToolCallStart { name }    →   正在调用 {name}...
  ToolCallResult { data }   →   {name} 完成
  Text { text }             →   {text}
  RunCompleted { outcome }  →   返回最终结果
```

- 传输：stdio（子进程模式）+ HTTP Streamable（服务模式），MCP 标准
- 流式：通过 MCP progress notification 或 SSE 推送每一步进展
- 外部可实时看到 Agent 在干嘛，不是干等黑盒结果

**产出:** agent-works `src/mcp/server.rs` + stdio / HTTP 双传输 + RuntimeEvent 桥接 ✅ 已完成

### 4.2 Skills 对齐开放标准（agent-works）✅ 已完成

当前 phi-agent 的 `PromptSkill` 只支持 Markdown + YAML frontmatter 的基础字段。行业已形成统一开放标准 **Agent Skills**（agentskills.io，Anthropic 2025.12 发布，Claude Code / OpenAI Codex / Gemini CLI / Copilot / Cursor 全部遵循）。

#### 4.2.1 目录结构对齐

当前只支持单文件字符串解析，需支持标准目录结构：

```
.phi/skills/
  deploy/
    SKILL.md              ← YAML frontmatter + Markdown 指令（必需）
    scripts/              ← 可执行辅助脚本（可选）
    references/           ← 按需加载的参考文档（可选）
    templates/            ← 模板文件（可选）
  review/
    SKILL.md
    references/
      checklist.md
```

- 扫描 `.phi/skills/`（项目级）+ `~/.phi/skills/`（用户级），按优先级合并
- `PromptSkill` 增加 `scripts_dir()` / `references_dir()` 方法

#### 4.2.2 SKILL.md frontmatter 字段补齐

标准字段，当前缺失的全部补齐：

| 标准字段 | 说明 | 当前 |
|------|------|------|
| `name` | kebab-case，≤64 字符，必须与目录名一致 | ✅ |
| `description` | 一句话描述，用于 LLM 判断何时激活 | ✅ |
| `version` / `author` / `tags` | 元信息 | ✅ |
| **`allowed-tools`** | 技能激活时可用工具白名单（逗号分隔） | ❌ 新增 |
| **`disallowed-tools`** | 技能激活时禁用的工具 | ❌ 新增 |
| **`model`** | 覆盖模型选择（`inherit` 默认） | ❌ 新增 |
| **`user-invocable`** | 用户能否手动 `/` 调用（默认 true） | ❌ 新增 |
| **`disable-model-invocation`** | 禁止 LLM 自动触发（用于 deploy/commit 等有副作用的技能） | ❌ 新增 |
| **`arguments`** | 参数占位符列表，如 `[branch, env]`，body 中用 `$branch` / `$env` 引用 | ❌ 新增 |
| **`context`** | 设为 `fork` 时在独立子 Agent 上下文运行 | ❌ 新增 |
| **`paths`** | gitignore 风格 glob，条件技能：匹配文件变更时才激活 | ❌ 新增 |

#### 4.2.3 变量替换

Body 中支持标准变量语法：

| 变量 | 说明 |
|------|------|
| `$ARGUMENTS` | 用户 `/skill-name` 后面输入的全部文本 |
| `$name` / `$ARGUMENTS[N]` | 命名参数 / 位置参数 |
| `${CLAUDE_SKILL_DIR}` → `$PHI_SKILL_DIR` | 技能目录路径（引用 scripts/） |

#### 4.2.4 渐进式披露

当前所有技能 body 全量注入 system prompt（浪费 token）。改为标准的三层渐进式：

| 层级 | 内容 | 时机 |
|------|------|------|
| **发现层** | name + description（每个技能 ~50 tokens） | 始终在 system prompt（`LazySkillPrompter`） |
| **激活层** | SKILL.md body（建议 ≤5000 tokens） | LLM 调用 `get_skill_detail` 时返回 |
| **执行层** | scripts/ + references/ | 技能执行时按需读取（`Skill` trait 新增 `read_reference(path)`） |

`FullDetailPrompter` 保留，适合技能数量少且固定的场景。

#### 4.2.5 运行时基础设施

| 项目 | 说明 | 所属 |
|------|------|------|
| **`list_skills` 工具** | LLM 运行时主动列举可用技能及状态 | phi-kernel-tools |
| **`get_skill_detail` 工具** | 按需获取技能详情（渐进式披露激活层） | phi-kernel-tools |
| **`apply_skill` 工具** | 应用技能，执行 scripts/ + references/ | phi-kernel-tools |
| **运行时启用/禁用** | `SkillRegistry::enable()` / `disable()`，无需重启 | agent-works |
| **热加载** | 目录模式 + `notify` crate 监听文件变更，自动重载 | agent-works |
| **参数类型校验** | `apply_skill` 校验 params 类型（number 拒绝非数字值） | phi-kernel-tools |

#### 4.2.6 兼容性承诺

- 当前 `PromptSkill::from_markdown()` API 保持不变，作为单文件字符串加载入口
- 新增 `PromptSkill::from_dir(path)` 作为标准目录加载入口
- 当前自定义 frontmatter 字段继续支持，标准字段优先

### 4.3 事件系统 ✅

- `RuntimeEvent` 增加 `agent_id`、`trace_id` 可选字段 — **已完成 (agent-base fe572a8)**
- OpenTelemetry 示例放 `examples/observability/`，不进内核

### 4.4 ToolPolicy trait 文档 ✅

- `agent-base/src/tool/policy.rs` 加 Rust doc 注释 — **已完成 (agent-base fe572a8)**

**产出:** agent-works `mcp/server.rs` MCP Server + phi-kernel-tools Skills Tool 对齐标准

### 4.5 Phase 4 完成总结

**已完成范围：**

| 仓库 | 内容 | 提交 |
|------|------|------|
| agent-works | MCP Server (stdio/HTTP JSON-RPC 2.0)、run 工具暴露、progress 通知、stdout 竞态修复 | `4468719` |
| agent-works | Skills 对齐 agentskills.io：from_dir/scan_dir、变量替换、hot-reload (notify)、SkillRegistry enable/disable/runtime reload | `4468719` |
| phi-kernel-tools | ListSkillsTool、ApplySkillTool 参数校验、tool name 验证 | `9f0306c` |
| phi-agent | auto skill loading (~/.phi/skills + .phi/skills)、MCP server bridge (into_mcp_server)、prompt_skill feature passthrough | `d9f6e8c` |
| agent-base | RuntimeEvent agent_id/trace_id (Phase 4.3)、ToolPolicy rustdoc (Phase 4.4) | `fe572a8` |

**测试覆盖：**
- agent-works: 130 tests（含 hot-reload 2x、tool name validation、prefix clobbering）
- phi-kernel-tools: 38 tests
- phi-agent: 138 tests（108 unit + 12 bridge + 18 integration），含 12 个 snapshot 测试

**剩余收尾（并入 Phase 5）：**
- CLI `phi serve` — MCP Server 入口（stdio + HTTP） ✅ 已完成
- `examples/mcp/mcp_server.rs` 示例 ✅ 已完成
- CHANGELOG 更新 ✅ 已完成

---

---

## Phase 5: 文件系统工具 — 架构基座（v0.5.0）

> **目标:** 给 LLM 文件读写能力，Skills 和 Memory 从"工具模式"改为"prompt 模式"，对齐 Claude Code/Codex。**工期: 3-4 周。**

### 背景：为什么这个 Phase 是转折点

当前 phi-agent 有三个"内核工具组"——skills（3 个 tool）、multi-agent（6 个 tool）、将来 memory（3 个 tool）。**Claude Code 和 Codex 都没有这些工具**——它们只有一个差异：LLM 有文件系统工具。

有文件工具之后，Skills 和 Memory 不再是工具，而是 **prompt 注入 + LLM 自己读写文件**。这是架构基座级别的变化。

```
没有文件工具                              有文件工具
┌──────────────────┐                    ┌──────────────────┐
│ LLM 想读 SKILL.md │                    │ LLM 想读 SKILL.md │
│     ↓            │                    │     ↓            │
│ 调 get_skill_detail │                  │ 调 read_file     │
│     ↓            │   ──变成──▶         │     ↓            │
│ 框架喂给它        │                    │ 框架就是 OS       │
│ (3 个专用工具)    │                    │ (1 组通用工具)    │
└──────────────────┘                    └──────────────────┘
```

**好处：**
1. **工具数量骤减** — 不需要为每个能力写专用工具，LLM 自己操作文件
2. **协议对齐** — 和 Claude Code `read`/`write` + Codex `open` 一致
3. **扩展性** — 用户加新"能力"只需写 markdown 文件，phi-agent 零改动
4. **安全可控** — 文件工具=审批点，所有文件操作过一遍审批

### 5.1 文件工具（phi-kernel-tools，feature gate `file`）

**3 个工具：**

| 工具 | 作用 | 对标 |
|------|------|------|
| `read_file` | 读取文件，支持 offset/limit 分页 | Claude Code Read |
| `write_file` | 写入/创建文件，支持 overwrite | Claude Code Write |
| `list_files` | 列目录内容，支持 glob | Claude Code ls |

**设计要点：**

- **路径安全**：所有路径相对工作目录解析，拒绝 `..` 越权
- **大小限制**：read_file 默认 2000 行上限，write_file 默认 1MB 上限
- **write_file 触发审批**：写入操作必须经过 ApprovalHandler，和 shell 同级安全
- **list_files 不递归**：默认只列一层，显式参数才递归
- **放在 phi-kernel-tools**（不算业务工具）：文件是 agent 的基础感知/操作能力

```toml
# phi-kernel-tools/Cargo.toml
[features]
file = []

# phi-agent/Cargo.toml
[features]
file = ["phi-kernel-tools/file"]
```

### 5.2 Skills 从工具模式改为 Prompt 模式

**去掉：**

| 文件 | 内容 |
|------|------|
| phi-kernel-tools: `src/skill/list_tool.rs` | `ListSkillsTool` |
| phi-kernel-tools: `src/skill/detail_tool.rs` | `SkillDetailTool` |
| phi-kernel-tools: `src/skill/apply_tool.rs` | `ApplySkillTool` |
| phi-kernel-tools: `skill` feature | 不再需要 |
| agent-works: `with_skill_detail_tool_factory` | Builder 方法 |
| agent-works: `with_list_skills_tool_factory` | Builder 方法 |

**保留 & 改造：**

| 组件 | 改为 |
|------|------|
| `PromptSkill::from_dir()` / `scan_dir()` | 保留，仍是加载入口 |
| `SkillRegistry` | 保留，管理技能生命周期 |
| `LazySkillPrompter` | **成为默认** — 把技能列表（name + description）注入 system prompt |
| hot-reload | 保留 |

**新的工作流：**

```
System prompt (始终有):
  ## Available Skills
  - deploy: Deploy to cloud platforms (file: .phi/skills/deploy/SKILL.md)
  - review: Code review checklist (file: .phi/skills/review/SKILL.md)

LLM 判断需要 deploy 技能:
  → 调 read_file(".phi/skills/deploy/SKILL.md")
  → 读到完整技能指令
  → 按指令执行

用户手动调:
  /deploy → CLI 拦截 → 直接注入 SKILL.md 全文
```

**对比旧方案：**

| | 旧方案（工具） | 新方案（prompt + 文件） |
|------|------|------|
| 技能发现 | LLM 调 list_skills | system prompt 始终有列表 |
| 技能激活 | LLM 调 get_skill_detail | LLM 调 read_file |
| 技能执行 | LLM 调 apply_skill | LLM 读完后自己按指令做 |
| 工具数量 | 3 个 | 0 个 |
| 对齐 Claude Code | ❌ | ✅ |

### 5.3 Memory 从工具模式改为 Prompt 模式（在文件工具之上）

**原来的设计（Phase 4.5）：**
- 3 个工具：`remember` / `recall` / `forget`
- `MemoryStore` trait + `FileMemoryStore`
- LLM 调工具来操作记忆

**新的设计（在文件工具之上）：**

有了文件工具后，记忆不需要专用工具。和 Skills 一样——prompt 注入 + LLM 自己读写文件。

```
System prompt:
  ## Memory
  Your persistent memory is stored in .phi/memory/*.md files.
  Use read_file / write_file to manage them.
  - MEMORY.md lists all memories with one-line descriptions.
  - Each memory is a separate .md file with frontmatter.

LLM 想记住东西:
  → 调 read_file(".phi/memory/MEMORY.md")   // 先看索引
  → 调 write_file(".phi/memory/some-fact.md")  // 写新记忆
  → 调 write_file(".phi/memory/MEMORY.md")  // 更新索引
```

这完全就是 Claude Code Memory 的做法——`~/.claude/memory/` 目录，markdown 文件，LLM 自己管理。

**改动：**
- agent-works: `MemoryStore` trait 保留（高级用户接 PostgreSQL），但不默认注册工具
- phi-kernel-tools: 不加 `remember`/`recall`/`forget` 工具
- phi-agent: system prompt 注入 memory 指令 + 目录引导

### 5.4 Multi-Agent 是否也要改？

**不改。** Multi-agent 和文件工具是不同层次的能力：

- 文件工具 = LLM 的"感官"，操作外部世界（文件系统）
- Multi-agent = LLM 的"分身"，操作内部世界（Agent 生命周期）

spawn/send/wait 这些是 Agent 管理，不是文件操作。6 个工具保持不动。

### 5.5 用户视角变化

```bash
# 有文件工具之前：每种能力配 feature
cargo run --features skill,memory,multi-agent

# 有文件工具之后：file 一个 feature 解锁技能+记忆
# skill 和 memory 变成 prompt 注入，不再是 tools
cargo run --features file,multi-agent
```

```toml
# phi-agent/Cargo.toml
[features]
default = ["telemetry", "logging", "file", "shell"]
full = ["file", "shell", "multi-agent", "browser", "mcp", "telemetry", "logging"]
file = ["phi-kernel-tools/file"]  # 文件工具 + skill/memory prompt 注入
```

---

### 5.6 实现步骤

| 步骤 | 仓库 | 内容 | 状态 |
|------|------|------|------|
| 0 | phi-agent | **Phase 4 收尾：** CLI `phi serve` MCP Server 入口 (stdio + HTTP) | ✅ 已完成 |
| 1 | phi-kernel-tools | 实现 read_file / write_file / list_files | ✅ 已完成 |
| 2 | phi-agent | builder.rs 注册 file tools + feature gate | ✅ 已完成 |
| 3 | agent-works | system prompt 加入 skill list（LazySkillPrompter 默认化） | ✅ 已完成 |
| 4 | phi-kernel-tools + phi-agent | 删除旧的 skill 工具和 feature | ✅ 已完成 |
| 5 | agent-works + phi-agent | system prompt 加入 memory 指令 | ✅ 已完成 |
| 6 | 测试 + 示例 | 集成测试 + `mcp_server`/`file_ops` example + CHANGELOG 更新 | ✅ 已完成 |

**产出:** CLI `phi serve` MCP 入口 + phi-kernel-tools 3 个 file 工具 + Skills 降为 prompt 注入 + Memory 降为 prompt 注入

---

## Phase 6: 开发者工具 + 生态（v0.6.0）

> **目标:** REPL 调试体验、phi-extra crate、会话管理。**工期: 2-3 周。**

### 6.1 REPL 调试命令

- `/events` — 实时事件流
- `/session` — 当前会话上下文
- `/tools` — 已注册工具列表

### 6.2 记忆模板（phi-tools）

记忆文件 `.md` 模板放到 `phi-tools/templates/memory/`，不新建 crate。

- 项目记忆模板、编码规范模板、用户偏好模板等预制 `.md` 文件
- LLM 创建新记忆时可以直接 `read_file` 模板参考格式
- 不进 agent-base / agent-works，纯数据文件

> 注：OTel 辅助、工具函数等需求明确后再考虑是否建 phi-extra，Phase 6 不提前建空 crate。

### 6.3 会话快照

- 导出/导入会话状态、状态 diff

### 6.4 混合架构 Demo

`examples/advanced/hybrid_langgraph.rs` — LangGraph 通过 MCP 调用 phi-agent。

### 6.5 实现步骤

| 步骤 | 仓库 | 内容 | 状态 |
|------|------|------|------|
| 1 | phi-agent | REPL 新增 `session`/`events`/`snapshot`/`snapshots` 命令 | ✅ 已完成 |
| 2 | phi-tools | 记忆模板 `.md` 文件 + memory prompt 更新 | ✅ 已完成 |
| 3 | phi-agent | 会话快照（create/list/restore/delete） | ✅ 已完成 |
| 4 | phi-agent | `hybrid_langgraph` example | ✅ 已完成 |

**产出:** REPL 调试命令 + 记忆模板 + 会话快照 + 混合 Demo

---

## 评审 & 修复（Phase 5-6 收尾）

代码评审发现 3 个 bug，已全部修复：

| # | 问题 | 修复 |
|---|------|------|
| 1 | `run.rs` 中 `parent().unwrap().parent().unwrap()` 有 panic 风险 | `SessionContext` 新增 `base_dir` 字段，`resolve_session`/`restore_snapshot` 自动填充 |
| 2 | snapshot 名字无校验，存在路径穿越风险 | 新增 `validate_snapshot_name()` 函数，`create_snapshot`/`restore_snapshot`/`delete_snapshot` 入口统一校验 |
| 3 | `metadata_path()` 上的 `#[allow(dead_code)]` 已过时 | 删除注解 |

**测试覆盖:** 新增 12 个 snapshot 单元测试（validate_snapshot_name ×3 / create_snapshot ×2 / list_snapshots ×2 / restore_snapshot ×2 / delete_snapshot ×2 / base_dir ×1），全量 138 tests pass，fmt + clippy clean。

---

## Phase 7: 性能 & 稳定（v1.0）

> **目标:** 压测、API 冻结、正式版发布。**按条件触发，不急。**

- 高并发压测 + 基准数据
- API 冻结，无 pending breaking changes
- MCP 集成测试完备
- 所有 trait 文档齐全

> 1.0 不代表无所不能。依然不做图编排引擎。

---

## 总览

```
Phase 1: 修倒退              3-5 天 ✅ 已完成    phi-agent 层 anyhow → AgentResult
Phase 2: 文档 & 示例          3-5 天 ✅ 已完成    examples 重组 + README + CONTRIBUTING
Phase 3: Multi-Agent         2-3 周 ✅ 已完成    agent-works 基础设施 + phi-kernel-tools 6 个 Tool + phi-agent Builder 集成
Phase 4: MCP Server + Skills  3-4 周 ✅ 已完成    agent-works MCP Server + Skills 对齐 agentskills.io + 事件字段 + hot-reload
Phase 5: 文件系统工具         3-4 周 ✅ 已完成    phi-kernel-tools 3 file tools + Skills/Memory 降为 prompt 注入
Phase 6: 工具 + 生态          2-3 周 ✅ 已完成    REPL 命令 + 记忆模板 + 会话快照 + 混合 Demo
Phase 7: 性能 & 稳定          3-5 天 ✅ 已完成    压测 + API 冻结 + v0.9.0
```

Phase 1-7 全部完成。版本号: 0.9.0。

---

## 与生态路线图的关系

| 文件 | 定位 | 受众 |
|------|------|------|
| `framework-roadmap.md` (本文) | phi-agent **框架**改进路线图 | 开源社区、框架用户 |
| `ecosystem-roadmap.md` | phi-agent **生态产品**路线图 | 内部、phi-bard/aap/phi-dash 用户 |

并行推进，互不阻塞。

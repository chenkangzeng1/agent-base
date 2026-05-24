# Harness 架构设计

> **最后更新**: 2026-05-24
>
> 本文档描述 agent-base 作为 Agent Harness（编排内核）的架构演进方向。
> 原始版本（2026 年初）以"待办"形式起草了 PlanOrchestrator / AutoContinueTool 等下沉计划，
> 目前已部分完成。本文档整合了最新代码状态、与 skilllite / Codex 的对比分析、
> 以及后续架构决策（agent-works 拆分、MCP / Skills / CLI 等增强方向）。
>
> 增强层的详细设计见 **[agent-works.md](./agent-works.md)**。

---

## 1. 目标

将 `agent-base` 打造成一个**纯粹的、领域无关的 Agent ReAct 运行时**，负责：

- **编排** (Orchestration)：ReAct 循环、Tool Calling、Plan 生成/执行的生命周期、事件流
- **记忆** (Memory)：Session 管理、消息历史、上下文窗口、经验存储
- **进化** (Evolution)：Reflexion 反思、失败恢复、策略优化、技能提炼
- **安全** (Safety)：审批、工具策略

`ops-agent` 退化为**SSH 运维领域的 Trait 实现 + 领域工具**。

`harness`（未来的代码工程助手）基于 `agent-works`（agent-base + MCP + Skills + CLI），
注册自己的领域工具和 Trait 实现。

---

## 2. 当前代码状态（2026-05-24）

### 2.1 三层关系

```
ops-copilot (Tauri Desktop App)
    ├── 前端: React + TypeScript
    └── 后端: Tauri commands
            │
            ├── agent-base (纯 ReAct 运行时)  ← Harness 内核
            │   ✅ ReAct 循环, LLM 抽象 (OpenAI / Anthropic), Tool 系统
            │   ✅ Session 管理, 事件总线 (broadcast channel)
            │   ✅ PlanOrchestrator + PlanExecTool（已完成下沉）
            │   ✅ AutoContinueTool（已完成下沉）
            │   ✅ SubAgentTool（Ephemeral / Persistent）
            │   ✅ Middleware 管道（UserMessage / PreLlm / PostLlm）
            │   ✅ 审批体系（ApprovalHandler + ToolPolicy）
            │   ✅ ReflexionHandler trait
            │   ✅ Checkpoint / Resume
            │   ✅ Skill trait + SkillPrompter trait（抽象接口）
            │   ⚠️ MCP 基础代码（McpClient / McpToolRegistry）仍在 agent-base — 待迁出到 agent-works
            │   ⚠️ Skill 具体实现（LazySkillPrompter / SkillDetailTool）仍在 agent-base — 待迁出
            │   ⚠️ AntiHallucinationMiddleware 仍在 ops-agent
            │
            ├── ops-agent (运维 Agent 库)
            │   ✅ SshCommandTool, TerminalManager, InteractiveTask
            │   ✅ OpsPlanExecutor (impl PlanGenerator + StepExecutor + RecoveryStrategy)
            │   ✅ OpsToolPolicy
            │   ⚠️ AntiHallucinationMiddleware（通用，零 SSH 依赖）
            │   ⚠️ CLI main.rs（事件打印机逻辑通用，可下沉）
            │
            └── ssh-simple (SSH 协议层)
```

### 2.2 已解决 / 仍存在的问题

| # | 问题 | 状态 |
|---|------|------|
| 1 | Plan 编排逻辑在 ops-agent | ✅ 已解决 — PlanOrchestrator / PlanExecTool 在 agent-base |
| 2 | OpsPlanExecutor 半耦合 | ✅ 设计上正确 — 它就是 StepExecutor trait 的 SSH 领域实现 |
| 3 | PlanGenerator prompt 硬编码 | ✅ 设计上正确 — prompt 由领域实现决定 |
| 4 | AutoFixTool 在 ops-agent | ✅ 已解决 — AutoContinueTool 在 agent-base |
| 5 | PlanTool definition 带运维语义 | ✅ 已解决 — PlanOrchestrator schema 通用 |
| 6 | Tauri 后端穿透 ops-agent | ⚠️ 仍然存在 |
| 7 | AntiHallucinationMiddleware 在 ops-agent | ⚠️ 待搬到 agent-works |
| 8 | MCP 代码在 agent-base 概念不纯 | ⚠️ 待迁出到 agent-works |
| 9 | Skill 具体实现在 agent-base 概念不纯 | ⚠️ 待迁出到 agent-works |
| 10 | 无内置通用工具 | ❌ 待做（在 agent-works） |
| 11 | 无 CLI REPL 抽象 | ❌ 待做（在 agent-works） |

---

## 3. 架构决策：agent-base × agent-works 双层结构

### 3.1 决策

**创建独立的 `agent-works` crate，将 MCP、Skill 实现、内置工具、反幻觉、CLI 等增强模块
从 agent-base 中迁出。agent-base 只保留纯 ReAct 运行时 + trait 接口。**

agent-works 通过 `pub use agent_base::*` re-export 所有 agent-base 类型，
业务方只需 `cargo add agent-works`。

### 3.2 为什么要拆分

| 理由 | 说明 |
|------|------|
| **概念纯度** | MCP / Skill 是工具来源，不是工具编排。agent-base 管编排，agent-works 管工具来源和体验 |
| **依赖控制** | MCP（rmcp, 子进程管理）、Skill（YAML/文件监控）、CLI 都会拉入更多依赖。保持 agent-base 轻量 (~12 deps) |
| **心理门槛** | `cargo add agent-base` 不需要解释"为什么依赖 rmcp" |
| **版本独立** | agent-base 核心 API 应稳定；agent-works 可以频繁迭代 breaking changes |
| **测试矩阵** | 两个 crate 独立测试，不交叉爆炸 |
| **生态防震** | MCP 被 A2A 取代、Skills 概念过时 → 只改 agent-works，agent-base 纹丝不动 |

### 3.3 不拆的理由曾经存在但被排除

之前考虑的"feature flags"方案的问题：
- Cargo 仍然要解析所有可选依赖
- 新用户看到 agent-base 的 Cargo.toml 会困惑
- feature 组合多（5 个 feature = 32 种组合）
- "agent-base 究竟是 runtime 还是平台"的认知模糊

### 3.4 拆分边界

```
留在 agent-base（纯运行时）           迁到 agent-works（增强层）
─────────────────────────────────  ─────────────────────────────────
ReAct Loop + Tool Calling          McpHUb (多 server + stdio + HTTP)
EventBus + SessionManager          SkillLoader (目录扫描)
LLM 抽象 (OpenAI / Anthropic)      LazySkillPrompter / FullDetailPrompter
Tool trait + ToolRegistry          SkillDetailTool
PlanOrchestrator + PlanExecTool    AntiHallucinationMiddleware
AutoContinueTool                   Builtin tools (read_file / write_file / ...)
SubAgentTool                        CLI Repl / CliEventPrinter
Middleware 管道                      McpClient / McpToolRegistry（迁出）
Approval 体系
ReflexionHandler trait
Skill trait（仅接口定义）
SkillPrompter trait（仅接口定义）
AgentBuilder.register_skill()
Checkpoint / Resume
```

---

## 4. 目标架构

```
┌─────────────────────────────────────────────────────────────────┐
│                     agent-base (纯运行时)                         │
│                                                                  │
│  AgentRuntime / ReAct Loop / Tool Calling                       │
│  EventBus (broadcast) / SessionManager / SessionStore           │
│  LLM 抽象 (LlmClient trait, OpenAI + Anthropic 实现)              │
│  Tool trait + ToolRegistry + ToolEngine                         │
│  Skill trait + SkillPrompter trait（仅接口，无实现）               │
│  PlanOrchestrator + PlanExecTool (Plan as Tool)                 │
│  AutoContinueTool / SubAgentTool                                │
│  Middleware 管道 / Approval 体系                                  │
│  ReflexionHandler trait / Checkpoint / Resume                   │
│                                                                  │
│  依赖: tokio, reqwest, serde, async-trait, tracing, uuid (~12)    │
└──────────────┬──────────────────────────────────────────────────┘
               │
               │ depends on
               ▼
┌─────────────────────────────────────────────────────────────────┐
│                     agent-works (增强层)                          │
│                                                                  │
│  pub use agent_base::*   (re-export 全部 agent-base 类型)        │
│                                                                  │
│  McpHUb: 多 server 聚合, stdio + HTTP transport, 生命周期管理     │
│  SkillLoader: 目录扫描, SKILL.md 解析, 渐进式披露                 │
│  LazySkillPrompter / FullDetailPrompter / SkillDetailTool        │
│  AntiHallucinationMiddleware (可配置)                             │
│  Builtin tools: read_file / write_file / list_directory / ...    │
│  CLI Repl / CliEventPrinter                                      │
│                                                                  │
│  依赖: agent-base, rmcp, walkdir, + 子进程/文件监控                │
└──────────────┬──────────────────────────────────────────────────┘
               │
               │ depends on (domain = agent-base or agent-works)
               ▼
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
│   ops-agent      │  │    harness       │  │   future...      │
│                  │  │                  │  │                  │
│ depends on:      │  │ depends on:      │  │ depends on:      │
│ agent-base       │  │ agent-works      │  │ agent-works      │
│ ssh-simple       │  │                  │  │                  │
│                  │  │                  │  │                  │
│ SSH 工具:         │  │ Domain 工具:      │  │ Domain 工具:      │
│ SshCommandTool   │  │ (待定)            │  │ (待定)            │
│ InteractiveTask  │  │                  │  │                  │
│ ListHostsTool    │  │ Trait 实现:        │  │ Trait 实现:        │
│                  │  │ PlanGenerator     │  │ PlanGenerator     │
│ Trait 实现:       │  │ StepExecutor      │  │ StepExecutor      │
│ OpsPlanExecutor  │  │ RecoveryStrategy  │  │ RecoveryStrategy  │
│ OpsToolPolicy    │  │ ToolPolicy        │  │ ToolPolicy        │
│                  │  │                  │  │                  │
│ 依赖选择:         │  │ 依赖选择:          │  │                  │
│ agent-base 极简   │  │ agent-works 全量  │  │                  │
│ 不需要 MCP/Skills │  │ 需要一切增强能力    │  │                  │
└──────────────────┘  └──────────────────┘  └──────────────────┘
```

### 4.1 三种使用模式

```toml
# 模式 1: 极简 agent — 只用 core runtime，自己做所有事
[dependencies]
agent-base = "0.1"

# 模式 2: 标准 agent — 要 MCP 但不需要 Skills/CLI
[dependencies]
agent-works = { version = "0.1", default-features = false, features = ["mcp"] }

# 模式 3: 全量 agent — 所有增强能力
[dependencies]
agent-works = { version = "0.1", features = ["full"] }
```

---

## 5. 已完成的下沉（agent-base 已有）

以下模块原在 ops-agent，现已全部完成下沉到 agent-base：

### 5.1 PlanOrchestrator + PlanExecTool

位于 `agent-base/src/engine/plan_orchestrator.rs`。

```rust
pub struct PlanOrchestrator {
    plan_generator: Arc<dyn PlanGenerator>,
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
}

pub struct PlanExecTool {
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
    recovery: Arc<dyn RecoveryStrategy>,
}
```

两者接收 Trait 实现，自身不关心 step 是什么。

### 5.2 AutoContinueTool

位于 `agent-base/src/tool/auto_continue.rs`。

纯编排信号 tool，零领域依赖。

### 5.3 PlanGenerator / StepExecutor / RecoveryStrategy traits

位于 `agent-base/src/engine/plan.rs`。

`StreamingJsonParser<T>` 也已内置，领域实现可复用。

### 5.4 SubAgentTool

位于 `agent-base/src/tool/subagent.rs`。

Ephemeral / Persistent 两种策略。子 Agent 拥有独立 ReAct 循环和工具集。

### 5.5 Skill trait + SkillPrompter trait（保留在 agent-base）

位于 `agent-base/src/skill/`。

```rust
pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn brief_description(&self) -> String;
    fn detailed_description(&self) -> String;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}

pub trait SkillPrompter: Send + Sync {
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String;
}
```

`AgentBuilder.register_skill()` 依赖 `Skill` trait 做编排（注册 tools + 注入 prompt），
这部分逻辑是纯运行时编排，保留在 agent-base。

**具体实现**（`LazySkillPrompter`、`FullDetailPrompter`、`SkillDetailTool`）
迁到 agent-works。

---

## 6. 待迁出 / 待新增模块（在 agent-works）

### 6.1 MCP — 从 agent-base 迁出 + 增强为 McpHUb

**现状**：`McpClient` / `McpToolRegistry` / `McpToolAdapter` 在 `agent-base/src/tool/mcp.rs`。

**动作**：
1. 将 MCP 代码全部迁到 agent-works
2. 在 agent-works 新建 `McpHUb`，支持：
   - 多 server 聚合
   - stdio + HTTP transport
   - 连接生命周期（connect / disconnect / health check）
   - 批量注册到 ToolRegistry

```rust
// agent-works/src/mcp/hub.rs
pub enum McpTransport {
    Http { url: String },
    Stdio { command: String, args: Vec<String> },
}

pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    pub auto_reconnect: bool,
}

pub struct McpHUb { servers: Vec<McpServerHandle> }

impl McpHUb {
    pub fn new() -> Self;
    pub fn add_server(&mut self, config: McpServerConfig);
    pub async fn connect_all(&mut self) -> AgentResult<()>;
    pub async fn discover_all(&mut self) -> AgentResult<Vec<(String, Vec<McpToolInfo>)>>;
    pub fn register_all(&self, registry: &mut ToolRegistry);
}
```

### 6.2 SkillLoader — agent-works 新增

```rust
// agent-works/src/skill/loader.rs
pub struct LoadedSkill {
    pub metadata: SkillMetadata,
    pub tools: Vec<Arc<dyn Tool>>,
    pub full_prompt: String,
    pub brief_prompt: String,
    pub skill_dir: PathBuf,
}

pub struct SkillLoader { skill_dirs: Vec<PathBuf>, watch: bool }

impl SkillLoader {
    pub fn new() -> Self;
    pub fn add_dir(&mut self, dir: PathBuf);
    pub fn with_watch(mut self, watch: bool) -> Self;
    pub fn discover(&self) -> AgentResult<Vec<LoadedSkill>>;
    pub fn watch_changes(&self) -> Option<Receiver<SkillChangeEvent>>;
}
```

### 6.3 AntiHallucinationMiddleware — 从 ops-agent 搬到 agent-works

现状在 `ops-agent/src/middleware/anti_hallucination.rs`，零 SSH 依赖。

做成可配置：

```rust
// agent-works/src/middleware/anti_hallucination.rs
pub struct AntiHallucinationConfig {
    pub max_nudges: usize,          // 默认 3
    pub nudge_message: String,      // 默认中英双语
    pub first_turn_only: bool,      // 默认 true
    pub min_tools_threshold: usize, // 默认 1
}

pub struct AntiHallucinationMiddleware { config, nudge_count }
```

### 6.4 内置通用工具 — agent-works 新增

```rust
// agent-works/src/builtin/
pub struct ReadFileTool    { workspace: PathBuf }
pub struct WriteFileTool   { workspace: PathBuf }
pub struct ListDirectoryTool { workspace: PathBuf }
pub struct FileExistsTool  { workspace: PathBuf }
pub struct SearchReplaceTool { workspace: PathBuf }
```

### 6.5 CLI 工具集 — agent-works 新增

```rust
// agent-works/src/cli/
pub struct CliEventPrinter {
    custom_handlers: Vec<Box<dyn Fn(&AgentEvent) -> Option<String>>>,
}

pub struct CliRepl { runtime, session_id, shell_commands }

impl CliRepl {
    pub fn new(runtime: AgentRuntime) -> Self;
    pub fn register_shell_command(&mut self, prefix: &str, cmd: ShellCommand);
    pub async fn run(&mut self) -> Result<()>;
}
```

---

## 7. 竞品对比与借鉴

### 7.1 Skilllite ReAct Runtime

| 维度 | agent-base 做法 | skilllite 做法 | 借鉴 |
|------|----------------|---------------|------|
| 事件模型 | EventBus (broadcast)，多消费者 | EventSink trait，单一回调 | agent-base 更灵活 |
| 规划方式 | Plan as Tool，人机协作 | TaskPlanner 内建，全自动 | 两种模式不冲突，agent-base 可选装配 |
| 反幻觉 | agent-works Middleware（可配置） | 多层防护（流式抑制/完成验证/nudge/回滚） | agent-works 负责第一层 |
| 渐进式披露 | Skill trait + SkillDetailTool + LazySkillPrompter（agent-works） | inject_progressive_disclosure 直接操作 messages | agent-works 已有框架，缺 SkillLoader |
| 上下文溢出 | 无 | 自动检测 + 截断 + 重试 3 次 | 可纳入 agent-base 的 LLM retry/config |
| 工具结果处理 | 截断 | 截断 + LLM 摘要（MapReduce） | 可考虑作为可选能力 |

### 7.2 OpenAI Codex (codex-rs)

详细的改进分析见 **[implrove-from-codex.md](./implrove-from-codex.md)**。

核心发现：

| 特性 | Codex 做法 | agent-base 现状 | 优先级 |
|------|-----------|----------------|--------|
| Auto-Compaction | LLM 摘要压缩历史对话 | ❌ 无 | P0 |
| Parallel Tools | FuturesUnordered 并发执行 | ❌ 串行 for 循环 | P0 |
| Steer Input | 中轮注入 pending input | ❌ 无 | P1 |
| Turn 前置处理 | Skills/MCP deps 解析 + 注入 | ❌ 无 | P1 |
| Contextual Fragment | Developer role 分段注入上下文 | ⚠️ role 已有但未用 | P2 |
| 分层工具执行 | 审批 → 沙箱 → 执行 → 重试 | ⚠️ 仅审批层 | P2 |
| Multi-Agent 通信 | Mailbox + InterAgentCommunication | ⚠️ 仅 SubAgentTool | P3 |
| TurnDiffTracker | 文件变更追踪 + Undo | ❌ 无 | 放 harness 层 |

关键区别：Codex 深度耦合 OpenAI Responses API（34 个内部 crate），不适合直接依赖，但工程实践值得借鉴。

---

## 8. 迁移步骤

| 阶段 | 内容 | 状态 | 位置 |
|------|------|------|------|
| **Phase 1** | PlanGenerator::generate_plan_streaming 提升为 trait 默认方法 | ✅ 已完成 | agent-base |
| **Phase 2** | PlanOrchestrator + PlanExecTool 在 agent-base | ✅ 已完成 | agent-base |
| **Phase 3** | AutoContinueTool 搬到 agent-base | ✅ 已完成 | agent-base |
| **Phase 4** | ops-agent 删除旧 PlanTool，改为 re-export | ✅ 已完成 | agent-base |
| **Phase 5** | 创建 agent-works crate，搭建基本结构 | ⬜ 待做 | agent-works |
| **Phase 6** | McpClient / McpToolRegistry / McpToolAdapter 从 agent-base 迁到 agent-works | ⬜ 待做 | agent-works |
| **Phase 7** | LazySkillPrompter / FullDetailPrompter / SkillDetailTool 从 agent-base 迁到 agent-works | ⬜ 待做 | agent-works |
| **Phase 8** | agent-works 实现 McpHUb（多 server + stdio transport） | ⬜ 待做 | agent-works |
| **Phase 9** | agent-works 实现 SkillLoader（目录扫描 + 渐进式披露） | ⬜ 待做 | agent-works |
| **Phase 10** | AntiHallucinationMiddleware 从 ops-agent 搬到 agent-works（可配置） | ⬜ 待做 | agent-works |
| **Phase 11** | agent-works 实现内置通用工具（read_file / write_file / list_dir） | ⬜ 待做 | agent-works |
| **Phase 12** | agent-works 实现 CliRepl / CliEventPrinter | ⬜ 待做 | agent-works |
| **Phase 13** | [Codex] Parallel Tools — `handle_tool_calls` 改为 `FuturesUnordered` | ⬜ 待做 | agent-base |
| **Phase 14** | [Codex] Auto-Compaction — `CompactionStrategy` trait + Middleware | ⬜ 待做 | agent-base |
| **Phase 15** | [Codex] Steer Input — `UserSteerInput` event + `steer_input` API | ⬜ 待做 | agent-base |
| **Phase 16** | agent-base 清理：移除迁出的模块，只保留 Skill/SkillPrompter trait | ⬜ 待做 | agent-base |
| **Phase 17** | ops-agent main.rs 改用 agent-works 的 CliRepl | ⬜ 待做 | ops-agent |
| **Phase 18** | ops-copilot 适配新架构（agent-works 或 agent-base） | ⬜ 待做 | ops-copilot |

---

## 9. 不做的事（明确边界）

- ❌ agent-base 不引入 ssh-simple 等外部领域依赖
- ❌ agent-base 不定义 `HostInfo` 等运维/业务类型
- ❌ PlanOrchestrator 的 tool definition 不包含任何领域语义
- ❌ agent-base 不包含 MCP / Skill 的具体实现（只有 trait 接口）
- ❌ agent-base 不包含 CLI / builtin tools / anti-hallucination
- ❌ 不在 agent-base 中内建 task planning（skilllite 风格），保持 Plan as Tool 的可选模式
- ❌ 不引入 codex-core 作为依赖
- ❌ agent-works 不引入 ssh-simple 等任何领域依赖
- ✅ ops-agent 的 CLI main.rs 保留作为调试入口
- ✅ `agent-works` pub use agent_base::* re-export 全部类型，用户只需依赖 agent-works

# agent-works 架构设计

> **创建日期**: 2026-05-24
>
> `agent-works` 是 `agent-base` 的**开箱即用增强层**。它站在 agent-base 的纯 ReAct 运行时之上，
> 提供 MCP 多 server 管理、Skills 目录扫描与渐进式披露、内置通用工具、反幻觉中间件、
> CLI REPL 等"装了电池"的能力。

---

## 1. 定位

```
agent-base = 纯运行时内核（~12 deps，只定义 trait 接口）
agent-works = 开箱即用的 Agent 工具箱（代理 agent-base 全部 API + 增强模块）
```

- **对业务 agent 开发者**：直接依赖 `agent-works`，拿到一切
- **对 ops-agent**：如果只需要运行时，继续只依赖 `agent-base`；如果将来需要 MCP/Skills，切到 `agent-works`
- **对 embedded/wasm**：只依赖 `agent-base`，零额外开销

---

## 2. 依赖关系

```
agent-base         (~12 deps: tokio, reqwest, serde, async-trait, tracing, uuid)
    ↑
    │ depends on
    │
agent-works        (~20 deps: agent-base, rmcp, walkdir, notify, etc.)
    ↑
    │ depends on (通常只依赖这一个)
    │
┌───────────────────┐  ┌───────────────────┐
│    harness        │  │   未来业务 agent    │
│ (代码工程助手)      │  │                   │
└───────────────────┘  └───────────────────┘
```

agent-works 通过 `pub use agent_base::*` re-export 所有 agent-base 类型，
业务方只需 `cargo add agent-works`。

---

## 3. 模块结构

```
agent-works/
├── Cargo.toml
├── src/
│   ├── lib.rs                   # pub use agent_base::*; + feature-gated mod
│   │
│   ├── builder.rs               # AgentBuilder 封装: 转发 agent-base API + skill 集成
│   │
│   ├── mcp/                     # [feature: mcp]
│   │   ├── mod.rs
│   │   ├── hub.rs               # McpHUb: 多 server 聚合 & 生命周期管理
│   │   ├── client.rs            # McpClient: HTTP + stdio transport（基于 rmcp 重写）
│   │   └── types.rs             # McpToolInfo, McpServerConfig
│   │
│   ├── skill/                   # [feature: skill]
│   │   ├── mod.rs               # Skill trait + SkillPrompter trait 定义（从 agent-base 迁入）
│   │   ├── loader.rs            # SkillLoader: 目录扫描 & SKILL.md 解析
│   │   ├── prompter.rs          # LazySkillPrompter / FullDetailPrompter（从 agent-base 迁入）
│   │   └── detail_tool.rs       # SkillDetailTool（从 agent-base 迁入）
│   │
│   ├── builtin/                 # [feature: builtin-tools]
│   │   ├── mod.rs
│   │   ├── read_file.rs         # ReadFileTool
│   │   ├── write_file.rs        # WriteFileTool
│   │   ├── list_directory.rs    # ListDirectoryTool
│   │   ├── file_exists.rs       # FileExistsTool
│   │   └── search_replace.rs   # SearchReplaceTool
│   │
│   └── cli/                     # [feature: cli]
│       ├── mod.rs
│       ├── printer.rs           # CliEventPrinter: 通用终端事件打印
│       └── repl.rs              # CliRepl: 通用 REPL 循环 + 可注册自定义 shell 命令
```

---

## 4. Feature Flags

```toml
# agent-works/Cargo.toml
[features]
default = []

# 独立 feature（互相无交叉依赖）
mcp = []                       # McpHUb: MCP 多 server 管理
skill = []                     # Skill trait + prompter + detail_tool + SkillLoader
builtin-tools = ["walkdir"]    # 内置通用工具（需要 walkdir 做文件遍历）
cli = []                       # CLI REPL（纯运行时，无额外依赖）

# 组合 feature（推荐使用）
full = ["mcp", "skill", "builtin-tools", "cli"]

[dependencies]
agent-base = { path = "../agent-base" }
rmcp = { version = "0.6", optional = true }       # MCP 协议 (mcp feature)
walkdir = { version = "2", optional = true }       # 目录遍历 (builtin-tools / skill)
notify = { version = "6", optional = true }         # 文件监控 (skill feature 热加载)
serde_json = "1"
tokio = { version = "1.52", features = ["full"] }
```

---

## 5. 各模块详细设计

### 5.1 McpHUb（MCP 多 server 聚合）

**从 agent-base 迁出**：`McpClient`、`McpToolRegistry`、`McpToolAdapter` 从 agent-base 移除。
**重写**：`McpClient` 基于 `rmcp` crate 重写，原生支持 HTTP + stdio 双 transport。
**新增**：`McpHUb` 作为多 server 管理器。

```rust
// agent-works/src/mcp/hub.rs

pub enum McpTransport {
    Http { url: String },
    Stdio { command: String, args: Vec<String> },
}

pub struct McpServerConfig {
    pub name: String,             // 唯一标识
    pub transport: McpTransport,
    pub auto_reconnect: bool,     // 断连自动重连
}

enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

struct McpServerHandle {
    config: McpServerConfig,
    client: Box<dyn McpTransportHandler>,
    tools: Vec<McpToolInfo>,
    state: ConnectionState,
}

pub struct McpHub {
    servers: Vec<McpServerHandle>,
}

impl McpHub {
    pub fn new() -> Self;
    pub fn add_server(&mut self, config: McpServerConfig);

    /// 并发连接所有 server
    pub async fn connect_all(&mut self) -> AgentResult<()>;

    /// 并发发现所有 server 的工具列表
    pub async fn discover_all(&mut self)
        -> AgentResult<Vec<(String, Vec<McpToolInfo>)>>;

    /// 将所有 server 的工具注册到 ToolRegistry
    pub fn register_all(&self, registry: &mut ToolRegistry);

    /// 健康检查
    pub async fn health_check(&self) -> Vec<ServerHealth>;

    /// 按名称查找工具所属的 server
    pub fn find_tool(&self, tool_name: &str) -> Option<&McpServerHandle>;

    /// 断开所有 server
    pub async fn disconnect_all(&mut self);
}
```

### 5.2 Skill 系统（trait 定义 + 加载器 + 渐进式披露）

**从 agent-base 迁出全部 skill 模块**：`Skill` trait、`SkillPrompter` trait、`LazySkillPrompter`、`FullDetailPrompter`、`SkillDetailTool` 全部迁到 agent-works。agent-base 彻底移除此概念。

**Skill trait 定义**（`skill/mod.rs`）：

```rust
// agent-works/src/skill/mod.rs

/// Skill - a reusable capability unit
///
/// Each Skill declares:
/// - Brief description (resident in system prompt)
/// - Detailed description (loaded on demand)
/// - Tool collection (auto-registered by agent-works builder)
pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn brief_description(&self) -> String;
    fn detailed_description(&self) -> String;
    fn tools(&self) -> Vec<Arc<dyn agent_base::Tool>>;

    fn version(&self) -> &'static str { "0.1.0" }
    fn tags(&self) -> &[&'static str] { &[] }
    fn author(&self) -> &'static str { "" }
}

/// Prompt injection strategy
///
/// Generates prompt text to inject into the system prompt based on registered skills.
pub trait SkillPrompter: Send + Sync {
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String;
}
```

**SkillLoader**（`skill/loader.rs`）：

```rust
// agent-works/src/skill/loader.rs

pub struct SkillMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub entry_point: String,
}

pub struct LoadedSkill {
    pub metadata: SkillMetadata,
    pub full_prompt: String,       // SKILL.md 完整内容
    pub brief_prompt: String,      // 摘要 (用于渐进式披露)
    pub skill_dir: PathBuf,
}

pub struct SkillLoader {
    skill_dirs: Vec<PathBuf>,
    /// 热加载: 是否监控 skill 目录变化
    watch: bool,
}

impl SkillLoader {
    pub fn new() -> Self;
    pub fn add_dir(&mut self, dir: PathBuf);
    pub fn with_watch(mut self, watch: bool) -> Self;

    /// 扫描所有目录，发现 .skills/*/SKILL.md
    pub fn discover(&self) -> AgentResult<Vec<LoadedSkill>>;

    /// 如果启用了 watch，返回一个流用于监听 skill 变化事件
    pub fn watch_changes(&self) -> Option<tokio::sync::mpsc::Receiver<SkillChangeEvent>>;
}

pub enum SkillChangeEvent {
    Added { name: String },
    Removed { name: String },
    Modified { name: String },
}
```

### 5.3 AgentBuilder（agent-base Builder 的封装）

agent-works 提供自己的 `AgentBuilder`，转发 agent-base 的全部 API 并扩展 skill 集成能力。
切换到 agent-works 时，业务代码只需改一行 `use`。

```rust
// agent-works/src/builder.rs

pub struct AgentBuilder {
    inner: agent_base::AgentBuilder,
    skills: Vec<Arc<dyn Skill>>,
    skill_prompter: Option<Arc<dyn SkillPrompter>>,
    skill_detail_tool_name: String,
    disable_skill_prompt_injection: bool,
}

impl AgentBuilder {
    pub fn new(client: Arc<dyn LlmClient>) -> Self { ... }

    // 转发 agent-base AgentBuilder 的全部方法
    pub fn system_prompt(self, p: impl Into<String>) -> Self { ... }
    pub fn register_tool(self, t: impl Tool + 'static) -> Self { ... }
    pub fn register_tool_arc(self, t: Arc<dyn Tool>) -> Self { ... }
    pub fn approval_handler(self, h: Arc<dyn ApprovalHandler>) -> Self { ... }
    pub fn tool_policy(self, p: Arc<dyn ToolPolicy>) -> Self { ... }
    pub fn middleware(self, mw: impl Middleware + 'static) -> Self { ... }
    pub fn context_window(self, max_tokens: usize) -> Self { ... }
    pub fn language(self, lang: Language) -> Self { ... }
    pub fn error_recovery(self, r: Arc<dyn ToolErrorRecovery>) -> Self { ... }
    pub fn session_store(self, s: Arc<dyn SessionStore>) -> Self { ... }
    // ... 其余 agent-base builder 方法

    // agent-works 扩展：skill 相关
    pub fn register_skill(self, s: impl Skill + 'static) -> Self { ... }
    pub fn register_skills(self, skills: Vec<Arc<dyn Skill>>) -> Self { ... }
    pub fn skill_prompter(self, p: Arc<dyn SkillPrompter>) -> Self { ... }
    pub fn disable_skill_prompt_injection(self) -> Self { ... }
    pub fn skill_detail_tool_name(self, n: impl Into<String>) -> Self { ... }

    pub fn build(mut self) -> AgentResult<AgentRuntime> {
        // 1. 处理 skills：注册工具 + 检测冲突 + 注入 prompt
        // 2. 默认 SkillPrompter = LazySkillPrompter
        // 3. 自动注册 SkillDetailTool
        // 4. delegate 给 self.inner.build()
    }
}
```

### 5.4 ToolEnforcementMiddleware（工具强制使用）

**已迁入 agent-base**。原名 `AntiHallucinationMiddleware`，源自 ops-agent。
重命名为 `ToolEnforcementMiddleware`——本质是"有工具可用时必须调用"而非"反幻觉"。
agent-works 通过 `pub use agent_base::*` 自动透传。

```rust
// agent-base/src/engine/tool_enforcement.rs

pub struct ToolEnforcementConfig {
    pub max_nudges: usize,
    pub nudge_message: String,
    pub first_turn_only: bool,
    pub min_tools_threshold: usize,
}

impl Default for ToolEnforcementConfig {
    fn default() -> Self {
        Self {
            max_nudges: 3,
            nudge_message: "CRITICAL: You have tools available but did not call any. \
                             Call the appropriate tool NOW. \
                             关键提示：你有可用的工具但没有调用。立即使用工具执行。"
                .to_string(),
            first_turn_only: true,
            min_tools_threshold: 1,
        }
    }
}

pub struct ToolEnforcementMiddleware {
    config: ToolEnforcementConfig,
    nudge_count: AtomicUsize,
}

impl ToolEnforcementMiddleware {
    pub fn new(config: ToolEnforcementConfig) -> Self { ... }
}

#[async_trait]
impl agent_base::Middleware for ToolEnforcementMiddleware {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        // 条件: available_tools >= threshold, !is_tool_call, count < max
        // first_turn_only: 只在本轮还没调过工具时触发
        // 动作: skip_push = true, follow_up_message = nudge_message
    }
}
```

### 5.5 Builtin Tools（内置通用工具）

```rust
// agent-works/src/builtin/read_file.rs
pub struct ReadFileTool { pub workspace: PathBuf }

// agent-works/src/builtin/write_file.rs
pub struct WriteFileTool { pub workspace: PathBuf }

// agent-works/src/builtin/list_directory.rs
pub struct ListDirectoryTool { pub workspace: PathBuf }

// agent-works/src/builtin/file_exists.rs
pub struct FileExistsTool { pub workspace: PathBuf }

// agent-works/src/builtin/search_replace.rs
pub struct SearchReplaceTool { pub workspace: PathBuf }
```

### 5.6 CLI 工具集

```rust
// agent-works/src/cli/repl.rs

pub struct CliRepl {
    runtime: AgentRuntime,
    session_id: SessionId,
    shell_commands: HashMap<String, ShellCommand>,
}

pub enum ShellCommand {
    Reset,
    ReadTask { task_id: u64, lines: usize },
    StopTask { task_id: u64 },
    InputTask { task_id: u64, text: String },
    Custom { handler: Box<dyn Fn(&str) -> bool> },
}

impl CliRepl {
    pub fn new(runtime: AgentRuntime) -> Self;

    /// 注册自定义 shell 命令（如 ops-agent 的 task-read / task-input）
    pub fn register_shell_command(&mut self, prefix: &str, cmd: ShellCommand);

    /// 运行 REPL 循环
    pub async fn run(&mut self) -> AgentResult<()>;
}
```

```rust
// agent-works/src/cli/printer.rs

pub struct CliEventPrinter {
    pub assistant_prefix_printed: bool,
    pub custom_handlers: Vec<Box<dyn Fn(&AgentEvent) -> Option<String>>>,
}

impl CliEventPrinter {
    pub fn new() -> Self;
    pub fn handle(&mut self, event: AgentEvent) -> AgentResult<()>;
    pub fn finish(&mut self);
}
```

---

## 6. 使用方式

### 模式 1：极简（只用 agent-base）

```toml
[dependencies]
agent-base = "0.1"
```

```rust
use agent_base::{AgentBuilder, AgentRuntime, Tool, ...};
```

### 模式 2：全量（agent-works full）

```toml
[dependencies]
agent-works = { version = "0.1", features = ["full"] }
```

```rust
// 只需 import agent-works，拿到一切
use agent_works::{
    // agent-base 的全部类型（通过 pub use agent_base::* 透传）
    AgentRuntime, AgentEvent, Tool, ToolRegistry, ToolOutput,
    AgentResult, SessionId, AgentError,
    PlanOrchestrator, SubAgentTool, LlmClient, OpenAiClient,
    // agent-works 的 AgentBuilder + 增强模块
    AgentBuilder,
    Skill, SkillPrompter, LazySkillPrompter,
    McpHUb, McpServerConfig, McpTransport,
    AntiHallucinationConfig, AntiHallucinationMiddleware,
    ReadFileTool, WriteFileTool, ListDirectoryTool,
    CliRepl, CliEventPrinter,
};

#[tokio::main]
async fn main() -> AgentResult<()> {
    let llm = Arc::new(OpenAiClient::new(api_key, model, None));

    // MCP
    let mut mcp_hub = McpHUb::new();
    mcp_hub.add_server(McpServerConfig { ... });
    mcp_hub.connect_all().await?;

    // Skills（推荐：直接实现 Skill trait + register_skill）
    // 或通过 SkillLoader 从目录加载

    // Build（使用 agent-works 的 AgentBuilder）
    let runtime = AgentBuilder::new(llm)
        .system_prompt("You are a helpful assistant.")
        .register_tool(ReadFileTool { workspace: ".".into() })
        .register_tool(WriteFileTool { workspace: ".".into() })
        .register_skill(MyCustomSkill)
        .middleware(AntiHallucinationMiddleware::new(AntiHallucinationConfig::default()))
        .build()?;

    // MCP tools 批量注册
    let mut tools = runtime.tools_mut();
    mcp_hub.register_all(&mut tools);

    // CLI REPL
    let mut repl = CliRepl::new(runtime);
    repl.run().await?;

    Ok(())
}
```

### 模式 3：按需（只选需要的 feature）

```toml
[dependencies]
agent-works = { version = "0.1", features = ["mcp", "anti-hallucination"] }
```

---

## 7. 实施步骤

| 步骤 | 内容 | 预估 |
|------|------|------|
| **Step 1** | 创建 `agent-works` crate，`cargo init --lib`，加 `agent-base` 依赖 | 小 |
| **Step 2** | `lib.rs` 加 `pub use agent_base::*` re-export，定义 feature flags | 小 |
| **Step 3** | `Cargo.toml` 定义 feature flags 结构和依赖 | 小 |
| **Step 4** | 从 agent-base 迁出 `McpClient` / `McpToolRegistry` / `McpToolAdapter` | 中 |
| **Step 5** | 基于 `rmcp` 重写 `McpClient`，实现 `McpHUb`（多 server 聚合 + HTTP/stdio transport + 生命周期） | 中 |
| **Step 6** | 从 agent-base 迁出 `Skill` trait / `SkillPrompter` trait / `LazySkillPrompter` / `FullDetailPrompter` / `SkillDetailTool` | 中 |
| **Step 7** | 实现 `AgentBuilder` 封装（转发 agent-base API + skill prompt 注入 + 自动注册 SkillDetailTool） | 中 |
| **Step 8** | 实现 `SkillLoader`（目录扫描 + SKILL.md 解析 + 热加载） | 中 |
| **Step 9** | 实现内置通用工具（`ReadFileTool` 等） | 中 |
| **Step 10** | 从 ops-agent 抽取通用 `CliEventPrinter`（去除 ops 领域逻辑） | 小 |
| **Step 11** | 实现 `CliRepl`（通用 REPL 循环 + 可注册自定义 shell 命令） | 中 |
| **Step 12** | agent-base 清理：移除 `src/tool/mcp.rs`、`src/skill/` 整个模块、`AgentBuilder` 中 skill 相关字段/方法 | 中 |
| **Step 13** | `AntiHallucinationMiddleware` 迁入 agent-base，重命名为 `ToolEnforcementMiddleware` | 小 |
| **Step 14** | ops-agent main.rs 改用 `agent-works` 的 `CliRepl` 和 `AgentBuilder` | 小 |

---

## 8. agent-base 同步变化

迁出完成后，agent-base 的变化：

```
移除:
  src/tool/mcp.rs              → agent-works/src/mcp/
  src/skill/mod.rs             → agent-works/src/skill/mod.rs（Skill trait + SkillPrompter trait）
  src/skill/prompter.rs        → agent-works/src/skill/prompter.rs
  src/skill/detail_tool.rs     → agent-works/src/skill/detail_tool.rs
  engine/builder.rs            → 删除 register_skill() / skill_prompter() / disable_skill_prompt_injection()
                                 / skill_detail_tool_name() 方法，删除 skills / skill_prompter /
                                 skill_detail_tool_name / disable_skill_prompt_injection 字段，
                                 删除 build() 中的 skill prompt 注入和 SkillDetailTool 注册逻辑
  lib.rs                       → 删除 skill 模块和 skill 类型的 pub use

保留:
  agent-base 不再有任何 skill 概念——回归纯工具注册 + 中间件 + 审批的运行时内核。
  `tool_enforcement.rs` 作为通用中间件实现在 agent-base 中保留。
```

---

## 9. 不做的事

- ❌ agent-works 不引入 ssh-simple 等任何领域依赖
- ❌ agent-works 不实现沙箱（Landlock/Seatbelt）
- ❌ agent-works 不定义任何业务类型
- ❌ agent-works 不改变 agent-base 的 trait 接口
- ✅ agent-works 保持与 agent-base 的 trait 兼容（所有 agent-base 的 trait 实现者不需要改动）

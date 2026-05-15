# Skills 功能设计

> agent-core 通用 Skill 系统 — 可组合的能力单元，按需加载

---

## 1. 概述

### 1.1 动机

agent-core 已有的能力抽象对比：

| 抽象 | 粒度 | 职责 |
|---|---|---|
| `Tool` | 单个函数调用 | LLM 自主决定调用时机，一次执行 |
| `SubAgentTool` | 完整 Agent 运行时 | 嵌套独立推理循环，重量级 |
| `Middleware` | 请求/响应钩子 | 无感知拦截，不提供工具 |

**Skill** 填补 Tool 和 SubAgent 之间的空白：
- **Tool 太细**：多个相关工具的组织、使用指南需要手动管理
- **SubAgent 太重**：独立推理循环 + 完整 Agent 上下文，开销大
- **System Prompt 膨胀**：把所有能力说明写死在 prompt 里不可维护

### 1.2 Skill 是什么

Skill 是一个**可复用的能力单元**，包含：

| 组成部分 | 说明 |
|---|---|
| **brief_description** | 一句话描述，常驻 system prompt（token 极省） |
| **detailed_description** | 完整操作手册，LLM 需要时通过工具按需获取 |
| **tools** | 该技能域的工具集合，自动注册到 ToolRegistry |

### 1.3 核心思路（方案 C：按需加载）

```
System Prompt（常驻，极少 token）：
  可用 Skills:
    - ssh-ops: SSH 远程运维，执行命令、管理文件、诊断服务器
    - db-diagnose: 数据库慢查询分析与索引优化建议
    - k8s-troubleshoot: Kubernetes 集群排障

  💡 需要某个 Skill 的详细操作指南时，调用 get_skill_detail 工具。
```

- **brief_description** 常驻，每个 skill 只占一行
- **detailed_description** 通过内置工具 `get_skill_detail(name)` 按需获取
- **tools** 自动注册，LLM 拿到详细手册后自然知道如何调用

### 1.4 token 对比

假设 10 个 skill，每个 detailed_description 约 500 token：

| 策略 | System Prompt token | 额外轮次 |
|---|---|---|
| 全量注入 | +5000 token / 每轮 | 无 |
| 按需加载 | +~50 token / 每轮 | 需要时 +1 轮 |

---

## 2. Skill trait 设计

### 2.1 定义

```rust
use std::sync::Arc;
use async_trait::async_trait;

use crate::tool::Tool;

/// Skill — 可复用的能力单元
///
/// 每个 Skill 声明：
/// - 简要描述（常驻 system prompt）
/// - 详细描述（按需加载）
/// - 工具集合（自动注册）
#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn brief_description(&self) -> String;
    fn detailed_description(&self) -> String;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}
```

### 2.2 为什么 tools 返回 Vec 而不是 &[Arc<dyn Tool>]

- 有的 Skill 的 tool 列表是动态构造的（比如 MCP Skill，需要连接后才能拿到工具列表）
- 返回 Vec 给了实现者最大的灵活性
- **约定**：`tools()` 只在 `build()` 阶段调用一次，实现者应保证幂等性

### 2.3 返回类型选择

`brief_description` 和 `detailed_description` 的返回类型定为 `String` 而非 `&'static str`：

- Skill 的描述可能需要运行时构造（从文件读取、根据配置拼接、模板化）
- 用 `&'static str` 限制太死，无法满足动态场景
- 如果实现者本身有 `&'static str`，转 `String` 开销极小

```rust
#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn brief_description(&self) -> String;
    fn detailed_description(&self) -> String;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}
```

---

## 3. 提示词注入策略

### 3.1 与 Skill trait 解耦

Skill trait 只声明"有什么"，提示词怎么拼交给独立的 `SkillPrompter`。

```rust
pub trait SkillPrompter: Send + Sync {
    /// 根据已注册的 skills，生成要注入 system prompt 的文本
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String;
}
```

### 3.2 内置实现

| 实现 | 行为 | 适用场景 |
|---|---|---|
| `LazySkillPrompter`（**默认**） | 只放 brief_description + 提示调用 get_skill_detail | 通用场景，skill 多时省 token |
| `FullDetailPrompter` | 把 brief + detailed 都塞进去 | 特殊场景，skill 少且需要零延迟 |

### 3.3 LazySkillPrompter 生成的 prompt 格式

```
## 可用 Skills

- **ssh-ops**: SSH 远程运维，执行命令、管理文件、诊断服务器
- **db-diagnose**: 数据库慢查询分析与索引优化建议
- **k8s-troubleshoot**: Kubernetes 集群排障

> 需要某个 Skill 的详细操作指南时，调用 get_skill_detail 工具获取。
```

### 3.4 提示文本可配置

将模板字符串作为 `LazySkillPrompter` 的字段，允许用户自定义：

```rust
impl LazySkillPrompter {
    pub fn new() -> Self { ... }
    pub fn title(mut self, title: impl Into<String>) -> Self { ... }
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self { ... }
    pub fn item_prefix(mut self, prefix: impl Into<String>) -> Self { ... }
}
```

### 3.5 注入位置语义

`SkillPrompter::build_prompt` 只负责生成文本，**不决定拼接位置**。注入位置由 AgentBuilder 控制：

- **默认**：system_prompt 尾部（Suffix），用 `---` 分隔
- **可选**：将来可以提供 `skill_prompt_position()` 方法，支持 Prefix 模式

### 3.6 SkillPrompter 接收 AgentConfig（预留）

当前 `build_prompt` 只接受 `skills` 列表。如果将来 prompter 需要根据 config 调整格式（比如 `enable_thought` 模式下用不同模板），可以将签名扩展为：

```rust
fn build_prompt(&self, skills: &[Arc<dyn Skill>], config: &AgentConfig) -> String;
```

以及提供一个默认实现（不接受 config），保持向后兼容。

---

## 4. 按需加载机制：get_skill_detail

### 4.1 工具定义

```rust
fn name() -> "get_skill_detail"
fn definition() -> {
    "function": {
        "name": "get_skill_detail",
        "description": "获取指定 Skill 的详细操作指南。当你需要了解某个 Skill 的完整使用方式时调用。",
        "parameters": {
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill 名称"
                }
            },
            "required": ["name"]
        }
    }
}
```

### 4.2 行为

- 调用 `get_skill_detail("ssh-ops")` → 返回该 skill 的 `detailed_description()`
- 调用 `get_skill_detail("unknown")` → 返回可用 skill 列表作为提示
- `control_flow = Break`，LLM 拿到手册后下一轮继续

### 4.3 实现方式

`get_skill_detail` 是一个由 AgentRuntime 在 `build()` 时自动生成的内部工具，对调用方完全透明。

### 4.4 工具名冲突保护

`get_skill_detail` 是内置工具，其名称需要保留：

- 如果用户已注册同名工具，`build()` 阶段返回错误（推荐）或自动跳过
- 也可以在 AgentBuilder 上提供 `skill_detail_tool_name()` 方法，允许自定义名称

---

## 5. AgentBuilder 变更

### 5.1 新增方法

```rust
impl AgentBuilder {
    /// 注册一个 Skill
    ///
    /// 会自动：
    /// 1. 将 skill 的所有 tools 注册到 ToolRegistry
    /// 2. build() 时通过 SkillPrompter 注入 system prompt
    pub fn register_skill(mut self, skill: impl Skill + 'static) -> Self { ... }

    /// 自定义提示词注入策略（可选）
    ///
    /// 默认使用 LazySkillPrompter
    pub fn skill_prompter(mut self, prompter: Arc<dyn SkillPrompter>) -> Self { ... }

    /// 禁用 skill 自动拼接到 system_prompt
    ///
    /// 如果你希望通过 Middleware 或其他方式手动注入 skill 提示词
    pub fn disable_skill_prompt_injection(mut self) -> Self { ... }

    /// 自定义 get_skill_detail 工具名称（可选）
    ///
    /// 当用户已注册同名工具时使用，默认 "get_skill_detail"
    pub fn skill_detail_tool_name(mut self, name: impl Into<String>) -> Self { ... }
}
```

### 5.2 build() 时发生的事情

```
AgentBuilder::build()

1. 遍历所有已注册 skill
   ├─ 注册 tools：skills[*].tools() → ToolRegistry
   │   └─ 若 tool 名冲突（同名出现在两个 Skill 中，或与 register_tool 注册的工具重名）：
   │      → build() 阶段 panic / 返回错误（推荐）
   │      → 未来可考虑自动加前缀 "skill_name::tool_name"
   └─ 收集 skill 列表到 AgentRuntime.skills

2. 用 SkillPrompter::build_prompt(skills) 生成 skill_prompt

3. skill_prompt 拼接到 config.system_prompt 尾部，分隔符使用 "\n\n---\n\n"
   - 如果 config.system_prompt 为 None，则 skill_prompt 单独作为 system_prompt
   - 提供 disable_skill_prompt_injection() 方法，允许用户手动控制（自己用 Middleware 注入）

4. 自动创建 get_skill_detail 工具并注册到 ToolRegistry（检测同名冲突）

5. 返回 AgentRuntime
```

### 5.3 Tool name 冲突策略

| 场景 | 行为 |
|---|---|
| Skill A 注册了 `exec`，Skill B 也注册了 `exec` | build() panic，要求改名 |
| 用户先 `register_tool(x)`，再 `register_skill(y)`，存在同名 | build() panic |
| 未来扩展方向 | 自动加前缀 `skill_name__tool_name`（可配置） |

---

## 6. AgentRuntime 变更

### 6.1 新增字段

```rust
pub struct AgentRuntime {
    // ... 现有字段不变 ...

    pub(crate) skills: Vec<Arc<dyn Skill>>,       // 已注册的 skill 列表
    pub(crate) skill_prompter: Arc<dyn SkillPrompter>, // 提示词策略
}
```

### 6.2 新增方法

```rust
impl AgentRuntime {
    pub fn skills(&self) -> &[Arc<dyn Skill>] { ... }
    pub fn get_skill_detail(&self, name: &str) -> Option<&'static str> { ... }
}
```

### 6.3 get_skill_detail 工具处理

在 `ToolRegistry` 中注册一个内部工具 `SkillDetailTool`，其 `call()` 实现：

```rust
async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
    let name = args["name"].as_str().unwrap_or("");
    let detail = self
        .skills
        .iter()
        .find(|s| s.name() == name)
        .map(|s| s.detailed_description());

    match detail {
        Some(desc) => ToolOutput {
            summary: desc.to_string(),
            raw: None,
            control_flow: ToolControlFlow::Break,
            truncated: false,
        },
        None => ToolOutput {
            summary: format!(
                "未找到 Skill '{}'。可用 Skills: {}",
                name,
                self.skills.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
            ),
            raw: None,
            control_flow: ToolControlFlow::Break,
            truncated: false,
        },
    }
}
```

---

## 7. Skill 依赖注入

### 7.1 设计原则

Skill 自身的依赖（如 SSH 连接、DB 连接池）通过**构造函数传入**，而非通过 `ToolContext`：

```rust
struct SshOpsSkill {
    ssh_config: SshRuntimeConfig,
}

impl SshOpsSkill {
    fn new(ssh_config: SshRuntimeConfig) -> Self { ... }
}
```

- `ToolContext` 负责传递 Agent 运行时上下文（session_id、event_bus、llm_client）
- Skill 的领域依赖（SSH/DB 等）由 Skill 的构造函数接管，保持类型安全
- `register_skill(SshOpsSkill::new(ssh_config))` 即完成注入

### 7.2 与 Middleware 的协作

skill_prompt 注入发生在 **build 阶段**（静态），而非每轮运行时（动态）：

- `build()` 阶段：收集 skills → SkillPrompter 生成文本 → 拼接到 `config.system_prompt`
- 运行时：Middleware 不再管理 skill prompt，减少每轮开销
- 如果用户用了 `disable_skill_prompt_injection()`，可以通过 Middleware::on_pre_llm 手动注入

---

## 8. AgentEvent 扩展

### 8.1 SkillDetailLoaded 事件

当 LLM 调用 `get_skill_detail` 时，发送事件通知上层：

```rust
// AgentEvent 新增变体（或用 Custom 实现）
AgentEvent::Custom {
    session_id: ...,
    payload: json!({
        "type": "skill_detail_loaded",
        "skill": "ssh-ops"
    }),
}
```

用途：前端提示"Agent 正在加载 ssh-ops 的详细操作手册"。

### 8.2 Skill 元数据（可选）

Skill trait 可增加默认实现的方法，不强制每个 Skill 都填：

```rust
pub trait Skill: Send + Sync {
    // ... 核心方法 ...

    fn version(&self) -> &'static str { "0.1.0" }
    fn tags(&self) -> &[&'static str] { &[] }
    fn author(&self) -> &'static str { "" }
}
```

用途：调试日志、未来 skill marketplace、前端展示。

---

## 9. 目录结构

```
src/
├── skill/
│   ├── mod.rs           # Skill trait + SkillPrompter trait
│   ├── registry.rs      # SkillRegistry
│   ├── prompter.rs      # LazySkillPrompter / FullDetailPrompter
│   └── detail_tool.rs   # get_skill_detail 内部工具
```

---

## 10. 使用示例

### 10.1 定义 Skill

```rust
use agent_core::skill::Skill;
use std::sync::Arc;

struct SshOpsSkill {
    ssh_config: SshRuntimeConfig,
}

impl SshOpsSkill {
    fn new(ssh_config: SshRuntimeConfig) -> Self {
        Self { ssh_config }
    }
}

impl Skill for SshOpsSkill {
    fn name(&self) -> &'static str {
        "ssh-ops"
    }

    fn brief_description(&self) -> String {
        "SSH 远程运维：执行命令、管理文件、诊断服务器状态".to_string()
    }

    fn detailed_description(&self) -> String {
        r##"
## SSH 远程运维 Skill

### 适用场景
- 需要在远程 Linux 服务器上执行命令
- 排查服务器性能问题（CPU、内存、磁盘）
- 管理服务（启动、停止、重启）
- 查看日志和配置文件

### 可用工具
- execute_ssh_command：执行短命令（ls、cat、df、systemctl status 等）
- create_and_execute_plan：多步计划执行，适合环境探测
- start_interactive_task：为长时间命令（apt install、docker pull）启动后台任务
- manage_interactive_task：管理后台任务（读取输出、发送输入、停止）

### 使用流程
1. 先通过 execute_ssh_command 快速获取服务器状态（df -h、free -m、top -bn1）
2. 发现问题后用 create_and_execute_plan 执行多步诊断
3. 需要安装软件或执行长时间操作时，用 start_interactive_task

### 注意事项
- 谨慎使用 sudo，会触发审批
- rm/reboot 等破坏性命令需要用户明确确认
        "##.trim().to_string()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(SshCommandTool::new(self.ssh_config.clone())),
            Arc::new(PlanTool::new(self.ssh_config.clone())),
            Arc::new(InteractiveTaskStartTool::new(terminal_manager.clone())),
            Arc::new(InteractiveTaskControlTool::new(terminal_manager.clone())),
        ]
    }
}
```

### 10.2 使用 Skill

```rust
use agent_core::{AgentBuilder, skill::LazySkillPrompter};

let runtime = AgentBuilder::new(llm_client)
    .system_prompt("你是运维助手，负责诊断和修复服务器问题。")
    .register_skill(SshOpsSkill::new(ssh_config))
    .register_skill(DbDiagnoseSkill::new(db_config))
    // 也可以切换注入策略
    // .skill_prompter(Arc::new(FullDetailPrompter))
    .build();
```

---

## 11. 扩展性设计

### 11.1 当前不做的

| 功能 | 原因 |
|---|---|
| Skill 动态激活/停用 | 当前场景固定 skill set，不需要运行时切换 |
| Skill 优先级/排序 | 过度设计，将来有需求再加 |
| Skill 之间的依赖声明 | 当前 skill 之间独立 |

### 11.2 预留的扩展空间

| 扩展点 | 方式 |
|---|---|
| 自定义 SkillPrompter | 实现 `SkillPrompter` trait，替换默认策略 |
| 动态 tool 列表 | Skill::tools() 返回 Vec，可以运行时决定 |
| Skill 内嵌 Middleware | 如果将来有需求，可以给 Skill trait 加 `middlewares()` 方法 |
| Skill 元数据 | `name()` / `brief_description()` 本身就是元数据 |

---

## 12. 实现计划

| 步骤 | 文件 | 内容 |
|---|---|---|
| 1 | `src/skill/mod.rs` | 新建 skill 模块，定义 `Skill` trait + `SkillPrompter` trait |
| 2 | `src/skill/prompter.rs` | 实现 `LazySkillPrompter`（默认）和 `FullDetailPrompter` |
| 3 | `src/skill/detail_tool.rs` | 实现 `get_skill_detail` 内部工具 |
| 4 | `src/engine/builder.rs` | `AgentBuilder` 新增 `register_skill()` / `skill_prompter()` / `skills` 字段 |
| 5 | `src/engine/runtime/mod.rs` | `AgentRuntime` 新增 `skills` / `skill_prompter` 字段，`build()` 时自动注入 |
| 6 | `src/lib.rs` | 导出 `skill` 模块公开类型 |
| 7 | `examples/skill_demo.rs` | 新增演示示例 |

---

## 13. 测试要点

- [ ] 单个 Skill 注册后，tools 正确进入 ToolRegistry
- [ ] 多个 Skill 注册后，tools 不冲突
- [ ] **两个 Skill 提供同名 tool** → build() panic
- [ ] 用户 `register_tool(x)` 后 Skill 中也有同名 tool → build() panic
- [ ] **空 skills 集合**：没有任何 skill 注册 → 行为退化到与现状一致
- [ ] `LazySkillPrompter` 输出格式正确，只含 brief_description
- [ ] `LazySkillPrompter` 模板文本可自定义
- [ ] `FullDetailPrompter` 输出包含 brief + detailed
- [ ] `get_skill_detail` 工具返回正确的 detailed_description
- [ ] `get_skill_detail` 对未知 skill 返回提示 + 可用 skill 列表
- [ ] `get_skill_detail` **参数缺失/类型错误**的容错
- [ ] System prompt 中包含 skill_prompt 且不破坏原有 prompt
- [ ] System prompt **为 None** 时，skill_prompt 单独作为 system_prompt
- [ ] `disable_skill_prompt_injection()` 解除自动拼接
- [ ] `ControlFlow::Break` 正确触发下一轮推理
- [ ] `get_skill_detail` 调用后发送 `skill_detail_loaded` 事件
- [ ] **Skill 内的 tools 与 Skill 外部 register_tool 的工具同时存在** → 正常共存

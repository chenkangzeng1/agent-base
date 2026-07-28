# phi-agent 品牌建设计划

## 目标

将 phi-agent 从一个个人项目打造成一个有辨识度、值得信赖的开源 AI Agent 框架品牌。

覆盖两个维度：
- **开发者体验 (DevEx)**：文档、教程、示例，让开发者快速上手
- **产品质量 (Quality)**：CI/CD、测试、规范，建立技术信任

---

## Phase 1: 质量基础设施 ✅ 已完成

> 目标：让每次提交都经过自动化检查，代码风格统一，测试可运行

### 1.1 CI/CD (GitHub Actions)

- [x] 创建 `.github/workflows/ci.yml` — fmt / clippy / build / test / doc，stable + beta
- [ ] 创建 `.github/workflows/publish.yml`（后续启用）

### 1.2 代码规范

- [x] 创建 `rustfmt.toml` — 统一代码风格（stable channel）
- [x] `cargo fmt` + `cargo clippy` phi-agent 零警告

### 1.3 测试框架

- [x] 创建 `tests/integration_test.rs` — 7 个集成测试（mock LLM client）
- [x] `base_agent_builder()` 和 `PhiAgent::build()` smoke test
- [ ] code coverage（tarpaulin + codecov.io，后续）

### 1.4 项目规范文件

- [x] `CHANGELOG.md`
- [x] `SECURITY.md` — 联系邮箱 phiagent@hibuka.com
- [x] `CONTRIBUTING.md`
- [x] `CODE_OF_CONDUCT.md`

---

## Phase 2: 开发者体验 ✅ 已完成

> 目标：新开发者 5 分钟内能跑起来，30 分钟内能写第一个自定义工具

### 2.1 API 文档 (doc comments)

- [x] 全部公开类型/函数添加 `///` 文档注释
- [x] `cargo doc --no-deps` 零警告
- [x] 每个模块 `//!` 描述职责

### 2.2 README 增强

- [x] Badges：CI / crates.io / docs.rs / license
- [x] 架构图（ASCII art）
- [x] "Why phi-agent" 卖点段落（简单 / Rust / 纯粹 / 你的规则）
- [x] 自定义工具完整示例
- [x] FAQ（4 个问题）
- [x] 中英文 README 同步

### 2.3 示例项目

- [x] `examples/hello-agent.rs` — 最简 agent 启动
- [x] `examples/custom-tool.rs` — 自定义 Calculator 工具
- [x] `examples/multi-tool.rs` — 注册多个工具
- [x] 所有示例通过编译
- [ ] ~~browser-agent.rs~~ — 浏览器工具暂不开源

### 2.4 教程文档

- [x] `guide/getting-started.md` + `_CN.md` — 5 分钟快速开始
- [x] `guide/custom-tool.md` + `_CN.md` — 如何写 Tool
- [x] `guide/configuration.md` + `_CN.md` — 配置详解
- [x] `guide/advanced.md` + `_CN.md` — 高级用法
- [ ] ~~browser-tools.md~~ — 浏览器工具暂不开源

### 2.5 额外完成

- [x] `.env.example` 增强 — 含 OpenAI / DeepSeek / Groq / Ollama 配置示例
- [x] `.gitignore` 添加 `CLAUDE.md` 忽略规则
- [x] 三个兄弟仓库（agent-base / agent-works / phi-tools）统一 repository URL → hibuka-labs

---

## Phase 3: 品牌形象（后续）

> 目标：有视觉辨识度，有社区入口

### 3.1 Logo & 视觉

- [ ] 设计 phi-agent Logo（符号 + 文字）
- [ ] 确定品牌色
- [ ] 统一 README / crates.io / 文档站 的视觉

### 3.2 文档站

- [ ] 使用 mdBook 搭建文档站
- [ ] 部署到 GitHub Pages 或自定义域名
- [ ] 包含：教程、API 参考、示例、设计文档

### 3.3 社区入口

- [ ] GitHub Discussions 开启
- [ ] 在 crates.io 的 README 中引导到文档站
- [ ] （可选）Discord / 飞书群

---

## 仓库地址

| 仓库 | 地址 |
|------|------|
| phi-agent | https://github.com/hibuka-labs/phi-agent |
| agent-base | https://github.com/hibuka-labs/agent-base |
| agent-works | https://github.com/hibuka-labs/agent-works |
| phi-tools | https://github.com/hibuka-labs/phi-tools |

三个 remote：github / gitee / origin (buka)

## 关键决策

- **浏览器工具暂不开源** — 保留在 `browser-tools` 分支
- **CLI 使用 OpenAiClient** — 仅支持 OpenAI 兼容 API；Anthropic 需要代码改动
- **不内置记忆存储** — 作为卖点强调：纯粹、可预测、数据可控
- **教程中英双语** — guide/ 下 EN + _CN 双版本

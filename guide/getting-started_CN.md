# 快速开始

5 分钟跑起你的第一个 phi-agent。

## 前置条件

- [Rust](https://rustup.rs)（stable，edition 2024）
- 一个 LLM API Key（兼容 OpenAI 接口）

## 1. 安装 phi-agent

```bash
cargo install phi-agent
```

## 2. 创建项目

```bash
phi init my-agent
cd my-agent
```

## 3. 配置 API Key

```bash
cp .env.example .env
# 编辑 .env，填入真实 API Key
```

`.env.example` 包含 OpenAI、Anthropic、DeepSeek 等常见提供商的配置示例。详见 [配置详解](configuration.md)。

## 4. 运行

`phi init` 已经生成了可用的 `src/main.rs`，直接运行：

```bash
cargo run
```

## 下一步

- [自定义工具](custom-tool.md) — 为 Agent 添加你自己的工具
- [Focus 专注判断](focus.md) — 结构化单任务 LLM 调用
- [配置详解](configuration.md) — 了解所有配置选项
- [高级用法](advanced.md) — 中间件、会话、事件日志

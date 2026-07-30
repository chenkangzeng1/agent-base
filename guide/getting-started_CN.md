# 快速开始

5 分钟跑起你的第一个 phi-agent。

## 前置条件

- [Rust](https://rustup.rs)（stable，edition 2024）
- 一个 LLM API Key（兼容 OpenAI 接口）

## 安装

```bash
cargo install phi-agent
```

## 方式一：CLI（推荐）

直接运行，进入交互对话：

```bash
phi
```

```
phi> 现在几点了？
🔧 get_time
2025-07-30 19:30:00

phi> /exit
```

内置 shell 工具、观测指标、思考模式。适合大多数用户。

## 方式二：代码集成

创建一个项目，编写你自己的 Agent 和工具：

```bash
phi init my-agent
cd my-agent
```

配置 API Key 后运行：

```bash
cp .env.example .env
# 编辑 .env，填入真实 API Key
cargo run
```

`phi init` 生成了一个 REPL + `ClockTool` 示例。打开 `src/main.rs`，照着 `ClockTool` 写你自己的工具，注册到 Agent 就行。

更多工具示例见 [自定义工具](custom-tool.md)。

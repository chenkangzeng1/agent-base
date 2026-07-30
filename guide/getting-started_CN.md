# 快速开始

5 分钟跑起你的第一个 phi-agent。

## 前置条件

- [Rust](https://rustup.rs)（stable，edition 2024）
- 一个 LLM API Key（兼容 OpenAI 接口）

## 安装

```bash
cargo install phi-agent
```

## 方式一：一键生成（推荐）

用 `phi init` 生成完整项目，包含一个示例工具和 REPL：

```bash
phi init my-agent
cd my-agent
cp .env.example .env   # 编辑 .env 填入 API Key
cargo run
```

```
phi> 现在几点了？
🔧 get_time
当前时间：2025-07-30 19:30:00

phi> /exit
```

打开 `src/main.rs`，你会看到 `ClockTool` 的完整代码。照着它写你自己的工具，注册到 Agent 就行。

详见 [自定义工具](custom-tool.md)。

## 方式二：库集成

把 phi-agent 作为库加入已有项目：

```bash
cargo add phi-agent
cargo add tokio --features full
cargo add anyhow
cargo add dotenvy
cargo add async-trait
cargo add serde_json
cargo add chrono
```

然后复制 `ClockTool` 示例代码到你的 `main.rs` 即可。
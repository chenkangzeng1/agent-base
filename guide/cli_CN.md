# CLI 使用

`phi` 二进制支持 REPL（交互式）和单次执行两种模式。基于 `OpenAiClient`，兼容任何 OpenAI 格式的 API。

## 快速开始

```bash
# 编译
cargo build --release

# 单次执行
cargo run -- "2+2 等于多少？"

# REPL 交互模式
cargo run
```

## 配置

创建 `.env` 文件或设置环境变量：

```bash
LLM_API_KEY=sk-your-key-here
LLM_MODEL=gpt-4o
# LLM_BASE_URL 默认为 https://api.openai.com/v1
```

详细配置见 [配置详解](configuration.md)。

## 单次执行

```bash
phi "解释一下这个代码库"
phi "列出所有 Rust 文件" --model opus
phi "查看架构" --json | jq '.type'
phi "静默运行" --quiet
```

## REPL 模式

```bash
phi
# 输入问题，回车发送
# Ctrl+C 取消当前轮次
# Ctrl+D 退出
```

## CLI 参数

| 参数 | 说明 |
|------|------|
| `QUERY` | 位置参数 — 单次执行的问题（不传则进入 REPL） |
| `-m, --model` | 覆盖模型名称 |
| `--base-url` | 覆盖 API 基础 URL |
| `-s, --session` | 会话 ID（恢复之前的会话） |
| `--no-thinking` | 禁用扩展思考 |
| `--json` | JSON 流输出（方便管道） |
| `--quiet` | 无输出 |

## 会话持久化

会话自动保存到 `~/.phi-agent/sessions/<id>/`。使用 `--session` 恢复：

```bash
phi --session 20250728_a1b2c3d4
```

超过 7 天不活跃的会话会自动清理。

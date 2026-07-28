# 自动文章发布方案

## 背景

目前有两个独立工具：

- **article-auto-publish**：Python CLI，通过 Playwright 扫码登录 + 各平台 API 发布文章。已支持 CSDN、掘金、知乎、SegmentFault、开源中国。但对没有 API 或有强反爬的平台（如博客园），无法自动化。
- **phi-agent**：Rust AI Agent 框架，可接入 LLM，配合 browser 工具组（导航、点击、输入、截图等 20 个工具），让 AI 自主操作浏览器。

## 目标

将两者结合，实现**全平台自动发布**：

- 有 API 的平台（CSDN、掘金）→ 走 API，快且稳
- 没有 API 的平台（博客园等）→ 走 Agent + 浏览器，LLM 分析 DOM 自主操作

## 整体架构

```
┌─────────────────────────────────────────┐
│         内容生产层 (phi-agent)              │
│  选题 → 调研 → 写作 → 润色 → Review        │
│  LLM 驱动的创作流程                        │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│        分发发布层 (article-auto-publish)    │
│  格式化 → 登录态 → 图片上传 → 发布到各平台   │
│                                           │
│  ├── API 适配器 (CSDN/掘金) — 直接 HTTP    │
│  └── Agent 引擎 (博客园/其他) — 浏览器操作   │
└─────────────────────────────────────────┘
```

## 集成方式

两个工具**都是 CLI**，通过子进程 + 文件系统互相调用，不需要任何 IPC 框架。

### 方向 1：phi-agent 调 article-auto-publish（写文章场景）

phi-agent 内置 `LocalShellTool`，Agent 在写文章过程中直接执行 shell 命令：

```
用户（在 phi REPL 里）：
  "帮我写一篇 Rust async 的文章，然后发到掘金"

Agent 流程：
  1. 搜索资料 → 写文章 → 保存到 articles/rust-async.md
  2. 执行: aap publish articles/rust-async.md -p juejin
  3. 汇报: "✅ 已发布到掘金: https://..."
```

零胶水代码，`LocalShellTool` 天然支持调用 Python 脚本。

### 方向 2：article-auto-publish 调 phi-agent（批量发布场景）

article-auto-publish 对无 API 的平台，启动 phi-agent 子进程操作浏览器：

```python
# article-auto-publish 内部
result = subprocess.run([
    "phi",
    "--connect-ws", ws_url,    # 连接到共享浏览器的 CDP
    "--query", agent_prompt,   # 发布指令
    "--auto-approve",          # 不需要人工确认
    "--format", "json",        # JSON 输出方便解析
    "--no-thinking",           # 不显示思考过程
    "--no-color",
    "--max-tool-calls", "60",
], capture_output=True, text=True, timeout=300)
```

## Cookie 共享方案

这是最关键的技术问题：登录态怎么在 article-auto-publish 和 phi-agent 之间共享。

### 方案：共享浏览器（CDP 连接）

```
article-auto-publish                    phi-agent
      │                                     │
      ├─ Playwright 启动 Chromium           │
      │  (--remote-debugging-port=9222)     │
      │                                     │
      ├─ 加载 Cookie 到浏览器               │
      │  context.add_cookies(cookies)       │
      │                                     │
      ├─ 获取 WS URL ──────────────────────▶│
      │  http://localhost:9222/json/version  │ --connect-ws ws://...
      │                                     │
      │                               Agent 在同一浏览器操作
      │                               天然带登录态，无需管 Cookie
      │                                     │
      │◀─────── Agent 发布完成 ──────────────│
      │                                     │
      ├─ 关闭浏览器                          │
```

优点：
- Cookie 完全不需要在进程间传递
- httpOnly、Secure 等所有类型 Cookie 都能正常工作
- phi-tools 已有 `BrowserToolset::connect()` 支持

## article-auto-publish 改动

### 1. 平台注册表增加 mode 字段

```python
PLATFORMS = {
    "csdn": {
        ...,
        "mode": "api",      # 有完整 API，直接发
    },
    "juejin": {
        ...,
        "mode": "api",
    },
    "zhihu": {
        ...,
        "mode": "api",      # 已有 API + Playwright 混合
    },
    "segmentfault": {
        ...,
        "mode": "agent",    # 发布需要浏览器操作
    },
    "cnblogs": {
        ...,
        "mode": "agent",    # 没有 API，全靠浏览器
    },
}
```

### 2. 新增文件

| 文件 | 用途 |
|------|------|
| `uploader/agent_publisher.py` | Agent 发布引擎：构造 prompt → 调 phi → 解析结果 |
| `utils/agent_browser.py` | 共享浏览器管理：启动带 CDP 的 Chromium、加载 Cookie、获取 WS URL |

### 3. publish 命令改造

根据平台 `mode` 分流：

```python
if platform_info["mode"] == "api":
    result = await platform_info["publish"](title, content, tags, cookie_file)
elif platform_info["mode"] == "agent":
    result = await publish_via_agent(title, content, tags, platform_info, cookie_file)
```

## Agent Prompt 设计

给 phi-agent 的指令需要结构化，确保 LLM 能可靠执行：

```
任务：在 {平台名} 上发布一篇文章。

文章信息：
- 标题：{title}
- 内容（Markdown）：
{content}
- 标签：{tags}

执行步骤：
1. 导航到编辑器页面：{editor_url}
2. 用 browser_snapshot 获取页面结构
3. 找到标题输入框，填入标题
4. 找到内容编辑器，填入文章内容
   — 如果是 Textarea，用 browser_input
   — 如果是富文本编辑器（contenteditable / CodeMirror），用 browser_evaluate 设置内容
5. 如果有标签/分类输入框，填入标签
6. 点击发布/提交按钮
7. 等待页面跳转或出现成功提示
8. 返回发布结果

注意事项：
- 每完成一个主要步骤，用 browser_screenshot 截图留证
- 如果出现弹窗，先关掉再继续
- 如果发布按钮是灰色的，检查还有哪些必填项没填
- 发布完成后，用以下 JSON 格式汇报结果：
  {"success": true/false, "article_url": "...", "message": "..."}
```

## 结果解析

phi-agent 的 JSON 输出格式（`--format json`）：

- 每个事件一行 JSON，最后一个是 `turn_finished` 事件
- `turn_finished.assistant_text` 包含 LLM 的最终回复
- 从回复中提取 JSON 结果块，解析出 `success` / `article_url` / `message`

容错策略：
- Agent 超时（5 分钟）→ 返回失败
- 找不到 JSON 结果 → 以 `assistant_text` 全文作为 message，success 看关键词判断
- Cookie 过期 → 提示用户重新登录

## 分步实施计划

### Phase 1：验证可行性（当前）

- [ ] 用 phi REPL 手动测试：Agent 能否在知乎编辑器里完成发布操作
- [ ] 确认 browser 工具组工作正常
- [ ] 观察 LLM 对页面结构的理解能力

### Phase 2：实现集成

- [ ] 新增 `utils/agent_browser.py` — 共享浏览器（CDP）管理
- [ ] 新增 `uploader/agent_publisher.py` — Agent 发布引擎
- [ ] 改造 `aap_cli.py` — PLATFORMS 增加 mode 字段，publish 按 mode 分发
- [ ] 博客园登录（`utils/browser.py` 新增 `login_cnblogs`）
- [ ] 端到端验证

### Phase 3：扩展覆盖

- [ ] SegmentFault / OSChina 的 browser publish 改为 Agent 模式（删掉 680 行硬编码 Playwright 脚本）
- [ ] 新增更多平台：51CTO、慕课网、简书、腾讯云社区
- [ ] 持续优化 prompt，提高成功率

## phi-agent 侧

**不需要任何代码改动**。现有的 browser 工具组 + CLI `--connect-ws` + `--auto-approve` 已经完全够用。

后续可优化的点（非必需）：
- 增加 `--response-format` CLI 参数，支持 JSON Schema 约束 LLM 输出
- 增加一个 `browser_set_cookies` 工具，方便从 JSON 文件加载 Cookie

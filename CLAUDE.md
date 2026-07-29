# CLAUDE.md

## Project: phi-agent

A general-purpose AI Agent framework in Rust, built on `agent-base` and `agent-works`.

### Architecture Principle
**phi-agent itself does NOT bundle any tools.** It provides infrastructure only (builder factory, renderers, config, session management). Tools are implemented in `phi-tools` and injected by consumers (CLI, web, etc.).

### Current Branch
`master` — the public open-source branch. **Does NOT include browser automation tools.**

Browser tools (21 tools via CDP) live on the `browser-tools` branch and in phi-tools' `browser-tools` branch. Not yet open-sourced.

### Dependency Chain
```
agent-base (runtime kernel + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← framework, no tools
    ↑
forge (bin) ← CLI, registers tools here
```

### Key Crates (separate repos, sibling dirs)
- `agent-base` — AgentRuntime, Tool trait, RuntimeEvent, LLM clients (OpenAI + Anthropic)
- `agent-works` — MCP, Skills, built-in file tools, Focus
- `phi-tools` — Tool implementations (LocalShellTool only on master)
- `log-core` — Structured file logging
- `phi-agent` — This crate

All repos have 3 remotes: `github`, `gitee`, `origin`.

### Project Structure
```
phi-agent/
├── .github/workflows/ci.yml  # CI: fmt + clippy + build + test + doc
├── examples/                 # Runnable examples
│   ├── hello-agent.rs
│   ├── custom-tool.rs
│   ├── focus-demo.rs
│   └── multi-tool.rs
├── guide/                    # User tutorials (EN + CN)
│   ├── getting-started.md / _CN.md
│   ├── custom-tool.md / _CN.md
│   ├── focus.md / _CN.md
│   ├── configuration.md / _CN.md
│   └── advanced.md / _CN.md
├── book/                     # mdBook documentation site
│   ├── book.toml
│   └── src/
├── assets/                   # Brand assets (logo, etc.)
│   └── logo.svg
├── tests/
│   └── integration_test.rs   # 7 tests with mock LLM client
├── docs/                     # Design notes (git-ignored, local only)
│   ├── design.md
│   ├── forge-tools-design.md
│   ├── auto-publish-design.md
│   └── brand-building-plan.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md               # Contact: phiagent@hibuka.com
├── CODE_OF_CONDUCT.md
├── rustfmt.toml
├── README.md / README_CN.md
└── src/
    ├── lib.rs                # Public API re-exports
    ├── agent/
    │   ├── builder.rs        # base_agent_builder() factory
    │   └── factory.rs        # PhiAgent struct + PhiAgentConfig
    ├── render/               # EventRenderer trait + Terminal/JsonStream/Null
    ├── cli/                  # AutoApprovalHandler (Auto/DenyAll)
    ├── config/               # LLM config resolution (CLI > env > .env > default)
    ├── prompt.rs             # build_system_prompt() + build_system_prompt_cn()
    ├── event_log.rs          # Turn event → JSONL persistence
    ├── session.rs            # Session ID, directory, file locking, cleanup
    └── bin/forge/            # CLI binary (phi): REPL + one-shot
```

### Brand Building Progress
- ✅ Phase 1 — Quality: CI/CD, rustfmt, CHANGELOG, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, tests
- ✅ Phase 2 — DevEx: API doc comments, enhanced README (badges, architecture, FAQ, Why section), 3 examples, 4 tutorials (EN+CN), .env.example
- 🔄 Phase 3 — Branding: Logo ✅, mdBook doc site ✅, Issue templates ✅, community (GitHub Discussions pending)

### Conventions
- Rust edition 2024
- Async runtime: tokio (full features)
- Error handling: anyhow + agent_base::AgentResult
- CLI: clap derive mode, uses OpenAiClient (OpenAI-compatible APIs only)
- Session data: ~/.phi-agent/sessions/<id>/
- Contact email: phiagent@hibuka.com

### Key Design Decisions
- **No built-in tools** — framework knows nothing about specific tools
- **No built-in memory** — no vector DB, no hidden state. Predictable and debuggable.
- **OpenAI-compatible CLI** — CLI uses OpenAiClient; Anthropic requires AnthropicClient (code change)
- **China-first prompt** — build_system_prompt_cn() for GFW-aware environments
- **Browser tools separated** — kept on browser-tools branch, not on master

### Pre-Push Checklist

CI runs `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` across **all** dependency repos.
**Run this locally before every push:**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

If `cargo fmt` reports diffs, fix them: `cargo fmt` (no `--check`).  
If clippy warns, fix the warnings — CI treats all warnings as errors.  
Note: due to path dependencies, this also checks `agent-base`, `agent-works`, `phi-tools`.  
**If you modified any dependency repo, push it BEFORE pushing phi-agent** — CI clones them fresh from `hibuka-labs` on each run.

### Documentation Deployment

```
docs.phi-agent.dev (域名: phiagent.dev, 注册商: 22net)
  ├── 境内 → 阿里云 OSS 香港 (phiagent-docs.oss-cn-hongkong.aliyuncs.com)
  └── 境外 → GitHub Pages (hibuka-labs.github.io)
```

- **Building**: `mkdocs build` (MkDocs Material + i18n plugin) → `site/`
- **Deploy**: `.github/workflows/deploy-docs.yml` — builds then syncs to OSS (oss2 SDK) + GitHub Pages
- **Manual trigger**: Actions → Deploy Docs → Run workflow (needed after first-time setup or when docs files aren't changed)
- **Auto trigger**: Push to master with changes in `docs/**`, `mkdocs.yml`, `requirements.txt`, or `deploy-docs.yml`
- **OSS Bucket**: `phiagent-docs` (香港), static website hosting enabled, default page `index.html`
- **Secrets** (GitHub → Settings → Secrets): `OSS_ACCESS_KEY_ID`, `OSS_ACCESS_KEY_SECRET`, `OSS_ENDPOINT`, `OSS_BUCKET`

### Gotchas

- **log-core** uses `main` as default branch (not `master`) on GitHub — push to `main` not `master`
- **Phi-tools** has a separate `browser-tools` branch not yet on master
- CI clones all 5 repos as siblings with `--depth 1`, so pushed code must be on the default branch

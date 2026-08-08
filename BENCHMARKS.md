# Benchmark Baseline (Pre-0.9.0)

Date: 2026-08-08 | Machine: Mac (Apple Silicon) | Rust: stable

## agent-base (ToolRegistry)

| Benchmark | Time | Notes |
|-----------|------|-------|
| registry/register_1 | 40 ns | Single tool registration with Arc boxing |
| registry/metadatas_50 | 3.1 µs | Metadata list generation from 50 tools |
| registry/remove | 52 ns | Single tool removal |

## agent-works (AgentBuilder)

| Benchmark | Time | Notes |
|-----------|------|-------|
| agent_works/build_empty | ~6 µs | Baseline builder → AgentRuntime |
| agent_works/build_with_prompt | ~7 µs | With 50-line system prompt |
| agent_works/build_10_tools | ~6.3 µs | Builder with 10 registered tools |
| agent_works/build_50_tools | 7.5 µs | Builder with 50 registered tools |
| agent_works/build_100_tools | 11.4 µs | Builder with 100 registered tools |

## phi-kernel-tools (File Tools)

| Benchmark | Time | Notes |
|-----------|------|-------|
| file/read_1000_lines | ~5 µs | Read 1000-line file via ReadFileTool |
| file/write_100_lines | 15.1 µs | Write 100-line file via WriteFileTool |
| file/list_100_flat | 233 µs | List directory with 120 files (100 flat + 20 in sub) |

## phi-agent (Full Stack)

| Benchmark | Time | Notes |
|-----------|------|-------|
| agent/build_empty | 12.2 µs | PhiAgent::build (no tools) |
| agent/build_10_tools | 13.7 µs | PhiAgent::build with 10 tools |
| agent/build_50_tools | 19.2 µs | PhiAgent::build with 50 tools |
| agent/build_100_tools | 26.3 µs | PhiAgent::build with 100 tools |
| bridge/build_from_builder | 12.2 µs | ProtocolServer::from_builder |
| bridge/get_or_create_session | 3.1 µs | Session creation via bridge |
| serialization/jsonl_batch_3 | 855 ns | 3 events → JSONL |
| serialization/to_value | 111 ns | Single event → serde_json::Value |
| serialization/jsonl_bulk_150 | 28.9 µs | 150 events → JSONL (realistic turn) |
| session/resolve_existing | 72.6 µs | Resolve existing session (with fs2 lock) |
| session/resolve_new | 174.2 µs | Create new session (with dir + lock file) |
| session/validate_valid | 10.7 ns | Validate session ID (regex-based) |
| session/validate_invalid | 31.2 ns | Validate invalid path-traversal ID |
| system_prompt/build_en | 222 ns | English system prompt generation |
| system_prompt/build_cn | 229 ns | Chinese system prompt generation |

## Key Observations

- **Tool registry is extremely fast**: ~40ns per register/remove, ~3µs for metadata from 50 tools
- **Agent construction scales linearly**: ~0.15µs per additional tool (phi-agent level)
- **File I/O dominates**: list_100_flat (233µs) is the slowest benchmark, expected for directory traversal
- **Serialization is cheap**: 150 events → JSONL in under 30µs — event logging won't bottleneck
- **Session creation cost**: ~174µs for a fresh session (includes directory creation + file lock)

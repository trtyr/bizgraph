# BIZGRAPH

**Business graph extractor for HAR traffic captures.** CLI + library — parses `.har` files into deterministic business graphs, persists to SQLite, generates AI analysis reports.

**Stack**: clap 4 · url 2 · rusqlite 0.31 · reqwest 0.12 · tokio 1 · serde · uuid · chrono

## STRUCTURE

```text
bizgraph/
├── Cargo.toml              # Single crate — [lib] + [[bin]]
├── install.sh              # Build release + install to ~/.local/bin
├── src/
│   ├── main.rs             # CLI binary — clap derive, analyze + project subcommands
│   ├── lib.rs              # Public API: analyze(), analyze_with_project(), load_config()
│   ├── types.rs            # All shared types — BusinessGraph, BusinessNode, BusinessEdge, Project, AnalysisRecord
│   ├── error.rs            # Custom Error enum — typed errors with From impls
│   ├── parser.rs           # HAR file parsing, TrafficRow extraction, host filtering
│   ├── graph.rs            # Deterministic node/edge construction, path normalization, schema inference
│   ├── db.rs               # SQLite persistence — projects, node/edge upsert, graph merge, analysis history
│   └── ai/
│       ├── mod.rs          # Re-export: analyze_with_ai(), analyze_with_ai_deep()
│       ├── prompts.rs      # System prompts, agent identity, token limits
│       ├── chat.rs         # Chat API types + HTTP client (reqwest)
│       ├── agent.rs        # Agent state, 4-phase orchestration, state updates
│       └── summarization.rs # Graph serialization for prompts, text parsing utilities
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add/change a shared type | `src/types.rs` | All graph types, Project, AnalysisRecord live here |
| Adjust HAR parsing | `src/parser.rs` | HAR deserialization, TrafficRow extraction, host filtering |
| Change graph mapping | `src/graph.rs` | Stable keys, path templates, schema inference, edge construction |
| Modify DB schema/queries | `src/db.rs` | rusqlite, WAL mode, upsert logic |
| Change error handling | `src/error.rs` | Error enum, Display impl, From conversions |
| Change AI prompts/workflow | `src/ai/` | `prompts.rs` for prompts, `agent.rs` for phases, `chat.rs` for API client |
| Add CLI flags/subcommands | `src/main.rs` | clap derive only |
| Change public API | `src/lib.rs` | Keep `pub fn analyze(...)` stable |

## CODE MAP

### Entry Points

| Entry | File | Signature |
|-------|------|-----------|
| `main` | `src/main.rs:62` | `async fn main()` |
| `analyze` | `src/lib.rs:30` | `fn analyze(har_path, host_filter) -> Result<BusinessGraph>` |
| `analyze_with_project` | `src/lib.rs:52` | `async fn analyze_with_project(har_path, host_filter, project_name_or_id, ...) -> Result<AnalysisResult>` |
| `analyze_with_ai_report` | `src/lib.rs:35` | `async fn analyze_with_ai_report(har_path, host_filter, api_key, model, api_url, deep) -> Result<(BusinessGraph, String)>` |
| `load_config` | `src/lib.rs:105` | `fn load_config() -> Result<(String, String, String)>` |
| `try_load_config` | `src/lib.rs:134` | `fn try_load_config() -> Option<(String, String, String)>` |

### Hot Symbols (by connectivity)

| Symbol | Kind | File | Role |
|--------|------|------|------|
| `AgentState::new` | method | `ai/agent.rs` | Constructs agent state from BusinessGraph |
| `BusinessGraph` | struct | `types.rs:128` | Central graph type — all modules depend on it |
| `build_graph_summary` | fn | `ai/summarization.rs` | Serializes graph for AI prompt |
| `run_agent` | fn | `ai/agent.rs` | Multi-phase agent loop |
| `parse_har` | fn | `parser.rs:113` | HAR → TrafficRow extraction |
| `TrafficRow` | struct | `parser.rs:8` | Normalized traffic record |

### Module Dependency (fan-in / fan-out)

| Module | Fan-in | Fan-out | Total | Notes |
|--------|--------|---------|-------|-------|
| `types.rs` | 7 | 0 | 7 | Most depended-on — pure data, no deps |
| `lib.rs` | 3 | 5 | 8 | Orchestrator — wires all modules |
| `db.rs` | 3 | 1 | 4 | Depends on types only |
| `graph.rs` | 2 | 2 | 4 | Depends on types, used by lib |
| `main.rs` | 1 | 3 | 4 | CLI — depends on lib |
| `ai/` | 2 | 1 | 3 | Depends on types, used by lib |
| `parser.rs` | 3 | 0 | 3 | Depends on url+serde_json, used by lib+graph |
| `error.rs` | 6 | 0 | 6 | Most depended-on — all modules use it |

## HEALTH

- **Score**: 97/100
- **Cycles**: none
- **God modules**: none (ai.rs split into ai/ module)
- **Dead code**: none detected

## CLI

```bash
# Analyze HAR traffic — AI analysis runs automatically if API key is configured
bizgraph analyze traffic.har --project myproject

# Filter by host
bizgraph analyze traffic.har --project myproject --host target.com

# Skip AI analysis
bizgraph analyze traffic.har --project myproject

# Project management
bizgraph project new myproject
bizgraph project list
bizgraph project show myproject        # shows stats, graph metrics, business tree, AI report preview
bizgraph project history myproject
bizgraph project export myproject -o graph.json
bizgraph project viz myproject          # interactive HTML visualization
bizgraph project diff myproject         # compare last two analyses (node diff + report sections)
bizgraph project report myproject       # show full AI report
bizgraph project delete myproject --force
```

## CONFIG

Config files (TOML, no environment variables):

| Location | Purpose |
|----------|---------|
| `~/.config/bizgraph/config.toml` | Global — API key, model, API URL |

```toml
api_key = "sk-..."
model = "deepseek-v4-pro"                # optional, default shown
api_url = "https://api.deepseek.com/chat/completions"  # optional
```

## CONVENTIONS

- **Determinism first**: sort all node/edge collections; derive IDs from stable keys via UUIDv5 (`STABLE_ID_NAMESPACE`), never randomize
- **stable_key format**:
  - Host: `host:<normalized-host>`
  - Business function: `bf:<host>:<path-prefix>`
  - Endpoint: `ep:<method>:<host>:<path-template>`
- **Edge labels**: `contains` (host→bf), `calls_after` (sequential flow), `data_dependency:*` (shared data)
- **serde**: all enums/structs use `#[serde(rename_all = "snake_case")]`
- **Sorted maps**: use `BTreeMap`/`BTreeSet` for deterministic serialization
- **Error handling**: custom `Error` enum in `error.rs` with `From` impls — no `Result<_, String>`
- **Properties**: tagged enum `BusinessNodeProperties` with `#[serde(tag = "kind", content = "details")]`
- **Tests**: inline `#[cfg(test)] mod tests` at file bottom, behavior-descriptive names (not `test_xxx`)

## ANTI-PATTERNS

- Don't make IDs random for the same input file
- Don't stuff parser-only helpers into `main.rs`
- Don't change stable key shapes without updating docs and downstream expectations
- Don't use `HashMap` where sort order matters — use `BTreeMap`
- Don't add security/vulnerability analysis to AI prompts — business analysis only

## NOTES

- HAR format is the sole input — standard JSON (HTTP Archive 1.2), no Excel support
- `url` crate handles URL parsing; `serde_json` handles HAR deserialization
- `ai/` was the god module (1623 lines) — now split into sub-modules (prompts, chat, agent, summarization)
- `bizgraph.db` is stored in `~/.config/bizgraph/`; `.gitignore` excludes `*.db`
- `parser.rs` has 10 unit tests (7 parsing + 3 validation); `graph.rs` has 83 unit tests
- AI defaults to DeepSeek API; compatible with any OpenAI-format endpoint
- DB column `excel_path` kept for backward compatibility; Rust field is `source_path`

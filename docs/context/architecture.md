# Architecture

## System Overview

Bizgraph is a Rust CLI + library that converts `.har` (HTTP Archive) traffic captures into deterministic business graphs, persists them to SQLite, and optionally generates AI analysis reports.

Single crate: `[lib]` + `[[bin]]`.

```
┌─────────────────────────────────────────────────────┐
│                      main.rs (CLI)                  │
│  clap derive: Analyze | Project {New,List,Show,...} │
└───────────────────────────┬─────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────┐
│                    lib.rs (Public API)               │
│  analyze() · analyze_with_ai_report()               │
│  analyze_with_project() · load_config()             │
└──┬──────────┬──────────┬──────────┬─────────────────┘
   │          │          │          │
   ▼          ▼          ▼          ▼
parser.rs  graph.rs    ai/       db.rs
HAR→Rows   Rows→Graph  Graph→Report  Graph↔SQLite
```

## Layer Model

| Layer | Module | Responsibility |
|-------|--------|----------------|
| CLI | `main.rs` | clap parsing, terminal output formatting |
| Orchestrator | `lib.rs` | Wires parse→graph→AI→DB pipeline, config loading |
| Parse | `parser.rs` | HAR deserialization, `TrafficRow` extraction, host filtering |
| Graph | `graph.rs` | Deterministic node/edge construction, path normalization, schema inference |
| AI | `ai/mod.rs` | Entry: `analyze_with_ai`, `analyze_with_ai_deep`, `identify_business_functions` |
| AI/Pre-analysis | `ai/business.rs` | Session grouping, endpoint sampling, AI-based business function identification |
| AI/Agent | `ai/agent.rs` | Multi-phase agent loop (`run_agent`), `AgentState`, token budget management |
| AI/Chat | `ai/chat.rs` | OpenAI-format HTTP client (`chat_fresh`), request/response types |
| AI/Prompts | `ai/prompts.rs` | System prompts, agent identity constants |
| AI/Summarization | `ai/summarization.rs` | Graph serialization for prompts (`build_graph_summary`, `build_graph_overview`) |
| Persistence | `db.rs` | SQLite (WAL mode), upsert/merge logic, project CRUD, analysis history |
| Types | `types.rs` | All shared structs/enums — zero dependencies on other modules |
| Errors | `error.rs` | Custom `Error` enum with `From` impls |

## Module Dependency Graph

```
error.rs ◄──────────────────────────────────────────────────┐
  ▲    (depended on by all modules)                         │
  │                                                         │
types.rs ◄─────────────────────────────────────────────────┐│
  ▲    (depended on by: lib, graph, db, ai/*, parser)      ││
  │                                                        ││
  ├────────────────────────────────────────────────────────┘│
  │                                                         │
lib.rs ──► parser.rs ──► (url, serde_json)                  │
  │   ──► graph.rs ──► types.rs                             │
  │   ──► ai/mod.rs ──► ai/business.rs                      │
  │                   ──► ai/agent.rs ──► ai/chat.rs         │
  │                                ──► ai/summarization.rs   │
  │                                ──► ai/prompts.rs         │
  │   ──► db.rs ──► (rusqlite, types.rs)                    │
  │                                                         │
main.rs ──► lib.rs (public API)                              │
         ──► types.rs (for display formatting)               │
         ──► db.rs (Database direct for project commands)    │
```

**Fan-in (most depended-on):**

| Module | Fan-in | Notes |
|--------|--------|-------|
| `types.rs` | 7 | Pure data, zero internal deps |
| `error.rs` | 6 | All modules use `Result<T>` |
| `lib.rs` | 1 | Only `main.rs` imports it |

## Core Types

```rust
// types.rs — the graph model

BusinessNodeKind = Host | BusinessFunction | Endpoint

BusinessNode {
    id: Uuid,                    // deterministic from stable_key
    stable_key: String,          // format: "host:<h>" | "bf:<h>:<pfx>" | "ep:<M>:<h>:<tpl>"
    label: String,
    kind: BusinessNodeKind,
    properties: BusinessNodeProperties,  // tagged enum
}

BusinessEdge {
    id: Uuid,
    source_node_id: Uuid,
    target_node_id: Uuid,
    label: String,               // "contains" | "calls_after" | "data_dependency:*"
    properties: serde_json::Value,
}

BusinessGraph { nodes: Vec<BusinessNode>, edges: Vec<BusinessEdge> }

// Deterministic IDs: Uuid::new_v5(STABLE_ID_NAMESPACE, stable_key)
```

## Data Flow

### 1. Analyze Pipeline (`analyze_with_project`)

```
HAR file
  │
  ▼
parse_har() ──────────────────────────────────────────────────────┐
  │ Vec<TrafficRow>                                                │
  │ { method, host, path, status, req_headers, resp_headers,       │
  │   req_body, resp_body, timestamp }                             │
  ▼                                                                │
Incremental filter ─── new_rows vs existing DB endpoints            │
  │                                                                │
  ├─ new endpoints exist ──► identify_business_functions()          │
  │    │ (AI pre-analysis)                                         │
  │    ▼                                                           │
  │  BusinessIdentification { business_functions: [...] }           │
  │    │                                                           │
  │    ▼                                                           │
  │  build_business_graph_from_ai(rows, identification)             │
  │    │                                                           │
  │    ▼                                                           │
  │  BusinessGraph                                                 │
  │    │                                                           │
  ├─ no new endpoints ──► build_business_graph(rows)  ◄────────────┘
  │                        (deterministic, no AI)
  ▼
db.merge_graph(project_id, graph)
  │  upsert_node() × N
  │  upsert_edge() × N
  │  refresh_business_function_counts()
  ▼
db.get_graph(project_id)  ──► full graph from DB
  │
  ▼
analyze_with_ai_deep(graph, ...)  ──► AI agent generates report
  │
  ▼
db.record_analysis(project_id, stats, report)
  │
  ▼
AnalysisResult { project, graph, stats, ai_report }
```

### 2. AI Agent Pipeline (`run_agent`)

```
AgentState::new(graph)
  │
  ▼
Phase 1: Overview ──► chat_fresh(graph_overview prompt)
  │                   ──► update_state_from_overview()
  ▼
Phase 2: Deep dive ──► per prioritized domain:
  │                    build_function_detail() ──► chat_fresh()
  │                    ──► parse_observations_into_state()
  ▼
Phase 3: Cross-domain ──► build_cross_summary() ──► chat_fresh()
  │
  ▼
Phase 4: Synthesis ──► chat_fresh(synthesis prompt)
  │
  ▼
extract_final_report() ──► Markdown string
```

### 3. Graph Construction (`build_business_graph`)

```
TrafficRow[]
  │
  ├─ filter: is_static_resource()
  │
  ▼
Pass 1: Collect hosts ──► Host nodes (stable_key: "host:<h>")
  │
  ▼
Pass 2: Infer business functions ──► business_path_prefix()
  │                                  BF nodes (stable_key: "bf:<h>:<pfx>")
  │
  ▼
Pass 3: Extract endpoints ──► normalize_path_template()
  │                            Endpoint nodes (stable_key: "ep:<M>:<h>:<tpl>")
  │
  ▼
Pass 4: Build edges
  │  Host ──contains──► BF ──contains──► Endpoint
  │  Endpoint ──calls_after──► Endpoint (sequential flow)
  │  Endpoint ──data_dependency:*──► Endpoint (shared data)
  │
  ▼
BusinessGraph { nodes: sorted, edges: sorted }
```

## Database Schema

SQLite with WAL mode. Located at `~/.config/bizgraph/bizgraph.db`.

| Table | PK | Key columns |
|-------|----|-------------|
| `projects` | `id` (UUID text) | `name` (unique) |
| `business_nodes` | `(project_id, stable_key)` | `id`, `kind`, `properties` (JSON), timestamps |
| `business_edges` | `(project_id, source_node_id, target_node_id, label)` | `properties` (JSON) |
| `analyses` | `id` (UUID text) | `project_id`, `row_count`, `ai_report`, `node_snapshot`, stats |

**Merge strategy:** `upsert_node` / `upsert_edge` — insert-or-update on conflict. No deletes during merge.

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Deterministic IDs via UUIDv5** | Same input → same graph. Enables reliable diffing and incremental merge. |
| **Sorted collections (BTreeMap/BTreeSet)** | Deterministic JSON serialization. Same input → byte-identical output. |
| **Stable key format** (`host:`, `bf:`, `ep:` prefixes) | Enables graph merge across HAR captures without ID collision. |
| **WAL mode SQLite** | Concurrent reads during long AI calls. Single-writer is fine for CLI. |
| **Incremental analysis** | Only new endpoints hit the AI. Existing data preserved. Reduces cost and latency. |
| **AI pre-analysis (`business.rs`) + deep agent (`agent.rs`)** | Two-phase: first identify business functions, then deep-dive per function. Falls back to deterministic-only if no API key. |
| **Tagged enum for node properties** | `#[serde(tag = "kind", content = "details")]` — polymorphic node data without trait objects. |
| **`ai/` split into sub-modules** | Was 1623-line god module. Now: prompts, chat, agent, summarization, business. |

## Tradeoffs

| Tradeoff | Impact |
|----------|--------|
| Single-crate `lib`+`bin` | Simple build, no workspace overhead. But: can't import library without pulling all deps. |
| `Mutex<Connection>` in `Database` | Simple concurrency model. But: not async-safe — tokio task could block on mutex. Acceptable for CLI. |
| `serde_json::Value` for edge properties | Flexible schema. But: no compile-time type checking on edge metadata. |
| Graph rebuild from DB after merge | Always reflects persisted state. But: extra DB round-trip after every analysis. |
| AI fallback to deterministic | Graceful degradation. But: two code paths to maintain (`build_business_graph` vs `build_business_graph_from_ai`). |

## Entry Points

| Entry | File:Line | Signature |
|-------|-----------|-----------|
| CLI binary | `main.rs:76` | `async fn main()` |
| `analyze` | `lib.rs:30` | `fn analyze(har_path, host_filter) -> Result<BusinessGraph>` |
| `analyze_with_ai_report` | `lib.rs:35` | `async fn analyze_with_ai_report(har, host, key, model, url, deep) -> Result<(Graph, String)>` |
| `analyze_with_project` | `lib.rs:62` | `async fn analyze_with_project(har, host, project, key?, model?, url?, report?) -> Result<AnalysisResult>` |
| `load_config` | `lib.rs:195` | `fn load_config() -> Result<(String, String, String)>` |

## Start Here

`src/lib.rs` — the orchestrator. Every pipeline flows through it. Read lines 30–181 to understand the three analyze variants and how they wire parser→graph→AI→DB.

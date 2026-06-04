# Module Reference

Organized by functional area. Each section covers path, responsibility, public API, internal dependencies, and notable patterns.

---

## CLI Layer

**Path:** `src/main.rs`
**Responsibility:** Parse CLI arguments via clap derive, dispatch to `lib.rs` orchestration functions, render terminal output.

### Public API

| Symbol | Kind | Purpose |
|--------|------|---------|
| `Command::Analyze` | subcommand | End-to-end HAR analysis with project persistence |
| `Command::Project` | subcommand | Project management (new, list, show, history, export, viz, diff, delete, report) |

### Internal Dependencies

- `bizgraph::analyze_with_project` — the core analysis pipeline
- `bizgraph::try_load_config` — optional AI config resolution
- `bizgraph::Database::open_default` — all project subcommands
- `bizgraph::types::*` — used directly for display rendering (`BusinessNodeKind`, `BusinessNodeProperties`)

### Notable Patterns

- All AI config is resolved before calling `analyze_with_project`; `None` signals "skip AI"
- `print_business_tree` (line 570) reconstructs the host→BF→endpoint hierarchy from flat `contains` edges at display time
- `generate_viz_html` (line 664) produces a self-contained HTML file using vis-network, embedded as a raw string
- `resolve_project` (line 420) normalizes name-or-UUID lookups for all project subcommands
- `print_graph_metrics` (line 486) computes fan-in/fan-out/cross-host at display time, not persisted

---

## Orchestration & Config

**Path:** `src/lib.rs`
**Responsibility:** Public API surface — wires parser, graph, db, and ai modules together. Loads TOML config from `~/.config/bizgraph/config.toml`.

### Public API

| Symbol | Kind | Signature |
|--------|------|-----------|
| `analyze` | fn | `(har_path, host_filter) → Result<BusinessGraph>` |
| `analyze_with_ai_report` | async fn | `(har_path, host_filter, api_key, model, api_url, deep) → Result<(BusinessGraph, String)>` |
| `analyze_with_project` | async fn | `(har_path, host_filter, project_name_or_id, api_key?, model?, url?, ai_report?) → Result<AnalysisResult>` |
| `load_config` | fn | `() → Result<(String, String, String)>` — (api_key, model, api_url) |
| `load_api_key` | fn | `() → Result<String>` |
| `try_load_config` | fn | `() → Option<(String, String, String)>` |
| `Database` | re-export | `pub use db::Database` |
| `Error`, `Result` | re-export | `pub use error::{Error, Result}` |

### Internal Dependencies

- `parser::parse_har` — traffic extraction
- `graph::{build_business_graph, build_business_graph_from_ai}` — graph construction
- `ai::{analyze_with_ai, analyze_with_ai_deep, identify_business_functions}` — AI analysis
- `db::Database` — persistence and project management

### Notable Patterns

- `analyze_with_project` implements incremental analysis: detects new vs existing endpoints via `db.get_endpoint_keys`, skips AI identification when no new endpoints found
- Falls back gracefully from AI-identified business functions to URL-based grouping on AI failure (`Err(_) => build_business_graph`)
- Records a node snapshot (JSON array of `stable_key`s) per analysis for diff comparison
- Config defaults: model = `deepseek-v4-flash`, api_url = `https://api.deepseek.com/chat/completions`
- `build_business_context` (line 184) serializes AI identification results into a plain-text context string for the deep analysis agent

---

## Traffic Ingestion

**Path:** `src/parser.rs`
**Responsibility:** Deserialize HAR (HTTP Archive 1.2) JSON files into `TrafficRow` structs. Validate structure, extract normalized fields.

### Public API

| Symbol | Kind | Purpose |
|--------|------|---------|
| `TrafficRow` | struct | Normalized traffic record with url, method, host, path, status, headers, bodies, timing |
| `parse_har` | fn | `(har_path, host_filter?) → Result<Vec<TrafficRow>>` |

### Internal Dependencies

- `error::{Error, Result}` — error types (`IoContext`, `Validation`, `JsonContext`)
- `serde_json` — HAR deserialization
- `url` — URL parsing and host extraction

### Notable Patterns

- HAR deserialization types (`HarFile`, `HarLog`, `HarEntry`, `HarRequest`, `HarResponse`, etc.) are private to this module
- Host filtering is prefix-based: `filter(|h| h.starts_with(host_filter))`
- `extract_port` (line 267) parses port from URL or infers from scheme (80/443)
- `format_headers` (line 278) serializes header arrays to `name: value\n` format
- 10 unit tests: 7 parsing tests + 3 validation tests (empty file, non-JSON, missing `log` field)

---

## Graph Construction

**Path:** `src/graph.rs`
**Responsibility:** Deterministic transformation from `TrafficRow` list to `BusinessGraph`. Path normalization, schema inference, edge construction, static resource filtering.

### Public API

| Symbol | Kind | Purpose |
|--------|------|---------|
| `build_business_graph` | fn | `(rows: &[TrafficRow]) → Result<BusinessGraph>` — URL-based grouping |
| `build_business_graph_from_ai` | fn | `(rows, identification) → Result<BusinessGraph>` — AI-identified grouping |
| `is_static_resource` | fn | `(path) → bool` — filters JS/CSS/images/fonts |
| `normalize_path_template` | fn | `(path) → String` — replaces dynamic segments with `{param}` |

### Internal Dependencies

- `types::*` — all node/edge types, `deterministic_id`
- `ai::BusinessIdentification` — for AI-identified graph variant
- `parser::TrafficRow` — input data

### Notable Patterns

- **Determinism**: all collections sorted by `stable_key` before ID derivation via `Uuid::new_v5(STABLE_ID_NAMESPACE, key)`
- **Stable key formats**: `host:<host>`, `bf:<host>:<path-prefix>`, `ep:<method>:<host>:<path-template>`
- **Edge types**: `contains` (host→BF, BF→endpoint), `calls_after` (sequential traffic), `data_dependency:*` (shared values between requests)
- `EndpointAccumulator` (line ~80) tracks per-endpoint state: methods, status codes, parameters, request/response bodies, schema candidates
- `SchemaShape` inference: derives JSON schema structure from observed request/response bodies
- `business_path_prefix` (line 711) determines BF grouping prefix from URL path
- 83 unit tests covering determinism, stable keys, edge construction, confidence scoring, parameter inference

---

## Persistence

**Path:** `src/db.rs`
**Responsibility:** SQLite storage for projects, nodes, edges, and analysis history. WAL mode, foreign keys, upsert logic.

### Public API

| Symbol | Kind | Purpose |
|--------|------|---------|
| `Database` | struct | Thread-safe SQLite wrapper (`Mutex<Connection>`) |
| `Database::open_default` | fn | Opens `~/.config/bizgraph/bizgraph.db` |
| `Database::open` | fn | Opens arbitrary path, runs schema migration |
| `create_project` | fn | Creates project, returns `Project` |
| `list_projects` | fn | All projects sorted by creation date |
| `get_project` / `get_project_by_name` | fn | Lookup by UUID or name |
| `resolve_project` | fn | Name-or-UUID resolution with ambiguity detection |
| `upsert_node` / `upsert_edge` | fn | Insert-or-update with change detection (returns `bool` = changed) |
| `merge_graph` | fn | Batch upsert of a full `BusinessGraph` → `AnalysisStats` |
| `clear_business_functions` | fn | Remove URL-based BF nodes (after AI re-identification) |
| `get_graph` | fn | Reconstruct full `BusinessGraph` from DB |
| `get_endpoint_keys` | fn | All endpoint stable_keys for incremental analysis |
| `record_analysis` | fn | Persist analysis metadata, stats, AI report, node snapshot |
| `get_latest_analysis` / `get_analysis_history` | fn | Analysis record retrieval |
| `delete_project` | fn | Cascade delete (project + nodes + edges + analyses) |

### Internal Dependencies

- `types::*` — all graph types, `Project`, `AnalysisRecord`, `AnalysisStats`
- `error::{Error, Result}` — `SqliteContext`, project error variants

### Notable Patterns

- Schema: `business_nodes` PK = `(project_id, stable_key)`, `business_edges` PK = `(project_id, source_node_id, target_node_id, label)`
- `merge_graph` (line 333) returns `AnalysisStats` with counts of new/updated/skipped items
- DB column `excel_path` kept for backward compatibility; Rust field is `source_path`
- `resolve_project` (line 175) handles ambiguity: if multiple name matches exist, returns `AmbiguousProject` error
- UUID columns stored as TEXT (RFC 4122 format), parsed back via `Uuid::parse_str`

---

## Shared Types

**Path:** `src/types.rs`
**Responsibility:** All shared data structures. Zero fan-out (pure data, no module dependencies).

### Public API

| Symbol | Kind | Purpose |
|--------|------|---------|
| `BusinessNodeKind` | enum | `Host`, `BusinessFunction`, `Endpoint` |
| `BusinessNodeProperties` | tagged enum | `BusinessFunction(BusinessFunctionProperties)`, `Endpoint(EndpointProperties)`, `Host(BTreeMap)` |
| `BusinessNode` | struct | `{id, stable_key, label, kind, properties}` |
| `BusinessEdge` | struct | `{id, source_node_id, target_node_id, label, properties}` |
| `BusinessGraph` | struct | `{nodes: Vec<BusinessNode>, edges: Vec<BusinessEdge>}` |
| `EndpointProperties` | struct | path_template, methods, status_codes, schemas, parameters, confidence |
| `BusinessFunctionProperties` | struct | host, path_prefix, endpoint_count, description |
| `SchemaShape` / `SchemaType` | struct/enum | Recursive JSON schema representation |
| `ParameterDescriptor` / `ParameterKind` / `ParameterLocation` | struct/enum | Endpoint parameter metadata |
| `StatusProfiles` | struct | Success/redirect/client_error/server_error counts |
| `Project` | struct | `{id, name, created_at}` |
| `AnalysisRecord` | struct | Full analysis metadata including AI report and node snapshot |
| `AnalysisStats` | struct | `{row_count, new_nodes, updated_nodes, new_edges, skipped_edges}` |
| `AnalysisResult` | struct | `{project, graph, stats, ai_report}` |
| `deterministic_id` | fn | `Uuid::new_v5(STABLE_ID_NAMESPACE, stable_key)` |
| `STABLE_ID_NAMESPACE` | const | UUIDv5 namespace for stable ID derivation |

### Notable Patterns

- `BusinessNodeProperties` uses `#[serde(tag = "kind", content = "details")]` for JSON polymorphism
- All enums use `#[serde(rename_all = "snake_case")]`
- `BTreeMap` / `BTreeSet` used for deterministic serialization order
- `BusinessImport*` types exist for external import scenarios (not used internally)

---

## Error Handling

**Path:** `src/error.rs`
**Responsibility:** Unified `Error` enum with `From` impls for all upstream error types. Zero fan-out.

### Public API

| Symbol | Kind | Purpose |
|--------|------|---------|
| `Error` | enum | 25+ variants covering IO, SQLite, JSON, HTTP, UUID, Chrono, TOML, config, API, validation, domain |
| `Result<T>` | type alias | `std::result::Result<T, Error>` |

### Notable Error Variants

| Variant | Context |
|---------|---------|
| `IoContext` / `SqliteContext` / `JsonContext` / `TomlContext` | Wraps source error with descriptive context string |
| `ApiResponse` | HTTP status + truncated body + URL |
| `BudgetExceeded` | Agent token/call budget exceeded |
| `ProjectNotFound` / `AmbiguousProject` / `ProjectAlreadyExists` | Project lifecycle errors |
| `ConfigMissingApiKey` / `ConfigRead` / `ConfigParse` | Config file errors |
| `Validation` | Generic validation with message |

### Notable Patterns

- Constructor helpers: `Error::io(ctx, src)`, `Error::sqlite(ctx, src)`, `Error::json(ctx, src)`, `Error::toml(ctx, src)`, `Error::validation(msg)`
- Implements `std::error::Error` with proper `source()` chain
- Implements `From` for: `std::io::Error`, `rusqlite::Error`, `serde_json::Error`, `reqwest::Error`, `uuid::Error`, `chrono::ParseError`, `toml::de::Error`
- Most depended-on module (6 modules import it)

---

## AI Analysis Module

**Path:** `src/ai/`
**Responsibility:** AI-powered business analysis — single-shot reports, multi-phase agent analysis, and business function identification.

### Module Entry — `src/ai/mod.rs`

**Public API:**

| Symbol | Kind | Purpose |
|--------|------|---------|
| `analyze_with_ai` | async fn | Single-shot: send graph summary to LLM, return Markdown report |
| `analyze_with_ai_deep` | async fn | Multi-phase agent analysis (4 phases) |
| `identify_business_functions` | async fn | AI identifies business function groupings from raw traffic |
| `BusinessIdentification` | struct | AI response: list of `BusinessFunctionGroup` |
| `BusinessFunctionGroup` | struct | `{name, description, endpoints: Vec<EndpointMapping>}` |
| `EndpointMapping` | struct | `{method, path, host}` |
| `prompts::*` | re-export | All prompt constants and limit values |

### Internal Module Dependency

```
mod.rs → agent.rs → {chat.rs, summarization.rs, prompts.rs}
       → business.rs → {chat.rs, prompts.rs, graph::is_static_resource}
       → summarization.rs → {types.rs, prompts.rs limits}
       → chat.rs → {error.rs}
       → prompts.rs → (standalone constants)
```

---

### Chat Client — `src/ai/chat.rs`

**Responsibility:** OpenAI-compatible HTTP chat client with retry logic.

| Symbol | Kind | Purpose |
|--------|------|---------|
| `ChatMessage` | struct | `{role, content}` with `::system()` / `::user()` constructors |
| `ChatRequest` | struct | `{model, messages, stream: false}` |
| `ChatResponse` | struct | Response deserialization |
| `chat_fresh` | async fn | Create request and send (stateless) |
| `send_chat_request` | async fn | HTTP POST with exponential backoff retry (3 retries, 1s/2s/4s) |

- Retries on 429 (rate limit) and 5xx errors only
- Uses `reqwest::Client::new()` per call (no connection pooling)

---

### Agent — `src/ai/agent.rs`

**Responsibility:** 4-phase analysis agent: OVERVIEW → DOMAIN (per domain) → CROSS → FINAL.

| Symbol | Kind | Purpose |
|--------|------|---------|
| `run_agent` | async fn | `(graph, api_key, model, url, business_context?) → Result<String>` |
| `AgentState` | struct | Mutable state: phase, domains, observations, cross_cutting, progress, token budget, turn responses |
| `DomainAnalysis` | struct | Per-domain analysis state (name, priority, endpoint_count, analyzed, summary) |
| `BusinessObservation` | struct | `{title, evidence, endpoints, notes}` |
| `TokenBudget` | struct | `{used, limit}` with enforcement |

- Budget: `MAX_DEEP_AI_CALLS = 7`, `AGENT_STATE_TOKEN_LIMIT = 100,000` tokens
- If `business_context` is provided, pre-populates domain knowledge instead of re-discovering
- `build_turn_context` (line 220) constructs per-turn prompt with accumulated state
- `force_summarize_context` (line 322) compresses context when approaching token limits
- `extract_final_report` (line 588) compiles accumulated observations into Markdown

---

### Summarization — `src/ai/summarization.rs`

**Responsibility:** Serialize `BusinessGraph` into text formats for AI prompts.

| Symbol | Kind | Purpose |
|--------|------|---------|
| `build_graph_summary` | fn | Full graph → structured text (used by `analyze_with_ai`) |
| `build_graph_overview` | fn | High-level overview for agent OVERVIEW phase |
| `build_function_detail` | fn | Detailed endpoint dump for a single domain (used by agent) |
| `build_cross_domain_summary` | fn | Cross-domain correlation summary |
| `extract_cross_cutting_items` | fn | Parse cross-cutting observations from AI responses |
| `prioritized_function_names` | fn | Extract ordered domain list from AI overview response |
| `parse_observations_from_response` | fn | Parse structured observations from AI text |
| `summarize_text` | fn | Truncate text to char limit |

- `MAX_ENDPOINTS_PER_DOMAIN = 20`, `SAMPLE_BODY_CHAR_LIMIT = 2,000`, `SUMMARY_HARD_CHAR_LIMIT = 80,000`

---

### Business Identification — `src/ai/business.rs`

**Responsibility:** AI-powered grouping of traffic into business functions. Pre-processes traffic into sessions and endpoint samples before sending to LLM.

| Symbol | Kind | Purpose |
|--------|------|---------|
| `identify_business_functions` | async fn | `(rows, api_key, model, url) → Result<BusinessIdentification>` |
| `group_sessions` | fn | Group traffic rows by 30-second time gaps |
| `Session` | struct | `{index, endpoints: Vec<String>}` |
| `EndpointSample` | struct | `{method, path, request_body, response_body, status}` |
| `BusinessIdentification` | struct | `{business_functions: Vec<BusinessFunctionGroup>}` |

- Session gap: 30 seconds between requests = new session
- Collects up to 2 samples per endpoint (first + last occurrence)
- Filters static resources before building endpoint list
- Response parsed as JSON into `BusinessIdentification`; falls back to `build_business_graph` on failure

---

### Prompts — `src/ai/prompts.rs`

**Responsibility:** System prompts, agent identity, and operational limits (pure constants).

| Symbol | Kind | Purpose |
|--------|------|---------|
| `SYSTEM_PROMPT` | const | Business analyst persona for single-shot analysis |
| `AGENT_IDENTITY_PROMPT` | const | Multi-phase agent persona with explicit workflow |
| `BUSINESS_ID_PROMPT` | const | Business identification task prompt |
| `MAX_DEEP_AI_CALLS` | const | `7` — agent call budget |
| `AGENT_STATE_TOKEN_LIMIT` | const | `100,000` |
| `TURN_DATA_CHAR_LIMIT` | const | `200,000` |
| `SUMMARY_HARD_CHAR_LIMIT` | const | `80,000` |

- All prompts explicitly forbid security/vulnerability analysis — business analysis only

# Architecture Review

Reviewed: 2026-06-05 | Codebase: bizgraph (single Rust crate, CLI + lib)

---

## Overall Grade: A-

A well-structured single-crate project with clear layer separation, deterministic design principles, and pragmatic pattern choices. The `ai/` module split from a 1623-line god module into 5 focused sub-modules shows good architectural evolution. Two structural issues prevent an A: CLI-layer boundary leakage and high blast radius on foundational types.

---

## 1. Separation of Concerns — **A**

| Aspect | Evidence |
|--------|----------|
| Layer model | 5 clear layers: CLI (`main.rs`) → Orchestrator (`lib.rs`) → Parse/Graph/AI → Persistence (`db.rs`) → Types (`types.rs`) |
| Type isolation | `types.rs` has **zero fan-out** — pure data, no dependencies on other modules (`src/types.rs:1-215`) |
| Error isolation | `error.rs` has **zero fan-out** — depended on by 6 modules but depends on none (`src/error.rs:1-268`) |
| AI module decomposition | Was 1623 lines; now split: `prompts.rs` (constants), `chat.rs` (HTTP client), `agent.rs` (orchestration), `summarization.rs` (serialization), `business.rs` (pre-analysis) |
| Parser encapsulation | HAR deserialization types (`HarFile`, `HarEntry`, etc.) are **private** to `parser.rs` — only `TrafficRow` is exported (`src/parser.rs:7-115`) |

**Recommendation:** None needed. Layer boundaries are clean and consistently enforced.

---

## 2. Dependency Direction — **B+**

| Aspect | Evidence |
|--------|----------|
| Correct: `main.rs` → `lib.rs` → modules | `src/main.rs:86` calls `bizgraph::analyze_with_project()`, the public API |
| **Violation: `main.rs` → `Database` directly** | `src/main.rs:119` — `bizgraph::Database::open_default()` bypasses `lib.rs` for all `Project` subcommands. The CLI layer directly instantiates the persistence layer. |
| **Violation: `main.rs` → `types::*` directly** | `src/main.rs:210,571,630,665` — imports `BusinessNodeKind` and `BusinessNodeProperties` directly for display rendering. |
| Correct: AI → types only | AI modules depend on `types.rs` (data) and `error.rs`, not on `db.rs` or `parser.rs` |
| Correct: graph → parser via types | `graph.rs` consumes `TrafficRow` (a data type), not the parser module itself |

**Evidence of CLI coupling — `src/main.rs:118-126`:**
```rust
Command::Project { action } => {
    let db = match bizgraph::Database::open_default() {
        Ok(db) => db,
        Err(error) => { eprintln!("Error: {error}"); std::process::exit(1); }
    };
    // ... db.create_project(), db.list_projects(), etc. directly
}
```

**Recommendation:** Wrap project CRUD operations in `lib.rs` public functions (e.g., `pub fn list_projects() -> Result<Vec<Project>>`) so `main.rs` never touches `Database` directly. This would make `lib.rs` the true single gateway.

---

## 3. Coupling — **B**

| Symbol | Impact Radius | Files Affected |
|--------|---------------|----------------|
| `BusinessGraph` | **53 symbols** across 8 files | `types.rs`, `ai/agent.rs`, `ai/mod.rs`, `ai/summarization.rs`, `lib.rs`, `db.rs`, `main.rs`, `graph.rs` |
| `TrafficRow` | **51 symbols** across 4 files | `parser.rs`, `ai/business.rs`, `graph.rs`, `lib.rs` |
| `Error` | **6 modules** | All modules import it |

`BusinessGraph` and `TrafficRow` are foundational types — some coupling is inherent and acceptable. However, the blast radius is large: changing a field on `BusinessGraph` touches 8 files and 53 symbols.

| Aspect | Evidence |
|--------|----------|
| Module boundaries are respected | `parser.rs` never imports from `graph.rs`; `ai/` never imports from `db.rs` |
| No circular dependencies | Dependency graph is a DAG: `main → lib → {parser, graph, ai, db} → {types, error}` |
| `ai/chat.rs` has minimal coupling | Only caller is `run_agent` (`src/ai/agent.rs:115`) — clean encapsulation of HTTP transport |

**Recommendation:** Accept current coupling as reasonable for a single-crate CLI tool. If the project grows, consider splitting into a workspace: `bizgraph-core` (types, parser, graph), `bizgraph-db` (persistence), `bizgraph-ai` (analysis).

---

## 4. Cohesion — **A**

| Module | Single Responsibility | Evidence |
|--------|----------------------|----------|
| `parser.rs` | HAR deserialization only | Exports 2 symbols: `TrafficRow`, `parse_har`. 10 unit tests. |
| `graph.rs` | Deterministic graph construction | Exports 4 symbols. 83 unit tests. No I/O. |
| `db.rs` | SQLite persistence | All DB operations, schema migration, project CRUD in one place. |
| `ai/chat.rs` | HTTP transport only | `ChatMessage`, `ChatRequest`, `ChatResponse`, `chat_fresh`, `send_chat_request` — nothing else. |
| `ai/agent.rs` | Multi-phase orchestration | `AgentState`, `run_agent`, phase transitions, token budget. |
| `ai/prompts.rs` | Pure constants | Zero logic, zero dependencies. |
| `ai/summarization.rs` | Graph → text serialization | 8 functions, all deterministic, no I/O. |
| `ai/business.rs` | Pre-analysis session grouping | `group_sessions`, `collect_endpoint_samples`, `identify_business_functions`. |
| `types.rs` | Shared data structures only | Zero fan-out, pure data definitions. |
| `error.rs` | Unified error type only | Zero fan-out, `From` impls for 7 upstream types. |

**Recommendation:** None needed. Every module has a clear, single responsibility.

---

## 5. Extensibility — **B**

| Scenario | Difficulty | Evidence |
|----------|------------|----------|
| Add new graph edge type | **Easy** | Add variant to `BusinessNodeProperties` in `types.rs`, update `graph.rs` construction |
| Add new AI provider | **Easy** | `ai/chat.rs` uses OpenAI-format API — change `api_url` in config |
| Add non-HAR input format | **Hard** | `TrafficRow` is the only intermediate representation; `parser.rs` and `lib.rs` assume HAR. Would need a trait abstraction. |
| Add new output format | **Medium** | JSON export exists in `main.rs:196`; HTML viz in `main.rs:664`. Adding PDF/Markdown would mean new functions in `main.rs`. |
| Add authentication to DB | **Easy** | `Database::open()` takes a path (`src/db.rs:40`); swap to connection string |
| Add streaming/incremental AI | **Medium** | `chat.rs` forces `stream: false` (`src/ai/chat.rs`). Would need streaming refactor. |
| Plugin system for graph transforms | **Hard** | No trait-based extensibility; graph construction is a closed function |

**Key design decisions that enable extensibility:**
- `serde(tag = "kind", content = "details")` on `BusinessNodeProperties` — polymorphic node data without trait objects (`src/types.rs:199`)
- Deterministic IDs via `Uuid::new_v5` — same input always produces same graph, enabling reliable diffing (`src/types.rs:202`)
- AI module split — each sub-module can be extended independently

**Key design decisions that limit extensibility:**
- Single-crate structure — can't depend on library without pulling all deps (documented tradeoff)
- `serde_json::Value` for edge properties — flexible but no compile-time checking (`src/types.rs:97`)
- Two code paths for graph construction: `build_business_graph` vs `build_business_graph_from_ai` — adding a third input source means a third path

**Recommendation:** If non-HAR input formats are planned, introduce a `trait TrafficSource` with `fn parse(&self) -> Result<Vec<TrafficRow>>` and make `parse_har` the first implementation. For now, this would be premature abstraction.

---

## 6. Design Patterns — **A-**

| Pattern | Usage | Assessment |
|---------|-------|------------|
| **Tagged enum over trait objects** | `BusinessNodeProperties` uses `#[serde(tag = "kind", content = "details")]` (`src/types.rs:181`) | Excellent — avoids dynamic dispatch, serde-friendly, compile-time exhaustive matching |
| **Deterministic ID derivation** | `Uuid::new_v5(STABLE_ID_NAMESPACE, stable_key)` (`src/types.rs:202`) | Excellent — same input → same ID across runs, enables reliable merge and diff |
| **Stable key prefixes** | `host:`, `bf:`, `ep:` conventions (`src/types.rs:86`) | Excellent — prevents ID collision across node kinds |
| **Incremental analysis** | `db.get_endpoint_keys()` → filter new rows → AI only on new (`src/lib.rs:79-104`) | Excellent — reduces AI cost and latency on repeated analyses |
| **Graceful AI fallback** | `Err(_) => build_business_graph(&rows)` (`src/lib.rs:51,119`) | Good — works without API key, but two code paths to maintain |
| **`Mutex<Connection>`** | `Database` wraps SQLite in `Mutex` (`src/db.rs:30`) | Acceptable for CLI; documented as not async-safe |
| **Constructor helpers on Error** | `Error::io()`, `Error::sqlite()`, etc. (`src/error.rs:94-127`) | Good — reduces boilerplate at call sites |
| **BTreeMap/BTreeSet** | Used throughout for deterministic serialization (`src/types.rs:201`) | Excellent — byte-identical output for same input |

**Anti-patterns found:** None. No TODO/FIXME/HACK comments in `src/`. No dead code detected. No god modules (post `ai/` split).

**Over-engineering check:** The project avoids premature abstraction well. No unnecessary trait hierarchies, no dependency injection frameworks, no factory patterns. The `ai/` module split was reactive (1623-line file), not speculative.

**Recommendation:** The `send_chat_request` creates a new `reqwest::Client` per call (documented in `modules.md`). If the project adds concurrent analysis, consider a shared client. For current CLI usage, this is fine.

---

## Summary Table

| Criterion | Grade | Key Strength | Key Risk |
|-----------|-------|--------------|----------|
| Separation of concerns | **A** | Clean 5-layer model, type isolation | — |
| Dependency direction | **B+** | Correct DAG, no cycles | `main.rs` bypasses `lib.rs` for DB access |
| Coupling | **B** | Module boundaries respected | `BusinessGraph` change touches 53 symbols |
| Cohesion | **A** | Every module has single responsibility | — |
| Extensibility | **B** | Tagged enums, deterministic IDs, modular AI | Hard to add non-HAR input; two graph construction paths |
| Design patterns | **A-** | Pragmatic, no over-engineering | `Mutex<Connection>` limits async; new `Client` per call |

---

## Priority Recommendations

1. **[Low effort, medium value]** Wrap project CRUD in `lib.rs` so `main.rs` never touches `Database` directly. 5-6 new `pub fn` wrappers in `lib.rs`.

2. **[Low effort, low value]** Consolidate `use bizgraph::types::*` imports in `main.rs` — the display functions (`print_graph_metrics`, `print_business_tree`, `generate_viz_html`) import types directly. Minor concern; acceptable for a CLI.

3. **[Medium effort, conditional]** If non-HAR input formats are planned: extract `trait TrafficSource` and refactor `parse_har` into an implementation. Not needed now.

4. **[Medium effort, conditional]** If the project grows beyond CLI: split into Cargo workspace (`bizgraph-core`, `bizgraph-db`, `bizgraph-ai`, `bizgraph-cli`). The module boundaries already support this cleanly.

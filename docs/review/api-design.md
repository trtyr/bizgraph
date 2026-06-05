# API Design Review — bizgraph

> **Reviewed:** 2026-06-05
> **Scope:** Library API (`src/lib.rs`, `src/types.rs`, `src/db.rs`, `src/error.rs`), CLI interface (`src/main.rs`), AI module surface (`src/ai/`)
> **Version:** 0.1.1

---

## Summary

| Criterion | Score | One-liner |
|-----------|-------|-----------|
| Consistency | B+ | Naming patterns are clean; function signatures diverge across the three `analyze` variants |
| Discoverability | B | Flat top-level functions are easy to locate; 24 public types lack docs to guide newcomers |
| Backward Compatibility | B | Serde/deterministic keys are stable; no versioning or deprecation policy yet |
| Input Validation | A- | Boundary checks exist at DB and error layer; lib.rs entry points trust callers too much |
| Documentation | C+ | External docs are excellent; source-level doc comments are sparse |
| Minimalism | B | Core graph types are tight; submodules leak internal types unnecessarily |

**Overall: B**

---

## 1. Consistency — Score: B+

**Evidence:**

- `analyze()` (`src/lib.rs:30`) takes 2 params. `analyze_with_ai_report()` (`src/lib.rs:35`) takes 6 positional params. `analyze_with_project()` (`src/lib.rs:62`) takes 7 params, wrapping config in `Option<&str>` tuples. The config triple `(api_key, model, api_url)` is passed as three separate `Option` args instead of a config struct.
- `load_config()` returns `Result<(String, String, String)>` (`src/lib.rs:195`) — a tuple with no field names. `try_load_config()` wraps it in `Option`. `load_api_key()` extracts the first element. These are consistent with each other, but the tuple return type is opaque.
- `Database` methods (`src/db.rs:96-209`) follow a clean `verb_noun` pattern: `create_project`, `list_projects`, `get_project`, `get_project_by_name`, `resolve_project`, `delete_project`. Consistent.
- Error variants mix naming styles: `ProjectNotFound { reference }` vs `ProjectAlreadyExists { name }` vs `AmbiguousProject { matches }`. The field names (`reference` vs `name`) are not standardized.

**Recommendation:**
- Replace the `(String, String, String)` config tuple with a named `Config` struct (currently private in `lib.rs:18-24`; make it public).
- Introduce an `AnalyzeOptions` builder or struct for `analyze_with_project` to replace the 7-parameter signature.
- Standardize error variant field names: use `name_or_id` consistently instead of `reference`/`name`.

---

## 2. Discoverability — Score: B

**Evidence:**

- `src/lib.rs` exposes 6 public functions plus 2 re-exports (`Database`, `Error`). A new developer can scan the file in 2 minutes and locate what they need.
- 8 submodules are `pub mod`: `error`, `ai`, `db`, `graph`, `parser`, `types`. All internal types are accessible via `bizgraph::ai::Session`, `bizgraph::graph::is_static_resource`, etc.
- `types.rs` defines 24 public structs/enums — the core data model. Only 2 have doc comments (`STABLE_ID_NAMESPACE` at line 7, `node_snapshot` at line 205). A developer browsing `BusinessNodeProperties` has no guidance on when to use each variant.
- CLI help strings in `main.rs` are well-written (every subcommand and flag has `///` docs).

**Recommendation:**
- Add doc comments to every public type in `types.rs` — at minimum the 6 core types (`BusinessGraph`, `BusinessNode`, `BusinessEdge`, `BusinessNodeKind`, `BusinessNodeProperties`, `Project`).
- Add a module-level `//!` doc comment to `lib.rs` explaining the two surfaces (library vs CLI) and the three tiers of analysis.
- Consider re-exporting key types from `lib.rs` (e.g. `pub use types::BusinessGraph`) so users do not need to navigate `bizgraph::types::BusinessGraph`.

---

## 3. Backward Compatibility — Score: B

**Evidence:**

- `serde(rename_all = "snake_case")` on all enums/structs (`src/types.rs:11,19,41,49,105`) ensures stable JSON serialization keys.
- Deterministic IDs via `Uuid::new_v5(STABLE_ID_NAMESPACE, stable_key)` (`src/types.rs:173-175`) guarantee same-input -> same-UUID. Stable keys are documented (`api.md:390-394`).
- DB column `excel_path` kept for backward compat (`src/db.rs:75`, `ensure_analysis_node_snapshot_column` pattern at `src/db.rs:88-89`).
- No `#[deprecated]` markers anywhere in the codebase.
- Semver is 0.1.1 — no stability promise. No changelog or migration guide.

**Recommendation:**
- Document the stable serialization contract (field names, `rename_all`) as a non-breakable rule in `CONVENTIONS.md`.
- Add a `BREAKING CHANGES` section to the changelog when bumping from 0.x to 1.0.
- If `excel_path` is legacy, mark it with a deprecation comment explaining the migration path to `source_path`.

---

## 4. Input Validation — Score: A-

**Evidence:**

- `create_project` (`src/db.rs:96-100`): trims input, rejects empty names with `Error::EmptyProjectName`.
- `resolve_project` (`src/db.rs:175-209`): trims input, rejects empty reference with `Error::EmptyProjectReference`, handles ambiguity with `Error::AmbiguousProject`.
- Error enum has `Validation { message }` variant (`src/error.rs:67`) and helper `Error::validation()` (`src/error.rs:122`).
- `InvalidNodeKind`, `InvalidUuidValue`, `InvalidTimestampValue` variants validate deserialized values (`src/error.rs:70-80`).
- Gap: `analyze_with_project` does not validate `project_name_or_id` before passing to `resolve_project` — the DB layer catches it, but the boundary is at `lib.rs`, not at the function entry.
- Gap: `analyze_with_ai_report` accepts empty `api_key`/`model`/`api_url` strings without validation. These propagate to HTTP requests.

**Recommendation:**
- Validate `api_key`, `model`, `api_url` at the `analyze_with_ai_report` boundary (reject empty/whitespace-only strings).
- Validate `har_path` is non-empty and exists before parsing — fail fast with a clear error instead of a low-level I/O error from the parser.
- Add a `confidence` range assertion (0.0-1.0) on `EndpointProperties` during deserialization.

---

## 5. Documentation — Score: C+

**Evidence:**

- **External docs are strong:** `docs/context/api.md` (394 lines), `architecture.md` (249 lines), `modules.md` (358 lines) — comprehensive, accurate, well-structured.
- **Source doc comments are sparse:**
  - `src/lib.rs`: 3 of 7 public items have `///` comments (`analyze` at line 29, `build_business_context` at line 183, `try_load_config` at line 223). Missing: `analyze_with_ai_report`, `analyze_with_project`, `load_config`, `load_api_key`.
  - `src/types.rs`: 2 of 24 public types have doc comments. Zero doc comments on `BusinessGraph`, `BusinessNode`, `BusinessEdge`, `BusinessNodeProperties`, `EndpointProperties`, `AnalysisResult`, etc.
  - `src/db.rs`: 0 doc comments on any of the 16 public methods.
  - `src/ai/mod.rs`: 2 of 3 public functions documented. `identify_business_functions` re-export has no docs.
  - `src/ai/summarization.rs`: 16 public functions, 0 doc comments.
- `cargo doc` generates successfully — but the output is mostly empty because of missing source comments.

**Recommendation:**
- **High priority:** Add `///` doc comments to `analyze_with_ai_report`, `analyze_with_project`, `load_config` in `lib.rs`.
- **High priority:** Add doc comments to the 6 core graph types in `types.rs`.
- **Medium priority:** Add doc comments to `Database` public methods — at minimum `open_default`, `create_project`, `merge_graph`, `get_graph`.
- Add `//!` module docs to `types.rs` and `db.rs` explaining their role.
- Add 1-2 usage examples (Rust doc examples) for `analyze()` to show the simplest path.

---

## 6. Minimalism — Score: B

**Evidence:**

- `BusinessGraph { nodes: Vec<BusinessNode>, edges: Vec<BusinessEdge> }` — minimal, no unnecessary fields.
- `Database` struct is opaque (`conn: Mutex<Connection>`, private field, `src/db.rs:30-32`). Good encapsulation.
- **Leaks:**
  - `pub mod ai` exposes `ai::business::Session`, `ai::business::EndpointSample`, `ai::business::EndpointMapping` — internal grouping types that should be private.
  - `pub use prompts::*` (`src/ai/mod.rs:11`) re-exports all prompt constants — internal implementation details.
  - `types::default_properties()` (`src/types.rs:169`) and `types::deterministic_id()` (`src/types.rs:173`) are helper functions that do not need to be public API.
  - `graph::is_static_resource()` and `graph::normalize_path_template()` (`src/graph.rs:22,628`) are implementation details used by `lib.rs` internally — no external consumer needs them.
  - `BusinessNodeProperties::Host(BTreeMap<String, serde_json::Value>)` (`src/types.rs:109`) is untyped — any JSON is valid. Could use a dedicated `HostProperties` struct.

**Recommendation:**
- Make `ai::business::{Session, EndpointSample, EndpointMapping}` private (remove `pub` or move to `pub(crate)`).
- Replace `pub use prompts::*` with explicit `pub(crate) use` — prompt text is an internal detail.
- Move `default_properties()` and `deterministic_id()` to `pub(crate)` or into `db.rs`/`graph.rs` where they are used.
- Consider `pub(crate)` for `graph::is_static_resource` and `graph::normalize_path_template` unless there is a clear external use case.
- Add a `HostProperties` struct for the `Host` variant instead of raw `BTreeMap<String, Value>`.

---

## Architecture Notes

The API surface splits cleanly into two tiers:

1. **High-level** (`lib.rs`): `analyze()`, `analyze_with_ai_report()`, `analyze_with_project()` — the three analysis entry points. These compose parser -> graph -> AI -> DB.
2. **Low-level** (`db.rs`, `types.rs`): Direct database access and graph type definitions. Used by `main.rs` for project management commands.

The split is good. The risk is that the low-level surface is too open — `Database` has 16 public methods, and all submodules are fully public. For a CLI tool this is acceptable; for a library with downstream consumers, it is too permissive.

---

## Action Items (Priority Order)

| # | Priority | Action | Files |
|---|----------|--------|-------|
| 1 | High | Add doc comments to 6 core types in `types.rs` | `src/types.rs` |
| 2 | High | Add doc comments to `analyze_with_ai_report`, `analyze_with_project`, `load_config` | `src/lib.rs` |
| 3 | High | Make `ai::business::Session` etc. `pub(crate)` | `src/ai/business.rs`, `src/ai/mod.rs` |
| 4 | Medium | Replace config tuple with public `Config` struct | `src/lib.rs` |
| 5 | Medium | Validate `api_key`/`har_path` at entry-point boundaries | `src/lib.rs` |
| 6 | Medium | Add `pub(crate)` to `graph::is_static_resource`, `normalize_path_template` | `src/graph.rs` |
| 7 | Low | Standardize error variant field names | `src/error.rs` |
| 8 | Low | Add `HostProperties` struct to replace raw `BTreeMap` | `src/types.rs` |

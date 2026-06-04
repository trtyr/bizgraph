# Conventions

Code style rules, naming conventions, and patterns enforced in bizgraph. Only deviations from standard Rust practice are listed below.

## Determinism

This is the highest-priority project convention. Every output must be reproducible from the same input.

- **ID generation**: all node/edge IDs derived from stable keys via `Uuid::new_v5(&STABLE_ID_NAMESPACE, key.as_bytes())` (`src/types.rs:173`). Never random UUIDs for persistent entities.
- **Sorted collections**: use `BTreeMap`/`BTreeSet` for all serialized maps (`src/types.rs:1,35`; `src/graph.rs:1`). `HashMap`/`HashSet` only for internal lookup sets that are never serialized (e.g., `existing_keys` in `src/lib.rs:80`).
- **Sorted node/edge arrays**: graph construction sorts all output collections before returning.
- **Stable key format** (see `src/graph.rs`):
  - Host: `host:<normalized-host>`
  - Business function: `bf:<host>:<path-prefix>`
  - Endpoint: `ep:<method>:<host>:<path-template>`
- **Edge labels**: `contains` (host→bf), `calls_after` (sequential flow), `data_dependency:*` (shared data).

## Error Handling

Single `Error` enum in `src/error.rs` with typed variants. No `Result<_, String>` anywhere.

- **Pattern**: two tiers per fallible source — bare variant (auto-conversion via `From`) and `*Context` variant (manual `.map_err` with a human-readable context string).
  - Example: `Error::Io(std::io::Error)` vs `Error::IoContext { context, source }` (`src/error.rs:5,12`).
- **Convenience constructors**: `Error::io(context, source)`, `Error::sqlite(context, source)`, `Error::json(context, source)`, `Error::toml(context, source)`, `Error::validation(message)` (`src/error.rs:93-126`).
- **Display impl** (`src/error.rs:129`): each variant formats with `{context}: {source}` or a domain-specific message. API response errors truncate body to 200 chars.
- **Source impl** (`src/error.rs:191`): delegates to inner error for bare variants, returns `None` for domain errors.
- **Custom `Result<T>`** alias: `pub type Result<T> = std::result::Result<T, Error>` (`src/error.rs:91`).

## Serde

- **All enums/structs** with serde derive use `#[serde(rename_all = "snake_case")]` (`src/types.rs:11,19,41,49,105`).
- **Tagged enum**: `BusinessNodeProperties` uses `#[serde(tag = "kind", content = "details")]` (`src/types.rs:105`) — externally-tagged format.
- **Skip empties**: `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]` for map fields, `skip_serializing_if = "Option::is_none"` for optional fields, `skip_serializing_if = "Vec::is_empty"` for optional arrays (`src/types.rs:34,36,77,99,165`).
- **HAR deserialization types** are private to `parser.rs` and use `#[serde(rename = "camelCase")]` for HAR-spec fields (`src/parser.rs:45-48`).

## File Organization

| File | Responsibility |
|------|---------------|
| `src/types.rs` | All shared data types. Zero logic, zero dependencies on other project modules. |
| `src/error.rs` | `Error` enum, `Result` alias, `From` impls. Zero dependencies on other project modules. |
| `src/parser.rs` | HAR deserialization (private types) → public `TrafficRow`. Depends on `url`, `serde_json`, `error`. |
| `src/graph.rs` | Graph construction from `TrafficRow[]`. Depends on `types`, `parser`, `error`. |
| `src/db.rs` | SQLite persistence. Depends on `types`, `error`. |
| `src/ai/` | AI analysis module. `prompts.rs` (system prompts), `chat.rs` (HTTP client), `agent.rs` (orchestration), `summarization.rs` (graph serialization for prompts). |
| `src/lib.rs` | Public API surface + orchestrator. Wires parser → graph → db → ai. |
| `src/main.rs` | CLI only (clap derive). Delegates to `lib.rs`. |

- **No cross-module re-exports**: each module imports what it needs directly.
- **`lib.rs` exports**: `Database`, `Error`, `Result` are re-exported for library consumers.
- **Single crate**: `[lib]` + `[[bin]]` in one `Cargo.toml`.

## Tests

- **Location**: inline `#[cfg(test)] mod tests` at file bottom. No `tests/` directory.
- **Naming**: behavior-descriptive snake_case, not numbered. Pattern: `<function>_with_<condition>` or `<function>_<expected_behavior>`.
  - `is_all_digits_with_numeric_string` ✓
  - `endpoint_confidence_increases_with_observations` ✓
  - `parses_simple_har` ✓
  - `test_01_parse` ✗
- **Test section separators**: `// ── <function_name> ────` comments between test groups (`src/graph.rs:1034,1061,1088`).
- **Test helpers**: private functions in the test module (e.g., `minimal_traffic_row` in `src/graph.rs`).
- **Coverage**: `parser.rs` has 9 tests, `graph.rs` has 83 tests.

## Clippy & Formatting

No `clippy.toml` or `rustfmt.toml` — uses Rust defaults. One explicit lint suppression:

- `#[allow(clippy::large_enum_variant)]` on `BusinessNodeProperties` (`src/types.rs:104`).

## Anti-Patterns (Not Present)

ffgrep for `TODO|FIXME|HACK|XXX|DEPRECATED` in `src/` returned zero matches. The codebase is clean.

From `AGENTS.md`, these are explicitly forbidden:

- Random IDs for the same input file.
- `HashMap` where sort order matters.
- Stuffing parser-only helpers into `main.rs`.
- Changing stable key shapes without updating docs and downstream.
- Adding security/vulnerability analysis to AI prompts (business analysis only).

## Async

- Runtime: `tokio` with `features = ["full"]`.
- AI functions (`analyze_with_ai`, `analyze_with_ai_deep`, `identify_business_functions`) are `async`.
- Sync entry points (`analyze`, `parse_har`, `build_business_graph`) remain non-async.
- `main.rs` uses `#[tokio::main]`.

## Config

- TOML-only, no environment variables.
- Path: `~/.config/bizgraph/config.toml`.
- Graceful degradation: `try_load_config()` returns `Option` when config is missing (`src/lib.rs:224`).
- `load_config()` returns `Err(ConfigMissingApiKey)` if key absent (`src/lib.rs:202`).

## Module Dependency Rule

`types.rs` and `error.rs` are leaf modules — they depend on no other project modules. Every other module depends on them. This is the intended dependency graph; do not introduce circular dependencies.

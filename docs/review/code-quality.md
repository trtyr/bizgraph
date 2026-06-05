# Code Quality Review — bizgraph

**Date**: 2026-06-05
**Reviewer**: Automated analysis
**Scope**: `src/` (7,376 lines across 13 files)

---

## Summary

| Criterion | Score | Notes |
|-----------|-------|-------|
| Naming | A | Clear, consistent, domain-appropriate |
| File Organization | A | Clean modular structure, reasonable sizes |
| Code Duplication | C | Significant copy-paste in graph construction |
| Complexity | B | Generally well-decomposed, a few dense spots |
| Dead Code | B | Unused import types, minor cleanup opportunities |
| Anti-Patterns | A | Zero markers, disciplined error handling |

**Overall: B+** — Well-engineered codebase with strong conventions. The main area for improvement is code duplication in `graph.rs`.

---

## 1. Naming — Score: A

**Strengths:**
- All types use domain-appropriate names: `BusinessNode`, `EndpointAccumulator`, `BusinessFunctionProperties`
- Functions are action-oriented: `build_business_graph`, `parse_har`, `normalize_path_template`
- Stable key formats are self-documenting: `ep:GET:/api/users/{id}`, `bf:example.com:/api`
- Enum variants use consistent `snake_case` serde renames (`src/types.rs:11,19,41`)

**Evidence:**
- `src/types.rs`: All 20+ public types have clear, descriptive names
- `src/graph.rs:45-233`: `build_business_graph` and related functions clearly convey purpose
- `src/ai/agent.rs:17-62`: Struct fields are unambiguous (`phase`, `domains`, `observations`, `token_budget`)

**Minor nitpicks:**
- `src/graph.rs:156,225,426`: Comparison closures use `left`/`right` which is fine but `a`/`b` is more idiomatic in Rust
- `src/lib.rs:18-24`: Private `Config` struct could use more descriptive field names (though it's internal)

**Recommendation:** No changes needed. Naming is excellent throughout.

---

## 2. File Organization — Score: A

**Strengths:**
- Clean separation of concerns: parser → graph → db → ai, orchestrated by `lib.rs`
- Leaf modules (`types.rs`, `error.rs`) have zero project dependencies
- `ai/` module is properly split into focused sub-modules (prompts, chat, agent, summarization, business)
- CLI logic isolated in `main.rs`, never leaks into library code

**File Sizes:**
| File | Lines | Assessment |
|------|-------|------------|
| `src/graph.rs` | 1,728 | Large, but 920+ lines are tests (83 tests). Core logic ~800 lines |
| `src/db.rs` | 990 | Appropriate for full CRUD + schema migration |
| `src/main.rs` | 868 | CLI rendering + 9 subcommands — reasonable |
| `src/ai/summarization.rs` | 864 | Graph serialization for multiple prompt formats |
| `src/ai/agent.rs` | 659 | 4-phase agent with state management |
| `src/ai/business.rs` | 646 | Session grouping + AI identification |
| `src/parser.rs` | 604 | HAR parsing + 10 tests |
| `src/error.rs` | 267 | Typed errors with Display/Source impls |
| `src/lib.rs` | 259 | Clean orchestration layer |
| `src/types.rs` | 216 | Pure data structures |

**Evidence:**
- `src/lib.rs:1-6`: Module declarations match dependency graph (no circular imports)
- `src/db.rs:30-32`: Database wraps `Mutex<Connection>` — single responsibility
- `src/ai/mod.rs`: Re-exports public API without leaking internals

**Recommendation:** Structure is exemplary. No reorganization needed.

---

## 3. Code Duplication — Score: C

**Issue:** `build_business_graph` (lines 45-233) and `build_business_graph_from_ai` (lines 236-434) share ~60% identical code.

**Duplicated sections:**
1. **Endpoint accumulation loop** (`src/graph.rs:66-90` vs `src/graph.rs:250-266`):
   ```rust
   // Both functions contain this nearly identical loop:
   for row in rows {
       let normalized_path = normalize_path_template(&row.path);
       let endpoint_key = format!("ep:{}:{}:{}", row.method, row.host, normalized_path);
       endpoint_state.entry(endpoint_key.clone())
           .or_insert_with(|| EndpointAccumulator::new(&row.host, &row.method, &normalized_path))
           .observe(row, &normalized_path);
       sequence.push(endpoint_key.clone());
       // ... response candidate extraction
   }
   ```

2. **Node creation** (`src/graph.rs:92-125` vs `src/graph.rs:286-319`):
   Identical endpoint node construction logic.

3. **Edge construction** (`src/graph.rs:155-230` vs `src/graph.rs:354-431`):
   Comments explicitly state "same logic as build_business_graph" (lines 354, 385).
   - `calls_after` edges: identical windows(2) loop
   - `data_dependency` edges: identical cross-reference logic

**What differs:**
- Business function grouping: URL-prefix-based (line 75-81) vs AI-identified (line 271-283)
- BF node properties: URL-based has no description, AI-based has `description`

**Recommendation:** Extract shared logic into private helper functions:
```rust
// Suggested refactor in src/graph.rs:
fn accumulate_endpoints(rows: &[TrafficRow]) -> (BTreeMap<String, EndpointAccumulator>, Vec<String>, BTreeMap<String, Vec<(String, String)>>) { ... }

fn create_endpoint_nodes(endpoint_state: &BTreeMap<String, EndpointAccumulator>) -> (Vec<BusinessNode>, HashMap<String, Uuid>) { ... }

fn build_edges(business_functions: &BTreeMap<String, BusinessFunctionAccumulator>, 
               endpoint_state: &BTreeMap<String, EndpointAccumulator>,
               sequence: &[String],
               response_candidates: &BTreeMap<String, Vec<(String, String)>>,
               node_ids: &HashMap<String, Uuid>) -> Result<Vec<BusinessEdge>> { ... }
```

This would reduce `graph.rs` by ~200 lines and eliminate the duplication.

---

## 4. Complexity — Score: B

**Strengths:**
- Most functions are well-decomposed (30-60 lines)
- `run_agent` (`src/ai/agent.rs:115-218`) cleanly separates 3 phases with parallel domain analysis
- `analyze_with_project` (`src/lib.rs:62-181`) has clear step-by-step structure

**Dense areas:**

1. **`EndpointAccumulator::observe`** (`src/graph.rs:483-549`, 67 lines):
   - 12 distinct responsibilities (method tracking, status codes, schema inference, parameter extraction, normalization notes, samples)
   - Acceptable for an accumulator pattern, but could extract `update_normalization_notes()` and `collect_samples()`

2. **`build_graph_summary`** (`src/ai/summarization.rs:9-170`, 161 lines):
   - 10 local variables initialized at function start (lines 10-19)
   - Large match arm with nested if-else for "interesting" endpoint detection
   - Could benefit from extracting `classify_endpoint()` helper

3. **`analyze_with_project`** (`src/lib.rs:62-181`, 119 lines):
   - 3 nested if-else chains for AI config resolution (lines 107-123)
   - Incremental analysis logic interleaved with graph building
   - The function does too many things: parse, resolve project, detect incremental, build graph, merge to DB, run AI, record analysis

**Recommendation:**
- Extract `EndpointAccumulator::observe` helper methods
- Break `analyze_with_project` into `resolve_or_create_project`, `detect_incremental`, `build_and_merge_graph`, `run_ai_analysis`
- Consider a `GraphSummaryBuilder` struct for `build_graph_summary`

---

## 5. Dead Code — Score: B

**Unused types in `src/types.rs`:**
- `BusinessImportRequest` (line 137)
- `BusinessImportNode` (line 143)
- `BusinessImportEdge` (line 151)
- `BusinessImportResult` (line 160)
- `default_properties()` (line 169)

These are defined but never used in the codebase. The `docs/context/modules.md` notes they "exist for external import scenarios (not used internally)".

**Evidence:**
- `ffgrep` for `BusinessImport` in `src/` returns only the type definitions
- `codegraph impact BusinessImportRequest` would show zero downstream usage

**Other observations:**
- `src/ai/summarization.rs:836`: Single `eprintln!` in non-CLI code (should use `log` crate or be removed)
- No `#[allow(dead_code)]` annotations found — the compiler would catch true dead code

**Recommendation:**
- Add `#[cfg(feature = "import")]` gate to import types if they're planned for future use
- Otherwise, remove them to reduce type surface area
- Replace `eprintln!` in `summarization.rs` with a proper logging framework or remove

---

## 6. Anti-Patterns — Score: A

**Zero markers found:**
`ffgrep('TODO|FIXME|HACK|XXX|DEPRECATED')` in `src/` returned 0 matches.

**Production code `unwrap()` usage:**
Only 2 instances in non-test code:

1. `src/parser.rs:184`: `Some(parsed_url.query().unwrap().to_string())`
   - **Safe**: Guarded by `is_none_or(|q| q.is_empty())` check on line 181
   - **Better**: Use `parsed_url.query().map(|q| q.to_string())` to avoid unwrap entirely

2. `src/parser.rs:486`: Same pattern (duplicate in `extract_port`)
   - Same safety guarantee, same improvement opportunity

**Test code `unwrap()`/`expect()`/`panic!`:**
- 35 instances across `parser.rs` and `graph.rs` test modules
- All appropriate for test assertions

**Other patterns observed:**
- `reqwest::Client::new()` called per request (`src/ai/chat.rs:79`) — no connection pooling. Acceptable for low-volume CLI tool, but would be problematic at scale.
- `use super::*` glob imports in 4 files (`ai/business.rs`, `ai/agent.rs`, `parser.rs`, `graph.rs`). Acceptable for test modules, but `graph.rs:1005` uses it in a non-test context.
- Consistent `?` operator usage — no `.unwrap()` chains in production code

**Recommendation:**
- Fix the 2 `unwrap()` calls in `parser.rs` with `.map()` pattern
- Consider creating a shared `reqwest::Client` instance for connection reuse
- Replace `use super::*` in `graph.rs:1005` with explicit imports

---

## Additional Observations

### Error Handling (Exemplary)
- Custom `Error` enum with 25+ variants (`src/error.rs:4-89`)
- Two-tier pattern: bare variant (auto-conversion) + `*Context` variant (with message)
- Convenience constructors: `Error::io()`, `Error::sqlite()`, `Error::json()`, `Error::toml()`, `Error::validation()`
- Proper `Display` and `std::error::Error` implementations with `source()` chain
- API response errors truncate body to 200 chars for readability (line 156)

### Determinism (Exemplary)
- All IDs derived via `Uuid::new_v5(&STABLE_ID_NAMESPACE, key)` — never random
- `BTreeMap`/`BTreeSet` used throughout for deterministic serialization
- Node/edge arrays sorted before return
- Stable key formats are consistent and well-documented

### Test Coverage
- `graph.rs`: 83 tests covering determinism, stable keys, edge construction, confidence scoring
- `parser.rs`: 10 tests (7 parsing + 3 validation)
- Test naming follows behavior-descriptive pattern: `endpoint_confidence_increases_with_observations`
- Test section separators (`// ── function_name ────`) for organization

### Serde Discipline
- All enums use `#[serde(rename_all = "snake_case")]`
- `BusinessNodeProperties` uses tagged enum: `#[serde(tag = "kind", content = "details")]`
- Empty collections properly skipped: `skip_serializing_if = "BTreeMap::is_empty"`

---

## Priority Recommendations

| Priority | Action | Impact | Effort |
|----------|--------|--------|--------|
| P1 | Extract duplicated graph construction logic in `src/graph.rs` | Reduces ~200 lines, eliminates maintenance risk | 2-3 hours |
| P2 | Fix 2 `unwrap()` calls in `src/parser.rs` | Eliminates theoretical panic risk | 15 minutes |
| P3 | Break up `analyze_with_project` in `src/lib.rs` | Improves readability and testability | 1 hour |
| P4 | Remove or gate unused `BusinessImport*` types | Reduces API surface | 30 minutes |
| P5 | Replace `use super::*` with explicit imports | Better IDE support, clearer dependencies | 15 minutes |

---

## Conclusion

This is a well-engineered codebase with strong conventions and disciplined practices. The determinism-first design, comprehensive error handling, and clean module boundaries are exemplary. The primary technical debt is the duplicated graph construction logic in `graph.rs`, which should be refactored before adding more graph variants. Overall code quality is high with clear, maintainable code throughout.

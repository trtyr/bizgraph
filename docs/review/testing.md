# Testing Quality Review

> **Date**: 2026-06-05
> **Total tests**: 108 (all passing)
> **Test suites**: 3 inline `#[cfg(test)]` modules
> **Test framework**: Rust built-in `#[test]` + `cargo test`
## Summary Table



| # | Criterion | Score | Evidence | Recommendation |
|---|-----------|-------|----------|----------------|
| 1 | Coverage | **C** | 3/10 source files have tests (30%) | Add tests for db.rs, error.rs, lib.rs, and ai/ submodules |
| 2 | Test Quality | **A** | Tests assert behavior, not just smoke; deterministic builds verified | Maintain current standard when expanding |
| 3 | Test Organization | **B** | Inline #[cfg(test)] with section separators; no integration/e2e tests | Add tests/ directory for integration tests |
| 4 | Edge Case Testing | **A** | Empty inputs, malformed data, boundary values all covered | Extend edge case coverage to DB and AI modules |
| 5 | Test Maintainability | **A** | Behavior-descriptive names, shared helpers, clear section separators | No action needed |
| 6 | CI Integration | **F** | No CI config of any kind | Add GitHub Actions with cargo test, cargo clippy, cargo fmt |


**Overall: B-** Strong test quality where tests exist, but 70 percent of modules have zero coverage and no CI.
---

## 1. Coverage

### What Is Tested (3 modules, 108 tests)

| Module | Tests | Lines | What Is Covered |
|--------|-------|-------|-----------------|
| src/graph.rs | 83 | 1003-1729 | Utility functions (is_all_digits, is_uuid_like, is_hash_like, split_extension, normalize_path_segment, normalize_path_template, business_path_prefix, path_parameter_kind, infer_value_kind, choose_parameter_kind, extract_response_candidates, parameter_kind_from_json) + full graph construction (empty input, single row, multi-endpoint, multi-host, calls_after edges, data_dependency edges, deterministic output, stable key formats, sorted output, status codes, confidence scaling) |
| src/parser.rs | 9 | 286-604 | HAR parsing, host filtering, empty URL skipping, missing optional fields, root path normalization, HTTP default port, empty file rejection, non-JSON rejection, missing log field rejection |
| src/ai/business.rs | 16 | 327-647 | JSON response cleaning, endpoint list deduplication, static resource filtering, session grouping (single/multiple/empty/no-timestamps), session summary deduplication, endpoint sample collection (richest/empty/static), sample summary |


### What Is NOT Tested (7 modules)

| Module | Size | Risk | Why It Matters |
|--------|------|------|----------------|
| src/db.rs | 36.5K | **HIGH** | SQLite persistence, upsert logic, graph merge, project CRUD - data corruption bugs would be silent |
| src/types.rs | 5.6K | MEDIUM | Complex serde with tagged enums, skip_serializing_if - serialization regressions break downstream |
| src/error.rs | 8.5K | LOW | Typed error enum with From impls - mostly structural, but Display formatting could regress |
| src/lib.rs | 8.2K | **HIGH** | Public API orchestrator wiring parser -> graph -> db -> ai - integration bugs hide here |
| src/ai/agent.rs | 22.3K | **HIGH** | Multi-phase agent loop, state management - complex async logic with no tests |
| src/ai/chat.rs | 3.5K | MEDIUM | HTTP client for AI API - untested error paths, timeout handling |
| src/ai/summarization.rs | 28.5K | MEDIUM | Graph serialization for prompts - complex formatting, truncation logic |
| src/main.rs | 35.9K | LOW | CLI (clap derive) - minimal custom logic, acceptable to skip |
| src/ai/prompts.rs | 4.8K | LOW | Static prompt strings - low risk, but token limit calculations should be verified |


### Recommendation

Priority order for adding tests:
1. **db.rs** - data integrity is critical; add in-memory SQLite tests
2. **lib.rs** - test the orchestration pipeline end-to-end with fixture HAR files
3. **ai/agent.rs** - test state transitions and phase orchestration
4. **types.rs** - add serde round-trip tests (serialize -> deserialize -> assert equality)
5. **ai/summarization.rs** - test graph serialization and truncation
---

## 2. Test Quality

### Strengths

Tests in this project are genuinely meaningful, not just smoke tests:

- **Behavioral assertions**: tests verify specific outputs, not just does not panic. Example: data_dependency_edge_created_when_response_value_appears_in_later_request (graph.rs:1607) checks the exact edge label data_dependency:token.
- **Determinism verification**: build_business_graph_is_deterministic (graph.rs:1541) runs the same input twice and compares all IDs, stable keys, and edges - critical for the project core promise.
- **Negative testing**: rejects_empty_file, rejects_non_json_file, rejects_json_without_log_field (parser.rs:566-603) verify error paths with specific error message assertions.
- **State progression**: endpoint_confidence_increases_with_observations (graph.rs:1698) verifies that confidence scores scale with data volume.


### Minor Weaknesses

- No #[should_panic] or Result-returning tests for expected-failure paths beyond parser validation.
- business.rs tests construct TrafficRow structs inline with all 20+ fields - repetitive but unavoidable without a shared test fixture crate.


**Score: A**

---

## 3. Test Organization

### Current Pattern

All tests follow the same inline pattern:

Section separators (// -- function_name --) group tests by function under test. This is excellent for navigation in a 1700+ line test module.


### What Is Missing

- **No tests/ directory**: zero integration tests. The pipeline parse_har -> build_business_graph -> db insert is never tested end-to-end.
- **No test fixtures**: parser.rs builds HAR JSON inline; a fixtures/ directory with real HAR samples would improve real-world coverage.
- **No benchmarks**: Cargo.toml has no [[bench]] section. Performance-critical paths have no regression protection.


**Score: B**
---

## 4. Edge Case Testing

### Excellent Coverage Where It Exists

**graph.rs** is the gold standard:
- Empty input: empty_rows_produce_empty_graph (line 1476)
- Single item: single_row_produces_bf_and_endpoint_nodes (line 1483)
- Duplicate items: same_endpoint_twice_produces_no_calls_after (line 1525)
- Multi-host: multiple_hosts_create_separate_bf_nodes (line 1591)
- Boundary values: is_all_digits_with_empty_string, is_hash_like_rejects_short_value, is_hash_like_rejects_7_hex_chars (boundary at 8 chars)
- Mixed formats: is_hash_like_with_16_alphanumeric_mixed, split_extension_with_multiple_dots


**parser.rs** covers:
- Empty files, non-JSON, missing fields
- Missing optional headers, IP, latency, response_length
- Root path normalization, HTTP vs HTTPS ports


**business.rs** covers:
- Empty sessions, no-timestamp sessions
- Static resource filtering (extensions, CDN paths)
- Deduplication in endpoint lists and session summaries


### Gaps

- No test for extremely large input (1000+ traffic rows)
- No test for malformed URL schemes (ftp://, ws://)
- No test for unicode in paths or hosts
- No test for concurrent access patterns (db.rs)


**Score: A**
---
## 5. Test Maintainability

| Aspect | Evidence |
|--------|----------|
| Naming | Behavior-descriptive names that read like a spec |
| Helpers | minimal_traffic_row() in graph.rs, make_row() in business.rs |
| Organization | Section separators group tests by function |
| Isolation | Tests use temp dirs, clean up after themselves |
| No flaky patterns | No sleeps, no network calls, no side effects |

**Score: A**
---
## 6. CI Integration

**No CI exists.** From docs/context/deploy.md line 113:

> No CI pipeline exists. No .github/workflows/, .gitlab-ci.yml, or other CI config found in the repo. Builds, tests, and installs are manual.

The release checklist (deploy.md:117-125) is manual:



No pre-commit hooks. No branch protection. No automated quality gates.

### Recommendation

Add .github/workflows/ci.yml with: checkout, cargo fmt --check, cargo clippy -- -D warnings, cargo test

This is the single highest-impact improvement - it enforces the existing manual checklist automatically.

**Score: F**

---

## Test Inventory

| File | Has tests | Count | Lines | Status |
|------|:-:|:-:|:-:|--------|
| src/graph.rs | yes | 83 | 1003-1729 | Excellent |
| src/ai/business.rs | yes | 16 | 327-647 | Good |
| src/parser.rs | yes | 9 | 286-604 | Good |
| src/db.rs | no | 0 | - | **Untested** |
| src/types.rs | no | 0 | - | **Untested** |
| src/error.rs | no | 0 | - | **Untested** |
| src/lib.rs | no | 0 | - | **Untested** |
| src/main.rs | no | 0 | - | Acceptable |
| src/ai/agent.rs | no | 0 | - | **Untested** |
| src/ai/chat.rs | no | 0 | - | **Untested** |
| src/ai/prompts.rs | no | 0 | - | Acceptable |
| src/ai/summarization.rs | no | 0 | - | **Untested** |
| **Total** | **3** | **108** | - | - |

---

## Action Items (Priority Order)

1. **Add GitHub Actions CI** - enforce cargo test + cargo clippy + cargo fmt --check on every push
2. **Add db.rs tests** - in-memory SQLite, upsert correctness, graph merge, project CRUD
3. **Add lib.rs integration tests** - end-to-end: fixture HAR -> parse -> graph -> DB -> verify
4. **Add tests/ directory** - integration test harness with [[test]] in Cargo.toml
5. **Add serde round-trip tests for types.rs** - serialize -> deserialize -> assert equality
6. **Add ai/agent.rs tests** - mock chat client, test phase transitions and error handling
7. **Add shared test fixtures** - tests/fixtures/ with real HAR samples, shared TrafficRow builder

---

*Generated by testing quality review on 2026-06-05.*

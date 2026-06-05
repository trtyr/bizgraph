# Error Handling Review

Reviewed: `src/error.rs`, `src/lib.rs`, `src/parser.rs`, `src/ai/chat.rs`, `src/ai/agent.rs`, `src/ai/business.rs`, `src/db.rs`, `src/main.rs`

## Summary

Bizgraph has exceptionally well-structured error handling for a CLI Rust project.
A single `Error` enum with 25+ typed variants, dual-tier (bare + Context) patterns,
convenience constructors, and proper `Display`/`Source` impls form a solid foundation.
The AI layer demonstrates thoughtful resilience — retries, fallbacks, and budget limits.
Only minor gaps remain.

---

## 1. Error Propagation — Grade: A

| Aspect | Evidence | Notes |
|--------|----------|-------|
| `Result<T>` on all public APIs | `lib.rs:30`, `lib.rs:35`, `lib.rs:62`, `db.rs:96`, `parser.rs:117` | Every fallible function returns `Result` |
| `?` propagation | `lib.rs:31`, `lib.rs:73-76`, `db.rs:118-127`, `parser.rs:118-147` | No manual match-and-unwraps in production paths |
| AI fallback silently swallows errors | `lib.rs:51`, `lib.rs:119` — `Err(_) =>` discards AI identification errors | Intentional graceful degradation, but the discarded error loses diagnostic info |
| Domain failure tolerance | `agent.rs:162-195` — logs failures to stderr, continues until threshold `MAX_DOMAIN_FAILURES` | Good resilience pattern; stderr logging acceptable for CLI |

**Recommendation:** The `Err(_)` at `lib.rs:51,119` could log a one-line warning before falling back:
```rust
Err(e) => {
    eprintln!("  AI identification unavailable ({e}), using deterministic graph");
    (build_business_graph(&rows)?, None)
}
```
This costs nothing and preserves diagnostic value when debugging user reports.

---

## 2. Error Types — Grade: A

| Aspect | Evidence | Notes |
|--------|----------|-------|
| Single typed enum, no `Result<_, String>` | `error.rs:4-89` — 25+ variants | Conventions doc confirms zero `Result<_, String>` in codebase |
| Dual-tier (bare + Context) | `error.rs:5` (Io) vs `error.rs:12` (IoContext); same for Sqlite, Json, Toml | `From` impls auto-convert bare; Context variants carry operation descriptions |
| Convenience constructors | `error.rs:93-126` — `io()`, `sqlite()`, `json()`, `toml()`, `validation()` | Reduces boilerplate at call sites |
| Domain-specific variants | `error.rs:57` BudgetExceeded, `error.rs:62` TaskPanicked, `error.rs:48` ApiResponse | Business logic errors are first-class, not shoehorned into generic types |
| `Display` with actionable messages | `error.rs:129-188` — every variant has a clear, human-readable format | `ApiResponse` truncates body to 200 chars; `BudgetExceeded` suggests fixes |
| Correct `source()` delegation | `error.rs:191-224` — bare variants delegate, domain variants return `None` | Enables error chain inspection via `std::error::Error` |

**Recommendation:** Consider adding `#[non_exhaustive]` to the enum if external crates might pattern-match on it in the future. Low priority for a CLI tool.

---

## 3. Edge Cases — Grade: B+

| Aspect | Evidence | Notes |
|--------|----------|-------|
| HAR format validation | `parser.rs:127-140` — three progressive checks (empty, not JSON, missing `log`) | Clear, actionable error messages including "exported from browser DevTools" hint |
| Empty URL/method filtering | `parser.rs:155-157` — `continue` on empty | Silent skip; acceptable for malformed entries |
| Empty project name | `db.rs:98-100` — returns `Error::EmptyProjectName` | Good |
| Config value normalization | `lib.rs:252-258` — trims whitespace, returns `None` for empty | Graceful handling of `"  "` in config |
| Missing `query()` after successful `Url::parse` | `parser.rs:184` — `parsed_url.query().unwrap()` | The only bare `unwrap()` in production code. Safe in practice (can't fail after parse succeeds on a valid URL), but breaks the project's own no-unwrap convention |
| Silent URL parse failures | `parser.rs:161` — `Err(_) => continue` | Valid entries with malformed URLs silently dropped |

**Recommendation:** Replace the `unwrap()` at `parser.rs:184` with `unwrap_or("")` or a match to align with the codebase convention. Add a debug-level log or counter for silently skipped entries (URL parse failures, empty entries) so users can diagnose data loss.

---

## 4. Panic Safety — Grade: A

| Aspect | Evidence | Notes |
|--------|----------|-------|
| No `unwrap()` in production paths | `parser.rs:164,189-216,228-234` — all use `unwrap_or("")` or `unwrap_or_default()` | Consistent safe-fallback pattern |
| `unwrap_or_default()` on AI responses | `chat.rs:112` — empty string if no choices; `agent.rs:657` — empty string on serialize failure | Won't panic; worst case returns empty data |
| `panic!` only in test code | `graph.rs:1693,1715,1722` — all inside `#[cfg(test)] mod tests` | Acceptable — test panics are the expected behavior |
| `process::exit(1)` at CLI boundary | `main.rs:114,123,413` | Correct for CLI — errors are printed before exit |
| `TaskPanicked` variant for tokio join errors | `error.rs:62-65`, `agent.rs:186-193` | JoinHandle panics are caught and reported, not propagated |

**Recommendation:** None. Production code is panic-free. The single `unwrap()` at `parser.rs:184` is technically safe but should be replaced for consistency (see criterion 3).

---

## 5. User-Facing Errors — Grade: A

| Aspect | Evidence | Notes |
|--------|----------|-------|
| Config missing → tells user where to put it | `error.rs:169-172` — `"Configure api_key in ~/.config/bizgraph/config.toml"` | Exact file path included |
| Budget exceeded → suggests fix | `error.rs:161-166` — `"Try analyzing a HAR with fewer endpoints, or increase MAX_DEEP_AI_CALLS"` | Actionable |
| Ambiguous project → lists candidates | `error.rs:146-151` — `"project reference 'X' is ambiguous: a, b, c"` | User can immediately disambiguate |
| Invalid HAR → explains how to fix | `parser.rs:137-139` — `"Make sure you exported from browser DevTools → Network → Export HAR"` | Beginner-friendly |
| API error → truncates response body | `error.rs:155-159` — body truncated to 200 chars with `...` suffix | Prevents terminal flooding from massive HTML error pages |
| All errors prefixed with `Error:` | `main.rs:113,122,412` — `eprintln!("Error: {error}")` | Consistent CLI convention |

**Recommendation:** For the `ApiResponse` error, include the HTTP status code category in the hint (e.g., "401 → check your API key", "429 → rate limited, try again later"). Currently it just shows the raw status number.

---

## 6. Recovery — Grade: A-

| Aspect | Evidence | Notes |
|--------|----------|-------|
| HTTP retry with exponential backoff | `chat.rs:72-126` — 3 retries, 1s/2s/4s delays, retries on 429 and 5xx | Well-structured retry loop |
| AI identification → deterministic fallback | `lib.rs:51`, `lib.rs:119` — `Err(_) => (build_business_graph(&rows)?, None)` | Graceful degradation — system works without AI |
| Domain failure threshold | `agent.rs:162-194` — stops domain analysis after `MAX_DOMAIN_FAILURES` | Prevents cascading failures |
| Incremental analysis reuse | `lib.rs:139-143` — reuses latest AI report when no new endpoints | No wasted API calls |
| Config absent → returns `None` | `lib.rs:224-226` — `try_load_config()` → `load_config().ok()` | CLI works without config for non-AI commands |
| Token budget enforcement | `agent.rs:643-654` — `check_budget()` returns `Error::BudgetExceeded` | Prevents runaway API costs |

**Recommendation:** Add a per-request timeout to the HTTP client (`chat.rs:79`). Currently `reqwest::Client::new()` uses no explicit timeout — a hung API server will block the entire pipeline indefinitely. Example:
```rust
let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(120))
    .build()?;
```

---

## Overall Assessment

| Criterion | Grade |
|-----------|-------|
| Error Propagation | **A** |
| Error Types | **A** |
| Edge Cases | **B+** |
| Panic Safety | **A** |
| User-Facing Errors | **A** |
| Recovery | **A-** |
| **Overall** | **A** |

## Top 3 Improvements

1. **Log swallowed AI errors** (`lib.rs:51,119`) — one `eprintln!` line preserves diagnostic value at zero cost.
2. **Add HTTP timeout** (`chat.rs:79`) — 120s timeout prevents indefinite hangs on unresponsive APIs.
3. **Replace last `unwrap()`** (`parser.rs:184`) — use `unwrap_or("")` to match codebase convention and close the only gap in the no-unwrap rule.

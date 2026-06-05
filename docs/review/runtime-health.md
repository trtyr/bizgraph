# Runtime Health Review

**Project**: bizgraph v0.1.1
**Date**: 2026-06-05
**Scope**: Runtime resilience, robustness, and operational health

## Summary

| Criterion | Score | Verdict |
|-----------|-------|---------|
| Timeout Handling | **D** | No HTTP timeouts configured — requests can hang indefinitely |
| Retry Logic | **B** | Good exponential backoff on HTTP, but limited scope |
| Logging | **C** | Adequate progress visibility, no structured logging |
| Concurrency Safety | **B-** | Correct for CLI scope, but Mutex blocks in async context |
| Configuration Validation | **B+** | Graceful degradation, clear errors, minor gaps |
| Resource Cleanup | **C** | New HTTP client per request, no connection pooling |
| Graceful Shutdown | **F** | No signal handling whatsoever |

**Overall**: **C+** — functional CLI tool with reasonable error handling, but missing production-grade resilience patterns.

---

## 1. Timeout Handling — D

**Score**: D (poor — no timeouts on I/O or network)

### Evidence

- `src/ai/chat.rs:79` — `reqwest::Client::new()` with no timeout configured. The default reqwest client has no timeout, meaning a hung API server will block indefinitely.
- `src/ai/chat.rs:80-86` — `.post(api_url).json(request).send().await` — no per-request timeout or deadline.
- `src/ai/agent.rs:155-159` — Parallel domain tasks spawned with `tokio::spawn` inherit the same unbounded timeout behavior.
- `src/db.rs:40-42` — `Connection::open(path)` — no busy timeout or connection timeout set. WAL mode helps, but a long-running write could block reads indefinitely.
- No overall operation deadline (e.g., "this entire analysis must complete within 30 minutes").

### Recommendation

1. **Set HTTP client timeout**: `reqwest::Client::builder().timeout(Duration::from_secs(120)).build()` — 120s is reasonable for LLM API calls.
2. **Set per-request timeout** on the client for shorter cycles if needed.
3. **Set SQLite busy timeout**: `PRAGMA busy_timeout = 5000;` in the connection setup.
4. **Add overall operation deadline** in `analyze_with_project` to prevent runaway analyses.

---

## 2. Retry Logic — B

**Score**: B (good — covers main failure modes, but scope is limited)

### Evidence

- `src/ai/chat.rs:69-133` — Core retry loop in `send_chat_request()`:
  - Max 3 retries with exponential backoff: 1s → 2s → 4s (line 74: `1000 * (1u64 << (attempt - 1))`)
  - Retries on HTTP 429 (rate limit) and 5xx (server errors) — line 116
  - Retries on connection errors (transport failures) — line 90-96
  - Returns last error after all retries exhausted — line 128-132
- `src/ai/agent.rs:181-191` — `MAX_DOMAIN_FAILURES = 2` (src/ai/prompts.rs:84): stops domain analysis after 2 consecutive domain task failures.
- `src/ai/agent.rs:186-192` — Catches tokio task panics gracefully, counting them as failures.

### Gaps

- **No retry on 408 (Request Timeout)** — should be treated as transient.
- **No jitter** in backoff — fixed exponential can cause thundering herd on shared rate limits.
- **DB operations not retried** — `src/db.rs` uses single-attempt upserts; SQLite BUSY errors (though rare in WAL mode) are not retried.
- **Domain tasks in agent.rs** — each domain task calls `chat_fresh` which has retry, but the domain task itself (the outer `tokio::spawn`) has no retry wrapper.

### Recommendation

1. Add jitter to backoff: `delay_ms + rand(0..500)`.
2. Include 408 and 409 as retryable status codes.
3. Consider wrapping SQLite writes in a retry-on-busy helper.

---

## 3. Logging — C

**Score**: C (adequate for CLI, but no structured logging)

### Evidence

- All logging uses `eprintln!` — no logging framework (`log`, `tracing`, `env_logger`). This means:
  - No log levels (debug/info/warn/error distinction)
  - No structured fields (machine-parseable output)
  - No configurable verbosity
- **Good**: Progress is visible at every major phase:
  - `src/ai/agent.rs:130` — Phase 1/3 overview
  - `src/ai/agent.rs:143-145` — Phase 2/3 domain deep-dive count
  - `src/ai/agent.rs:156` — Per-domain progress
  - `src/ai/agent.rs:198` — Phase 3/3 cross-domain
  - `src/ai/chat.rs:75` — Retry attempts logged
  - `src/lib.rs:96-100` — Incremental analysis stats
- **Good**: Error context is rich — `src/error.rs:155-158` truncates API response body to 200 chars.
- **Good**: No sensitive data (API keys) in log output — verified by searching for `api_key` in log statements.

### Gaps

- API response bodies logged on error (`src/ai/chat.rs:121`) could contain large payloads.
- No way to silence progress output for scripting/piping.
- No timestamps in log messages.

### Recommendation

1. Adopt `tracing` crate with `tracing-subscriber` — minimal migration effort.
2. Add `--quiet` flag to suppress progress output.
3. Consider truncating logged response bodies to a configurable limit.

---

## 4. Concurrency Safety — B-

**Score**: B- (correct for single-user CLI, but not production-safe for library use)

### Evidence

- `src/db.rs:31` — `conn: Mutex<Connection>` — single SQLite connection behind a std::sync::Mutex.
  - **Risk**: `Mutex::lock()` is a blocking call. When called from async context (tokio runtime), this blocks the entire executor thread. Documented as acceptable for CLI in architecture.md tradeoffs.
  - `src/ai/agent.rs:155-159` — Domain analysis tasks spawned with `tokio::spawn` are `Send` but do NOT access the Database. The DB is only accessed before/after the parallel phase. This is safe.
- **No race conditions** in graph construction — `build_business_graph` takes `&[TrafficRow]` (immutable borrow), returns owned `BusinessGraph`.
- **Agent state** is `mut` and owned by `run_agent` — no shared mutable state between tasks.
- `tokio::spawn` domain tasks capture owned `String` values (api_key, model, api_url) — no lifetime issues.

### Gaps

- If bizgraph is used as a library in a multi-threaded server, the `Mutex<Connection>` will be a bottleneck and potential deadlock source.
- `reqwest::Client::new()` is called per-request (line 79 of chat.rs) instead of being shared — this prevents connection pooling.

### Recommendation

1. For library use: consider `tokio::sync::Mutex` or connection pooling (e.g., `r2d2`).
2. Document the single-writer constraint prominently in library API docs.
3. Share a `reqwest::Client` instance across requests.

---

## 5. Configuration Validation — B+

**Score**: B+ (good — fails fast with clear messages)

### Evidence

- **Graceful degradation**: `src/lib.rs:224` — `try_load_config()` returns `Option`, allowing AI features to be optional. CLI uses this to run without API key.
- **Fail-fast on required values**: `src/lib.rs:202` — `load_config()` returns `Err(ConfigMissingApiKey)` with a clear message: "API key not found. Configure api_key in ~/.config/bizgraph/config.toml".
- **TOML parse errors**: `src/error.rs:84-88` — `ConfigRead` and `ConfigParse` variants carry the file path and source error.
- **Empty value handling**: `src/lib.rs:252-258` — `normalize_config_value()` trims whitespace and rejects empty strings.
- **Defaults**: `src/lib.rs:208,214` — model defaults to "deepseek-v4-flash", api_url defaults to "https://api.deepseek.com/chat/completions".

### Gaps

- No validation of `api_url` format (e.g., must be HTTPS, must be a valid URL).
- No validation that `api_key` is non-whitespace-only (trim + empty check exists, but a key of "   " would be caught).
- `config_path_in_home()` falls back to `.config/bizgraph/config.toml` (relative) if HOME is unset — `src/lib.rs:232` — which is surprising behavior.

### Recommendation

1. Validate `api_url` as a valid URL with `url::Url::parse()`.
2. Log a warning when falling back to relative config path (HOME unset).

---

## 6. Resource Cleanup — C

**Score**: C (functional but wasteful)

### Evidence

- **SQLite**: `src/db.rs:30-32` — `Database` wraps `Mutex<Connection>`. Connection is closed when `Database` is dropped (Rust RAII). WAL mode (`src/db.rs:44`) ensures crash-safe recovery.
- **HTTP Client**: `src/ai/chat.rs:79` — `reqwest::Client::new()` is created **on every API call**, inside the retry loop. This means:
  - No connection pooling across calls
  - New TLS handshake per request (for HTTPS endpoints)
  - TCP connections are not reused
- **File handles**: `src/lib.rs:240` — `fs::read_to_string(&path)` opens and closes the file in one call — RAII handles cleanup.
- **Tokio tasks**: `src/ai/agent.rs:155` — spawned tasks are `.await`ed immediately after, so they complete before the function returns. No leaked tasks.
- **No Drop impl** on any struct — all cleanup relies on Rust's default drop behavior, which is fine for the current types.

### Recommendation

1. **Create `reqwest::Client` once** and pass it through the call chain (or use a shared lazy_static/once_cell). This is the single biggest resource improvement.
2. Consider connection pool for SQLite if the library is used in a long-running context.

---

## 7. Graceful Shutdown — F

**Score**: F (no signal handling)

### Evidence

- `src/main.rs:76` — `#[tokio::main] async fn main()` — no signal handler registered.
- No `tokio::signal::ctrl_c()` handler.
- No `SIGTERM` / `SIGINT` handler.
- No cleanup logic for in-progress operations.
- **Mitigating factor**: This is a short-lived CLI tool, not a long-running server. The OS will clean up resources on kill. SQLite's WAL mode means the database won't be corrupted by an abrupt exit.
- **However**: An interrupted AI analysis will leave the project in a partially-analyzed state with no indication that the analysis was incomplete.

### Recommendation

1. Add a `ctrl_c` handler in `main.rs` that prints "Interrupted — partial analysis not saved" and exits cleanly.
2. For the library: document that callers should handle cancellation externally.
3. Consider writing a "analysis started" marker to the DB that gets cleared on completion — interrupted analyses can then be detected.

---

## Positive Patterns

These are well-done and worth preserving:

1. **Budget enforcement**: `src/ai/agent.rs:209-214` — Hard cap on AI calls (`MAX_DEEP_AI_CALLS = 7`) with `BudgetExceeded` error. Prevents runaway costs.
2. **Token budget tracking**: `src/ai/agent.rs:643-654` — `check_budget()` prevents context window overflow.
3. **Context truncation**: `src/ai/agent.rs:240-246` — Graceful truncation with notification when payload exceeds limit.
4. **Domain failure circuit breaker**: `src/ai/agent.rs:181-184` — Stops after `MAX_DOMAIN_FAILURES = 2` consecutive failures.
5. **Deterministic IDs**: All graph entities use UUIDv5 — no randomness means no collision risk.
6. **Error type hierarchy**: `src/error.rs` — Comprehensive typed errors with context strings. No `unwrap()` or `expect()` in production paths (verified — zero matches in src/).
7. **Incremental analysis**: `src/lib.rs:80-103` — Only new endpoints are sent to AI, reducing cost and latency.

## Priority Fixes

| Priority | Issue | Effort | Impact |
|----------|-------|--------|--------|
| **P0** | Add HTTP client timeout (120s) | 5 min | Prevents indefinite hangs |
| **P1** | Reuse `reqwest::Client` across requests | 30 min | 10x fewer TLS handshakes |
| **P1** | Add SQLite busy_timeout pragma | 5 min | Prevents DB contention |
| **P2** | Add jitter to retry backoff | 10 min | Better behavior under shared rate limits |
| **P2** | Add ctrl_c handler in main.rs | 15 min | Clean exit on interrupt |
| **P3** | Adopt `tracing` for structured logging | 2 hours | Observability foundation |
| **P3** | Validate api_url as URL | 10 min | Better error messages |

# Network Resilience Review — Bizgraph

> **Scope**: HTTP client behavior for AI API calls (reqwest → DeepSeek-compatible endpoint).
> **Reviewed**: 2026-06-05
> **Files analyzed**: `src/ai/chat.rs`, `src/ai/agent.rs`, `src/ai/mod.rs`, `src/lib.rs`, `src/error.rs`

---

## Summary

Bizgraph's entire network surface is a single HTTP function: `send_chat_request()` in `src/ai/chat.rs:64`. It makes non-streaming POST requests to an OpenAI-format chat completions endpoint. The AI agent (`run_agent()`) calls this function up to `MAX_DEEP_AI_CALLS` times across 4 phases — some in parallel via `tokio::spawn`.

The CLI's network usage is inherently transient (fire-and-forget POSTs), so some resilience criteria (persistent connections, heartbeats) are less critical than for a long-lived service. But the current implementation has notable gaps even for a CLI tool.

---

## Criterion 1: Reconnection / Retry Mechanism

| | |
|---|---|
| **Score** | **B** |
| **Evidence** | `src/ai/chat.rs:69-133` — `send_chat_request()` implements retry with exponential backoff |
| **Details** | Max retries: 3. Backoff: 1s → 2s → 4s (line 74: `1000 * (1u64 << (attempt - 1))`). Retries on: network errors (`reqwest::Error` at line 88-96), HTTP 429 rate-limit, HTTP 5xx server errors (line 116). Final error after exhaustion: returns `last_err` or synthetic `ApiResponse` (line 128-132). |
| **Recommendation** | **Good foundation.** The backoff formula is correct. Improvements: (1) Add jitter to prevent thundering herd if multiple parallel domain calls all hit rate-limit simultaneously — `agent.rs:148-160` spawns N parallel tasks that could all retry at the same wall-clock time. (2) Respect `Retry-After` header on 429 responses before falling back to exponential delay. (3) Consider increasing max retries to 5 for production use — 3 attempts with max 4s delay (total ~7s) may be too aggressive for a flaky network. |

## Criterion 2: Timeout Handling

| | |
|---|---|
| **Score** | **D** |
| **Evidence** | `src/ai/chat.rs:79` — `reqwest::Client::new()` uses default Client with zero timeout configuration |
| **Details** | `reqwest::Client::new()` creates a client with: **no connect timeout** (can hang on DNS/TLS handshake indefinitely), **no read timeout** (can hang waiting for API response), **no overall request timeout**. The AI API can return responses in 10-60+ seconds for complex analyses. If the server accepts the connection but never sends a response body, the client will wait forever. No `tokio::time::timeout` wrapper exists anywhere in the codebase. |
| **Recommendation** | **Critical.** Replace `Client::new()` with `Client::builder()` and set: (1) `.connect_timeout(Duration::from_secs(10))` for DNS+TLS, (2) `.timeout(Duration::from_secs(120))` for the overall request (generous for long AI completions), (3) `.read_timeout(Duration::from_secs(60))` for stalled connections. Alternatively, wrap the `.send().await` in `tokio::time::timeout()`. Without this, a hung API endpoint will freeze the entire CLI with no way to recover except Ctrl+C. |

## Criterion 3: Proxy Resilience

| | |
|---|---|
| **Score** | **F** |
| **Evidence** | Zero results for `proxy`, `PROXY`, `socks`, `SOCKS`, `http_proxy`, `https_proxy`, `all_proxy` across all of `src/`. |
| **Details** | No proxy configuration at all. The application relies entirely on `reqwest`'s default behavior (which reads system environment variables `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` via `reqwest::Client::new()`). There is no `.proxy()` builder call, no custom proxy handling, and no documentation of proxy support. |
| **Recommendation** | **Low priority for CLI.** reqwest's default env-var proxy detection is usually sufficient for personal CLI tools. If corporate proxy support is needed: (1) Add explicit `Client::builder().proxy(...)` with proxy URL from config, (2) document proxy configuration in README, (3) consider `.no_proxy()` for direct API access through corporate firewalls. |

## Criterion 4: Persistent Connection Health

| | |
|---|---|
| **Score** | **N/A** |
| **Evidence** | No heartbeat, ping-pong, or keepalive logic found. No long-lived connections exist. |
| **Details** | This is a CLI tool. All network calls are short-lived HTTP POSTs — the connection is established, the request is sent, the response is read, and the connection is released. There are no WebSocket, SSE, or long-polling connections. This criterion does not apply. |

## Criterion 5: Network Error Classification

| | |
|---|---|
| **Score** | **C** |
| **Evidence** | `src/ai/chat.rs:88-126` — error handling in retry loop; `src/error.rs:44-56` — `ApiRequest`, `ApiResponse`, `ApiResponseDecode` error variants |
| **Details** | **Partial classification exists.** HTTP-level: 429 and 5xx are treated as retryable (line 116); all other HTTP status codes (4xx except 429) are treated as permanent — returned immediately without retry (line 125). Network-level: `reqwest::Error` (covers DNS failure, connection refused, TLS errors, timeouts) is always retried (lines 88-96) — this is **too broad**, as permanent failures like invalid URL or TLS certificate errors will also be retried 3 times with exponential backoff. The error types (`ApiRequest`, `ApiResponse`, `ApiResponseDecode`) provide good structure but no transient/permanent classification flag. |
| **Recommendation** | Add transient/permanent classification on `reqwest::Error`: (1) `is_timeout()`, `is_connect()`, `is_request()` → retry (transient), (2) `is_builder()` → don't retry (permanent config error). This avoids wasting 7+ seconds retrying a malformed URL. Consider adding a helper `fn is_transient(err: &reqwest::Error) -> bool`. |

## Criterion 6: Graceful Degradation

| | |
|---|---|
| **Score** | **B** |
| **Evidence** | `src/lib.rs:46-52` — AI identification failure falls back to deterministic graph; `src/lib.rs:107-123` — no API key skips AI entirely; `src/ai/agent.rs:162-194` — domain failure budget prevents cascade failure |
| **Details** | **Two-tier degradation works well.** (1) If `identify_business_functions()` fails (network error or bad response), the pipeline falls back to deterministic `build_business_graph()` — line 51 `Err(_) => (build_business_graph(&rows)?, None)`. (2) If no API key is configured, AI is skipped entirely (line 121-122). (3) The agent has a domain failure budget (`MAX_DOMAIN_FAILURES`) — if too many parallel domain analyses fail, it stops early instead of failing the whole run (lines 181-184). **Gap:** No graceful degradation when the AI call fails *during* the agent run (e.g., `chat_fresh()` fails in Phase 3 at line 206) — this propagates as a hard error and aborts the entire analysis. |
| **Recommendation** | (1) In `run_agent()`, consider catching network errors in Phase 3 (cross-domain) and returning a partial report from completed phases instead of aborting. (2) Cache the last successful AI report in the DB — if a re-analysis fails due to network, offer the cached report. (3) Add a `--no-ai` CLI flag for fully offline mode. |

## Criterion 7: Connection Pool Management

| | |
|---|---|
| **Score** | **D** |
| **Evidence** | `src/ai/chat.rs:79` — `reqwest::Client::new()` inside the retry loop |
| **Details** | `reqwest::Client` is created **on every retry attempt** inside the `for attempt in 0..=max_retries` loop (line 72 → 79). This means: (1) A new connection pool is allocated per attempt — no connection reuse across retries. (2) In `run_agent()`, parallel domain tasks (line 155-160) each create their own Client via `chat_fresh()` → `send_chat_request()` — no shared connection pool across concurrent requests. (3) reqwest's `Client` internally maintains a connection pool (via hyper) — creating it once and reusing it would allow TCP/TLS connection reuse for keep-alive connections to the same API endpoint. |
| **Recommendation** | (1) Move `reqwest::Client::new()` outside the retry loop — create once per `send_chat_request()` call. (2) Better: create a shared `Client` in `run_agent()` and pass it through to all `chat_fresh()` calls — this enables connection pooling across all 4 agent phases and parallel domain tasks. (3) For production: consider `Client::builder().pool_max_idle_per_host(4).pool_idle_timeout(Duration::from_secs(90))` to match the parallelism in Phase 2. This is a low-effort change with meaningful latency improvement for multi-domain analysis. |

---

## Overall Assessment

| Criterion | Score | Priority |
|-----------|-------|----------|
| Reconnection / Retry | **B** | Medium — add jitter + Retry-After |
| Timeout Handling | **D** | **High** — requests can hang indefinitely |
| Proxy Resilience | **F** | Low — CLI tool, env-var proxy is usually enough |
| Persistent Connection Health | **N/A** | N/A — no long-lived connections |
| Network Error Classification | **C** | Medium — transient/permanent distinction is incomplete |
| Graceful Degradation | **B** | Medium — good fallback for AI identification, weak for mid-agent failures |
| Connection Pool Management | **D** | Medium — client created per attempt, no reuse |

**Overall Grade: C+**

The retry-with-backoff in `chat.rs` is a solid foundation. Graceful degradation for the AI identification step is well-designed. The two critical gaps are: **(1) no timeout configuration** — requests can hang the CLI indefinitely, and **(2) Client created per retry attempt** — wastes connection pooling. Both are straightforward fixes.

### Top 3 Improvements (by effort/impact)

1. **[10 min]** Set timeouts on `reqwest::Client::builder()` in `chat.rs:79` — prevents hung CLI
2. **[15 min]** Hoist `Client` creation out of retry loop and share across agent phases — enables connection reuse
3. **[20 min]** Add jitter to retry backoff and classify `reqwest::Error` as transient/permanent — prevents wasted retries on permanent failures

---

*Generated by network resilience review. Only source files in `src/` were analyzed.*

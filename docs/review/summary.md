# Project Review Summary

**Project:** bizgraph
**Date:** 2026-06-05
**Stack:** Rust 2021 / clap 4 + rusqlite 0.31 + reqwest 0.12 + tokio 1

## Scores

| Angle | Score | Key Finding |
|-------|-------|-------------|
| Architecture | A- | Clean 5-layer model, ai/ module split well-done; main.rs leaks into db.rs |
| Code Quality | B+ | Strong conventions, zero TODO/FIXME; graph.rs has duplicated construction logic |
| Error Handling | A | Comprehensive typed Error enum, proper propagation, almost no unwrap() |
| Testing | B- | 108 tests, high quality where they exist; only 3/10 files tested, zero CI |
| API Design | B | Consistent patterns, good boundary checks; sparse doc comments, no versioning policy |
| Infrastructure | C | No CI/CD, no rust-toolchain.toml, no cargo-audit, API key plaintext in config |
| Runtime Health | C+ | Good retry/backoff, budget enforcement; no HTTP timeout, no signal handling |
| Network Resilience | C+ | Solid retry logic; no request timeout (hangs indefinitely), Client created per attempt |

## Overall Grade: B-

Well-engineered core code with exemplary determinism practices and error handling. The main gaps are operational: no CI/CD, no HTTP timeouts, and limited test coverage. For a personal CLI tool at v0.1.1 this is solid work; for production use, the timeout and CI gaps need fixing.

## Top 3 Strengths

1. **Determinism-first design** — UUIDv5 from stable keys, BTreeMap everywhere, sorted outputs. Same input always produces same graph. This is rare and valuable.
2. **Error handling** — Custom Error enum with 25+ typed variants, From impls for all upstream types, proper context strings. Almost zero unwrap() in production code.
3. **Clean module decomposition** — ai/ was split from a 1623-line god module into 5 focused sub-modules (prompts, chat, agent, summarization, business). Zero cycles, clear dependency direction.

## Top 3 Areas to Improve

1. **No HTTP timeout** (`src/ai/chat.rs:79`) — `reqwest::Client::new()` has no timeout. A slow AI API will hang the CLI indefinitely. Fix: `.timeout(Duration::from_secs(120))`. 5 minutes of work.
2. **No CI/CD** — Zero automation. No cargo test, no clippy, no fmt check on push. A single GitHub Actions workflow would catch regressions. 30 minutes of work.
3. **Test coverage gaps** — 108 tests in 3 files (parser, graph, business). Zero tests for db.rs, lib.rs, agent.rs, summarization.rs, chat.rs. The core orchestration path is untested.

## Quick Wins (high impact, low effort)

1. Add `timeout(120s)` to reqwest Client in chat.rs — prevents hung CLI (5 min)
2. Reuse reqwest Client across requests instead of creating per retry — enables connection pooling (15 min)
3. Fix 2 `unwrap()` calls in parser.rs — close the only gap in no-unwrap rule (15 min)
4. Add `cargo-audit` to check dependency vulnerabilities (5 min)

## Systemic Issues (patterns, not one-offs)

1. **No operational resilience** — No timeouts, no signal handling, no graceful shutdown. The tool works but cannot handle real-world network conditions.
2. **Test desert outside core** — parser.rs and graph.rs are well-tested; everything else has zero tests. The orchestration layer (lib.rs, agent.rs) is a blind spot.
3. **Infrastructure debt** — No CI, no release process, no changelog, no rust-toolchain.toml. Fine for solo dev, blocks collaboration.

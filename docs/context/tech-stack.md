# Tech Stack

> Auto-generated from `Cargo.toml`, `Cargo.lock`, and `install.sh`.  
> Last updated: 2026-06-04

## Language & Toolchain

| Item | Value |
|------|-------|
| Language | Rust |
| Edition | 2021 |
| Crate version | 0.1.1 |
| License | MIT |
| rustc (dev machine) | 1.93.0 |
| cargo (dev machine) | 1.93.0 |
| rust-toolchain file | None (uses system default) |
| `.cargo/config.toml` | None |

## Crate Layout

| Target | Path | Description |
|--------|------|-------------|
| `[lib]` | `src/lib.rs` | Public API — `analyze()`, `analyze_with_project()`, `load_config()` |
| `[[bin]]` | `src/main.rs` | CLI binary — clap derive, `analyze` + `project` subcommands |

## Direct Dependencies

### Core — Data & Parsing

| Crate | Version Spec | Resolved | Features | Purpose |
|-------|-------------|----------|----------|---------|
| `serde` | `1` | 1.0.228 | `derive` | Serialization framework |
| `serde_json` | `1` | 1.0.150 | — | JSON parsing (HAR format) |
| `url` | `2` | 2.5.8 | — | URL parsing and normalization |
| `toml` | `0.8` | 0.8.23 | — | Config file parsing (`config.toml`) |

### CLI

| Crate | Version Spec | Resolved | Features | Purpose |
|-------|-------------|----------|----------|---------|
| `clap` | `4` | 4.6.1 | `derive` | Command-line argument parsing |

### Database

| Crate | Version Spec | Resolved | Features | Purpose |
|-------|-------------|----------|----------|---------|
| `rusqlite` | `0.31` | 0.31.0 | `bundled` | SQLite persistence (WAL mode, upsert) |

### Async / HTTP

| Crate | Version Spec | Resolved | Features | Purpose |
|-------|-------------|----------|----------|---------|
| `tokio` | `1` | 1.52.3 | `full` | Async runtime |
| `reqwest` | `0.12` | 0.12.28 | `json` | HTTP client (AI API calls) |

### Utilities

| Crate | Version Spec | Resolved | Features | Purpose |
|-------|-------------|----------|----------|---------|
| `chrono` | `0.4` | 0.4.44 | `serde` | Timestamps, date handling |
| `uuid` | `1` | 1.23.1 | `v4`, `v5`, `serde` | Deterministic node IDs (UUIDv5 from stable keys) |

## Dev Dependencies

None declared. No `[dev-dependencies]` section in `Cargo.toml`.

## Build Tools

| Tool | Source | Purpose |
|------|--------|---------|
| `cargo build --release` | `install.sh` | Release build |
| `cp` + `chmod` | `install.sh` | Install binary to `~/.local/bin/` |

No Makefile, no CI config, no linter/formatter config (rustfmt, clippy) checked into the repo.

## Runtime Requirements

| Requirement | Notes |
|-------------|-------|
| SQLite | Bundled at compile time via `rusqlite/bundled` → `libsqlite3-sys 0.28.0` (no system SQLite needed) |
| TLS | System `native-tls` (OpenSSL on Linux, Secure Transport on macOS, SChannel on Windows) |
| Network | HTTP/HTTPS for AI API calls (default: DeepSeek endpoint) |
| `~/.config/bizgraph/config.toml` | Optional — API key, model, API URL |
| `~/.config/bizgraph/bizgraph.db` | Auto-created SQLite database |
| `~/.local/bin/` | Install target (must be in `$PATH`) |

## Notable Transitive Dependencies

| Crate | Resolved | Pulled By | Role |
|-------|----------|-----------|------|
| `hyper` | 1.9.0 | `reqwest` | HTTP engine |
| `libsqlite3-sys` | 0.28.0 | `rusqlite` (bundled) | Compiled SQLite C library |
| `native-tls` | 0.2.18 | `reqwest` | Platform TLS backend |
| `serde_derive` | — | `serde` (derive) | Proc-macro for `#[derive(Serialize, Deserialize)]` |
| `clap_derive` | — | `clap` (derive) | Proc-macro for `#[derive(Parser)]` |

## Version Constraints Summary

| Constraint | Value |
|------------|-------|
| Minimum Rust version | Not explicitly pinned (edition 2021 = rustc 1.56+) |
| Effective minimum | Likely rustc ≥ 1.70+ (dep features), confirmed working on 1.93.0 |
| Lock file | `Cargo.lock` committed — pinned resolved versions |
| Total locked packages | ~100 (exact count from `Cargo.lock`) |

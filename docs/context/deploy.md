# Deploy / Build / Test Reference

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | Edition 2021 — any recent `rustup` default works |
| C compiler | Needed by rusqlite bundled SQLite build |
| `$HOME/.config/bizgraph/` | Auto-created on first run; holds `config.toml` and `bizgraph.db` |
| `$HOME/.local/bin/` | Default install target; must be in `$PATH` |

No system SQLite required — rusqlite uses the `bundled` feature.

## Build

```bash
# Debug build (fast compile, slower binary)
cargo build

# Release build (optimized)
cargo build --release

# Run directly
cargo run -- analyze traffic.har --project myproject
cargo run --release -- analyze traffic.har --project myproject
```

## Install

The project ships `install.sh` which does a release build and copies the binary to `~/.local/bin/`:

```bash
./install.sh
# Steps: cargo build --release → cp target/release/bizgraph ~/.local/bin/
# Checks PATH, suggests export if missing
```

Manual install (equivalent):

```bash
cargo build --release
mkdir -p ~/.local/bin
cp target/release/bizgraph ~/.local/bin/bizgraph
chmod +x ~/.local/bin/bizgraph
```

Verify:

```bash
bizgraph --help
```

## Configuration

Config file only — **no environment variables** for API keys or model settings.

```toml
# ~/.config/bizgraph/config.toml
api_key = "sk-..."
model = "deepseek-v4-pro"
api_url = "https://api.deepseek.com/chat/completions"
```

| Field    | Required | Default                                  |
|----------|----------|------------------------------------------|
| `api_key`| Yes      | — (AI features disabled without it)      |
| `model`  | No       | `deepseek-v4-pro`                        |
| `api_url`| No       | `https://api.deepseek.com/chat/completions` |

The DB is stored at `~/.config/bizgraph/bizgraph.db` (SQLite, WAL mode).

## Test

Tests are **inline `#[cfg(test)]` modules** — no separate test directory.

| File              | Test count | Covers                                    |
|-------------------|------------|-------------------------------------------|
| `src/graph.rs`    | ~20+       | Node/edge construction, stable keys, schema inference |
| `src/parser.rs`   | ~9         | HAR parsing, validation, TrafficRow extraction |
| `src/ai/business.rs` | ~16   | Business function identification logic     |

```bash
# Run all tests
cargo test

# Run tests in a specific module
cargo test --lib graph
cargo test --lib parser
cargo test --lib ai::business

# Run a single test by name
cargo test test_endpoint_stable_key

# Verbose output
cargo test -- --nocapture
```

## Lint / Format

```bash
# Format check
cargo fmt --check

# Format
cargo fmt

# Clippy lints
cargo clippy -- -D warnings
```

## CI/CD

**No CI pipeline exists.** No `.github/workflows/`, `.gitlab-ci.yml`, or other CI config found in the repo. Builds, tests, and installs are manual.

## Release Checklist

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
# bump version in Cargo.toml
# commit, tag
./install.sh
```

## Infrastructure Requirements

| Component | Spec |
|-----------|------|
| OS        | Linux, macOS (Windows untested) |
| Disk      | ~50 MB for release build artifacts |
| Network   | Only for AI features (DeepSeek API or compatible OpenAI-format endpoint) |
| Database  | SQLite file in `~/.config/bizgraph/` — no server process |
| Memory    | Standard desktop; no heavy requirements |

## File Layout After Install

```
~/.config/bizgraph/
├── config.toml       # API key, model, URL
└── bizgraph.db       # SQLite projects/graphs/history

~/.local/bin/
└── bizgraph          # CLI binary
```

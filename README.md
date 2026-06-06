```text
██████╗ ██╗███████╗ ██████╗ ██████╗  █████╗ ██████╗ ██╗  ██╗
██╔══██╗██║╚══███╔╝██╔════╝ ██╔══██╗██╔══██╗██╔══██╗██║  ██║
██████╔╝██║  ███╔╝ ██║  ███╗██████╔╝███████║██████╔╝███████║
██╔══██╗██║ ███╔╝  ██║   ██║██╔══██╗██╔══██║██╔═══╝ ██╔══██║
██████╔╝██║███████╗╚██████╔╝██║  ██║██║  ██║██║     ██║  ██║
╚═════╝ ╚═╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝     ╚═╝  ╚═╝
```

[![Rust](https://img.shields.io/badge/rust-2021+-ed8225?style=flat-square&logo=rust&logoColor=white)](https://rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-22C55E?style=flat-square)](LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/trtyr/bizgraph/ci.yml?style=flat-square&label=ci)](https://github.com/trtyr/bizgraph/actions)

# bizgraph

**Turn HAR traffic captures into a deterministic business graph.**

CLI + library that parses `.har` files into stable business graphs, persists to SQLite, and generates AI-powered analysis reports that answer 8 core business questions. For pentesters and security researchers who need to understand a target's business structure from captured traffic.

[GitHub](https://github.com/trtyr/bizgraph) · [Quick Start](#quick-start) · [CLI Reference](#cli-reference) · [AI Analysis](#ai-analysis) · [Ask (Conversational Q&A)](#ask-conversational-qa) · [Visualization](#visualization) · [Architecture](#architecture)

## Quick Start

```bash
# Install from crates.io (recommended)
cargo install bizgraph

# Or install from source
git clone https://github.com/trtyr/bizgraph.git && cd bizgraph
cargo install --path .

# Configure AI (optional, enables AI analysis + ask)
mkdir -p ~/.config/bizgraph
cat > ~/.config/bizgraph/config.toml << 'EOF'
api_key = "sk-..."
model = "deepseek-v4-pro"
api_url = "https://api.deepseek.com/chat/completions"
EOF

# 1. Analyze HAR traffic — AI report runs automatically
bizgraph analyze traffic.har --project myproject

# 2. View the AI report
bizgraph project report myproject

# 3. Ask follow-up questions (conversational, with context)
bizgraph ask "有哪些业务？" --project myproject
bizgraph ask "认证接口的参数是什么？" --project myproject

# 4. Interactive visualization
bizgraph project viz myproject
```

## CLI Reference

### Analyze

```bash
bizgraph analyze <traffic.har> [--project <name>] [--host <prefix>]
```

- `<traffic.har>`: input HAR file (HTTP Archive 1.2, exported from browser DevTools)
- `--project, -p`: project name or ID (auto-creates if not exists)
- `--host, -H`: filter requests by host prefix

AI analysis runs automatically if an API key is configured. On incremental re-runs, AI business function identification is cached; the report is always regenerated.

### Ask (Conversational Q&A)

```bash
bizgraph ask "<question>" --project <name> [--clear]
```

Multi-turn conversation with full context. Each call:
1. Loads the project's graph data as persistent context
2. Includes previous conversation history
3. Sends everything to the AI with your question
4. Persists both question and answer to SQLite for future rounds

```bash
bizgraph ask "有哪些业务？" --project myproject
bizgraph ask "认证接口的参数是什么？" --project myproject        # remembers prior context
bizgraph ask "它的数据依赖关系呢？" --project myproject          # continues the thread
bizgraph ask "重新开始" --project myproject --clear              # clear history, start fresh
```

### Project Management

```bash
bizgraph project new <name>                    # Create a new project
bizgraph project list                           # List all projects
bizgraph project show <name>                    # Stats, graph metrics, business tree
bizgraph project history <name>                 # Analysis history
bizgraph project diff <name>                    # Compare last two analyses
bizgraph project report <name>                  # Full AI report
bizgraph project viz <name>                     # Interactive HTML visualization
bizgraph project export <name> -o graph.json    # Export graph as JSON
bizgraph project delete <name> --force          # Delete project
```

## AI Analysis

When an API key is configured, `analyze` automatically runs a multi-phase AI agent that answers 8 business questions:

| # | Question | Section |
|---|----------|---------|
| 1 | What businesses exist? | Business Overview |
| 2 | What endpoints does each business have? | Endpoint Catalog by Business |
| 3 | What does each endpoint do? | Endpoint Purpose Analysis |
| 4 | What is the call sequence between endpoints? | Call Sequence and Flow |
| 5 | What data dependencies exist? | Data Dependencies |
| 6 | How do business domains relate? | Cross-Business Relationships |
| 7 | What are the core business flows? | Core Business Flows |
| 8 | Which endpoints are business-critical? | Key Business Endpoints |

The AI agent works in 3 phases:
- **Phase 1**: High-level business overview from the full graph
- **Phase 2**: Per-domain deep dives (parallel, scoped context per domain)
- **Phase 3**: Cross-domain correlation and final report synthesis

### Token Budget

For large HAR files, the agent manages a token budget to avoid exceeding model context limits. Default: 150K tokens. Configurable in `config.toml`:

```toml
api_key = "sk-..."
token_budget = 200000    # optional, default 150000
```

## Visualization

```bash
bizgraph project viz myproject
```

Generates an interactive HTML file (`graph.html`) with:

- **Three edge types**: hierarchy (grey), call sequence (blue), data dependencies (red dashed)
- **Toggle controls**: show/hide endpoints, descriptions, call flow, dependencies
- **Click-to-inspect**: click any node for detailed info (type, description, parameters, connections)
- **Legend**: color-coded for quick orientation

## Output Format

`bizgraph analyze` produces a `BusinessGraph`:

- **Nodes**: `Host`, `BusinessFunction`, and `Endpoint`
- **Edges**: `contains` (host→bf), `calls_after` (sequential flow), `data_dependency:*` (shared data)
- **Properties**: traffic-derived metadata including methods, paths, parameters, confidence scores

All IDs are deterministic (UUIDv5 from stable keys) — same input always produces the same graph.

## Configuration

Config file at `~/.config/bizgraph/config.toml`:

```toml
api_key = "sk-..."                                    # API key for AI analysis
model = "deepseek-v4-pro"                             # optional, default shown
api_url = "https://api.deepseek.com/chat/completions"  # optional
token_budget = 150000                                  # optional, default shown
```

No environment variables. Everything goes through the config file.

## Architecture

```text
bizgraph/
├── Cargo.toml              # Single crate — [lib] + [[bin]]
├── src/
│   ├── main.rs             # CLI — clap derive, analyze + project + ask subcommands
│   ├── lib.rs              # Public API: analyze(), ask(), clear_conversation(), load_config()
│   ├── types.rs            # BusinessGraph, BusinessNode, BusinessEdge, Project
│   ├── error.rs            # Custom Error enum with typed variants
│   ├── parser.rs           # HAR parsing, TrafficRow extraction, host filtering
│   ├── graph.rs            # Deterministic node/edge construction, path normalization
│   ├── db.rs               # SQLite persistence — WAL mode, upsert, graph merge, conversations
│   └── ai/
│       ├── mod.rs          # Re-export: analyze_with_ai(), analyze_with_ai_deep()
│       ├── prompts.rs      # System prompts (8-section), agent identity, token limits
│       ├── chat.rs         # Chat API types + HTTP client (timeout, retry with jitter)
│       ├── agent.rs        # Agent state, 3-phase orchestration, scoped context
│       ├── summarization.rs # Graph serialization for prompts
│       └── business.rs     # AI business function identification from raw traffic
```

### Module Dependencies

| Module | Depends On | Used By |
|--------|-----------|---------|
| `types.rs` | — | everything |
| `error.rs` | — | everything |
| `parser.rs` | url, serde_json | lib, graph |
| `graph.rs` | types | lib |
| `db.rs` | types, ai::chat | lib |
| `ai/` | types | lib |
| `lib.rs` | all above | main |
| `main.rs` | lib | — |

## Building

- **Rust** ≥ 1.93 (pinned in `rust-toolchain.toml`)
- **No C library required** — rusqlite uses bundled SQLite

```bash
cargo build                          # Debug
cargo build --release                # Release
cargo test                           # Run all tests
cargo clippy --all-targets           # Lint
./install.sh                         # Build release + install to ~/.local/bin
```

## Companion Tool

Need to map attack paths after you understand the business surface? See [Theseus](https://github.com/trtyr/theseus).

## License

MIT

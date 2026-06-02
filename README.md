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
[![Platform](https://img.shields.io/badge/platform-cross--platform-8B5CF6?style=flat-square)]()

# bizgraph

**Turn HAR traffic captures into a deterministic business graph.** CLI + library that parses `.har` files into stable business graphs, persists to SQLite, and generates AI-powered analysis reports. Built on clap 4 + rusqlite + reqwest + tokio. For pentesters and security researchers who need to understand a target's business structure from captured traffic.

[GitHub](https://github.com/trtyr/bizgraph) · [Quick Start](#quick-start) · [CLI Reference](#cli-reference) · [Architecture](#architecture) · [Building](#building)

## Quick Start

```bash
# Install from source
git clone https://github.com/trtyr/bizgraph.git
cd bizgraph
cargo install --path .

# Analyze HAR traffic — AI analysis runs automatically if API key is configured
bizgraph analyze traffic.har --project myproject

# Filter by host
bizgraph analyze traffic.har --project myproject --host target.com

# View project
bizgraph project show myproject
```

## CLI Reference

### Analyze

```bash
bizgraph analyze <traffic.har> [--project <name>] [--host <prefix>]
```

- `<traffic.har>`: input HAR file (HTTP Archive 1.2, exported from browser DevTools)
- `--project, -p`: project name or ID to save results
- `--host, -H`: prefix filter against the request host

### Project Management

```bash
bizgraph project new <name>          # Create a new project
bizgraph project list                # List all projects
bizgraph project show <name>         # Show stats, graph metrics, business tree
bizgraph project history <name>      # Show analysis history
bizgraph project diff <name>         # Compare last two analyses
bizgraph project report <name>       # Show full AI report
bizgraph project viz <name>          # Generate interactive HTML visualization
bizgraph project export <name> -o graph.json  # Export graph as JSON
bizgraph project delete <name> --force  # Delete project
```

### Configuration

Config file at `~/.config/bizgraph/config.toml`:

```toml
api_key = "sk-..."                          # API key for AI analysis
model = "deepseek-v4-pro"                   # optional, default shown
api_url = "https://api.deepseek.com/chat/completions"  # optional
```

## Output Format

`bizgraph analyze` produces a `BusinessGraph`:

- **Nodes**: `Host`, `BusinessFunction`, and `Endpoint`
- **Edges**: `contains` (host→bf), `calls_after` (sequential flow), `data_dependency:*` (shared data)
- **Properties**: traffic-derived metadata for each node and edge

All IDs are deterministic (UUIDv5 from stable keys) — same input always produces the same graph.

## Architecture

```text
bizgraph/
├── src/
│   ├── main.rs         # CLI — clap derive, analyze + project subcommands
│   ├── lib.rs          # Public API: analyze(), analyze_with_project(), load_config()
│   ├── types.rs        # BusinessGraph, BusinessNode, BusinessEdge, Project
│   ├── error.rs        # Custom Error enum with typed variants
│   ├── parser.rs       # HAR parsing, TrafficRow extraction, host filtering
│   ├── graph.rs        # Deterministic node/edge construction, path normalization
│   ├── db.rs           # SQLite persistence — WAL mode, upsert, graph merge
│   └── ai/
│       ├── mod.rs      # Re-export: analyze_with_ai(), analyze_with_ai_deep()
│       ├── prompts.rs  # System prompts, token limits
│       ├── chat.rs     # Chat API types + HTTP client
│       ├── agent.rs    # Agent state, 4-phase orchestration
│       └── summarization.rs  # Graph serialization for prompts
```

## Building

- **Rust** ≥ 1.56 (edition 2021)
- **No C library required** — bizgraph is pure Rust (rusqlite uses bundled SQLite)

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Install to ~/.local/bin
./install.sh
```

## Companion Tool

Need to map attack paths after you understand the business surface? See [Theseus](https://github.com/trtyr/theseus).

## License

MIT

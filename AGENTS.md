# BIZGRAPH

**Business graph extractor for Yakit Excel traffic.** Single binary — CLI + library outputting deterministic JSON.

**Stack**: clap 4 · calamine 0.26 · serde · serde_json · uuid · chrono

## STRUCTURE

```text
bizgraph/
├── Cargo.toml              # Single crate — [lib] + [[bin]]
├── src/
│   ├── main.rs             # CLI binary — clap derive, analyze subcommand
│   ├── lib.rs              # Public analyze() entrypoint
│   ├── types.rs            # BusinessGraph, BusinessNode, BusinessEdge, parser structs
│   ├── parser.rs           # Yakit Excel workbook parsing and row normalization
│   └── graph.rs            # Deterministic node/edge construction
├── install.sh              # Build release + install to ~/.local/bin
├── AGENTS.md
└── README.md
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add/change a type | `src/types.rs` | Shared graph types live here |
| Adjust Excel parsing | `src/parser.rs` | Header detection, row extraction, host filtering |
| Change graph mapping | `src/graph.rs` | Stable keys, labels, node/edge assembly |
| Add CLI flags | `src/main.rs` | clap derive only |
| Change public API | `src/lib.rs` | Keep `pub fn analyze(...)` stable |
| Update install flow | `install.sh` | Release build + local binary copy |

## CODE MAP

### `src/types.rs` — Shared Types

| Symbol | Type | Role |
|--------|------|------|
| `BusinessGraph` | struct | Top-level output with `nodes` and `edges` |
| `BusinessNode` | struct | Business graph node with stable key and JSON properties |
| `BusinessNodeKind` | enum | `Host`, `BusinessFunction`, `Endpoint` |
| `BusinessEdge` | struct | Directed relationship between nodes |
| `ParsedWorkbook` | struct | Internal parser result with rows and workbook metadata |
| `TrafficRow` | struct | Normalized Yakit traffic record |

### `src/parser.rs` — Workbook Parsing

- Opens `.xlsx` via `calamine::open_workbook_auto`
- Detects Yakit-style headers from the first matching row in each sheet
- Maps rows into `TrafficRow`
- Applies optional Host prefix filtering before graph construction

### `src/graph.rs` — Graph Mapping

- Builds deterministic `Host` → `BusinessFunction` → `Endpoint` chains
- Deduplicates nodes and edges by `stable_key`
- Sorts output for stable JSON diffs
- Generates deterministic IDs from stable keys

### `src/main.rs` — CLI

- `bizgraph analyze --yakit-excel ...`
- Summary mode prints counts only
- Full mode emits JSON to stdout or file

## YAKIT EXCEL COLUMN MAPPING

| Meaning | Accepted headers |
|---------|------------------|
| Host | `Host`, `Hostname` |
| Method | `Method`, `Request Method` |
| Path | `Path`, `Request Path` |
| URL | `URL`, `Request URL` |
| Status code | `Status`, `Status Code`, `Response Status Code` |
| Content type | `Content-Type`, `MIME Type`, `Response Content Type` |

If `Path` is absent, derive it from `URL`. If both Host and URL are empty, skip the row.

## CONVENTIONS

- **BusinessNodeKind variants**: `Host`, `BusinessFunction`, `Endpoint` only
- **stable_key format**:
  - Host: `host:<normalized-host>`
  - Business function: `business_function:<normalized-host>:<function-hint>`
  - Endpoint: `endpoint:<normalized-host>:<method>:<normalized-path>`
- **Edge labels**: `contains` for host→function, `exposes` for function→endpoint
- **Determinism**: sort nodes/edges and derive IDs from stable keys, never randomize output order
- **Properties**: always store extra parser data in `serde_json::Value` objects

## COMMANDS

```bash
./install.sh
cargo build
cargo check
cargo run -- analyze --yakit-excel traffic.xlsx --summary
cargo run -- analyze --yakit-excel traffic.xlsx --host target.com --pretty
```

## ANTI-PATTERNS

- Don't add web server, database, or frontend code
- Don't make IDs random for the same input workbook
- Don't rely on sheet order for semantic meaning beyond deterministic traversal
- Don't stuff parser-only helpers into `main.rs`
- Don't change stable key shapes without updating docs and downstream expectations

## NOTES

- `calamine` stringifies mixed Excel cell types; normalize before graph mapping
- Header names may vary by export; add aliases conservatively
- Deterministic IDs are derived from stable keys, not workbook row position alone
- This crate is the business-surface companion to Theseus, not an attack graph tool

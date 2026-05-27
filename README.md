```text
██████╗ ██╗███████╗ ██████╗ ██████╗  █████╗ ██████╗ ██╗  ██╗
██╔══██╗██║╚══███╔╝██╔════╝ ██╔══██╗██╔══██╗██╔══██╗██║  ██║
██████╔╝██║  ███╔╝ ██║  ███╗██████╔╝███████║██████╔╝███████║
██╔══██╗██║ ███╔╝  ██║   ██║██╔══██╗██╔══██║██╔═══╝ ██╔══██║
██████╔╝██║███████╗╚██████╔╝██║  ██║██║  ██║██║     ██║  ██║
╚═════╝ ╚═╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝     ╚═╝  ╚═╝
```

# bizgraph

Turn Yakit Excel traffic into a clean business graph.

BizGraph is a standalone Rust CLI for pentesters who want to understand a target's business structure from captured traffic. It reads Yakit Excel exports, groups requests into stable business-facing entities, and emits deterministic JSON for downstream analysis.

## Quick start

```bash
cargo install bizgraph
bizgraph analyze --yakit-excel traffic.xlsx --host target.com --output graph.json
```

## Who it's for

Pen testers who need to move from raw HTTP traffic to a compact map of hosts, business functions, and endpoints.

## Output format

`bizgraph analyze` writes a `BusinessGraph` JSON document:

- `nodes`: `Host`, `BusinessFunction`, and `Endpoint`
- `edges`: stable directed links such as `contains` and `exposes`
- `properties`: traffic-derived metadata for each node and edge

## CLI reference

```bash
bizgraph analyze --yakit-excel <traffic.xlsx> [--host <prefix>] [--output <graph.json>] [--summary] [--pretty]
```

- `--yakit-excel, -f`: input Yakit `.xlsx` export
- `--host, -H`: prefix filter against the Host column
- `--output, -o`: write JSON to a file instead of stdout
- `--summary`: print node and edge counts only
- `--pretty`: pretty-print JSON output

## Install

From crates.io:

```bash
cargo install bizgraph
```

From source:

```bash
git clone https://github.com/trtyr/bizgraph.git
cd bizgraph
./install.sh
```

## Example

```bash
bizgraph analyze --yakit-excel traffic.xlsx --host target.com --output graph.json
```

## Companion tool

Need to map attack paths after you understand the business surface? See Theseus: https://github.com/trtyr/theseus

## License

MIT

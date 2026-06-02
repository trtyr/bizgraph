# Plan Tree

## Authority

1. `docs/plantree/baseline/runtime-flows.md`
2. `docs/plantree/baseline/README.md`

## Active Plans

| Plan | Status | Scope |
|------|--------|-------|
| *(none)* | — | — |

## Design Decisions (from runtime-flows review)

| Decision | Detail |
|----------|--------|
| 全局配置目录 | `~/.config/bizgraph/` — config.toml + bizgraph.db |
| 配置 key 通用化 | `api_key` / `model` / `api_url`，不绑定 DeepSeek |
| AI 为默认行为 | 分析后默认展示业务结构，不需要 `--ai` |
| API 配置走全局 | 不放项目级别，统一在 config.toml |
| 数据库全局 | `~/.config/bizgraph/bizgraph.db`，不在 CWD |

## Completed

| Plan | Scope |
|------|-------|
| `plans/custom-error-type/` | Replace `Result<_, String>` with custom `Error` across crate |
| *(inline)* | Delete server.rs — remove stub + unused deps |
| *(inline)* | graph.rs unit tests — 83 tests for zero-coverage core module |
| *(inline)* | HAR migration — remove Excel support, rewrite parser.rs |
| *(inline)* | Make `--project` required for analyze command |
| *(inline)* | Global database — `~/.config/bizgraph/bizgraph.db` |
| *(inline)* | Generic config — `api_key` not `deepseek_api_key` |
| *(inline)* | AI default behavior |
| *(inline)* | Business tree display — host→bf→endpoint after analysis |
| *(inline)* | AI context limits — TURN_DATA_CHAR_LIMIT 3.2K→200K |

## Read Path

1. `docs/plantree/baseline/README.md`
2. `docs/plantree/baseline/runtime-flows.md`

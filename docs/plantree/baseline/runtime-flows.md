# Runtime Flows

## Business Flow: HAR Analysis

```text
① bizgraph project new <name>
         │
         ▼
② bizgraph analyze <har> --project <name>
         │
         ├── --host <filter>      # 可选：按 host 前缀过滤
         │
         ▼
   ┌──────────────────┐
   │  parse_har()     │  HAR JSON → Vec<TrafficRow>
   │  build_graph()   │  TrafficRow → BusinessGraph (确定性)
   │  db.merge_graph()│  合并到项目 SQLite
   │  AI (default)    │  LLM API → 业务分析报告
   └──────────────────┘
         │
         ▼
   ┌──────────────────┐
   │  Business Tree   │  自动展示 host→bf→endpoint 树形结构
   │  AI Report       │  展示 AI 分析报告摘要
   └──────────────────┘
         │
         ▼
③ bizgraph project show <name>
④ bizgraph project history <name>
⑤ bizgraph project export <name> -o graph.json
```

### Step-by-Step

#### ① 创建项目

```bash
$ bizgraph project new gezida
Created project 'gezida' (ec7dd0ed)
```

#### ② 分析 HAR

```bash
$ bizgraph analyze traffic.har --project gezida
Project: gezida
Imported rows: 373
New nodes: 174
Updated nodes: 0
New edges: 950
Skipped edges: 0
Nodes: 174 total
  business_function: 17
  endpoint: 157
Edges: 950 total

Business Structure:
  [host] co.gocheck.cn
    [bf] /  (1 endpoints)
      GET    /  28%
    [bf] /cdn  (59 endpoints)
      GET    /cdn/antd/locale-provider/zh_CN.js  28%
      ...
  [host] pm.gocheck.cn
    [bf] /pm-gezida  (66 endpoints)
      POST   /pm-gezida/user/login  83%  params: [login, password, schoolCode, ...]
      GET    /pm-gezida/user/info/current  48%
      ...

AI Report Preview:
## 一、核心业务
GoCheck 是一个面向高校的学术论文管理 SaaS 平台...
```

#### ③ 查看项目状态

```bash
$ bizgraph project show gezida
Project: gezida
ID: ec7dd0ed
Created: 2026-06-01T02:31:27.092621+00:00
Nodes: 174 total
  business_function: 17
  endpoint: 157
Edges: 950 total
Analyses: 1 total
Last analysis: 2026-06-01T02:32:01.664771+00:00 (rows=373, +nodes=174, ~nodes=0, +edges=950, skipped_edges=0)
```

> **注意**: ③ 和 ② 的输出高度重复。③ 的价值在于：多次分析后查看累计状态，以及显示项目 ID 和创建时间。

#### ④ 查看分析历史

```bash
$ bizgraph project history gezida
History for gezida:
2026-06-01T02:32:01.664771+00:00	rows=373	+nodes=174	~nodes=0	+edges=950	skipped_edges=0	host=-	source=traffic.har
```

#### ⑤ 导出图 JSON

```bash
$ bizgraph project export gezida -o gezida-graph.json
Exported 'gezida' to gezida-graph.json
```

> 导出大小：~865 KB，~25,600 行。包含所有节点的 stable_key、路径模板、schema 推断、置信度，以及全部边关系。

### Key Behaviors

- **项目自动创建**：`analyze --project <name>` 如果项目不存在会自动创建
- **图合并**：同一项目多次分析不同 HAR 文件，节点和边会合并（upsert）
- **确定性**：相同输入永远产生相同的 stable_key 和 UUID
- **host 过滤**：`--host pm.gocheck.cn` 只分析该 host 的流量
- **AI 报告**：需要配置 `~/.config/bizgraph/config.toml` 中的 API key

### Data Flow

```text
.har file
  → parse_har() → Vec<TrafficRow>
  → build_business_graph() → BusinessGraph { nodes, edges }
  → db.merge_graph() → SQLite (projects, nodes, edges, analyses)
  → (optional) AI → analysis report string
  → db.record_analysis() → history entry
```

### Typical Session

```bash
# 首次使用
bizgraph project new gezida
bizgraph analyze ~/Downloads/traffic.har --project gezida --host pm.gocheck.cn
bizgraph project show gezida

# 追加分析（同一项目，不同 HAR）
bizgraph analyze ~/Downloads/traffic2.har --project gezida --host pm.gocheck.cn
bizgraph project history gezida

# 导出
bizgraph project export gezida -o gezida-graph.json
```

## Known Issues

### 流程问题

1. **`project show` 和分析摘要重复** — ③ 和 ② 输出高度重复，③ 的独特价值只有项目 ID 和创建时间

### 已解决

| 问题 | 解决方式 |
|------|----------|
| 缺少业务内容展示 | 分析后自动打印 host→bf→endpoint 树形结构 |
| AI 不应该是可选参数 | AI 为默认行为 |
| 配置 key 绑定了 DeepSeek | `deepseek_api_key` → `api_key` |
| 数据库在 CWD | 移到 `~/.config/bizgraph/bizgraph.db` |
| API 配置应走全局 | 统一在 `~/.config/bizgraph/config.toml` |
| AI 上下文限制太小 | TURN_DATA_CHAR_LIMIT 3.2K→200K |

### 设计决策

- **全局配置目录**: `~/.config/bizgraph/`
  - `config.toml` — api_key, model, api_url
  - `bizgraph.db` — 所有项目和分析记录
- **配置 key 通用化**: `api_key` / `model` / `api_url`，不绑定厂商
- **AI 应为默认行为**: 分析完成后默认输出业务结构 + AI 报告
- **完整 API 配置三要素**: api_key + model + api_url，支持任何 OpenAI 兼容端点

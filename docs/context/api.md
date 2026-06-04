# bizgraph API Reference

> Version: 0.1.1 · Stack: Rust 2021 · Deps: clap 4, rusqlite 0.31, reqwest 0.12, tokio 1, serde, uuid, chrono

bizgraph exposes two surfaces: a **CLI binary** (`bizgraph`) and a **library crate** (`bizgraph::*`).

---

## CLI

```
bizgraph <COMMAND>
```

### `analyze`

Parse a `.har` file, build a deterministic business graph, merge into a project, and optionally run AI analysis.

```
bizgraph analyze <HAR_PATH> --project <NAME> [--host <FILTER>]
```

| Argument / Flag | Required | Description |
|---|---|---|
| `HAR_PATH` | yes | Path to `.har` file (HTTP Archive 1.2 JSON) |
| `-p, --project <NAME>` | yes | Project name or ID — auto-creates if not exists |
| `-H, --host <FILTER>` | no | Prefix-match filter on the `Host` column |

**Output (stdout):** project name, import stats (rows, new/updated nodes, new/skipped edges), node/edge totals, business function tree with endpoints and confidence scores, AI report preview (first 500 chars).

AI analysis runs automatically when `api_key` is configured in `~/.config/bizgraph/config.toml`. Supports incremental analysis — only new endpoints are sent to AI.

### `project`

Manage persisted graph projects.

```
bizgraph project <SUBCOMMAND>
```

| Subcommand | Arguments / Flags | Description |
|---|---|---|
| `new` | `<NAME>` | Create a new empty project |
| `list` | — | List all projects (ID, created_at, name) |
| `show` | `<NAME>` | Overview: stats, node counts by kind, graph metrics, business tree, AI report preview |
| `history` | `<NAME>` | Analysis history with timestamps and per-run stats |
| `export` | `<NAME> [-o <PATH>]` | Export full graph as JSON (nodes, edges, business functions, AI report). Default: stdout |
| `viz` | `<NAME> [-o <PATH>]` | Generate interactive HTML visualization (vis-network). Default output: `graph.html` |
| `diff` | `<NAME>` | Compare last two analyses: added/removed business functions and endpoints, stats delta, report section changes |
| `report` | `<NAME>` | Print the full AI analysis report for the latest analysis |
| `delete` | `<NAME> --force` | Delete project and all data. Requires `--force` flag |

---

## Library API

All public functions live in the crate root (`bizgraph::*`).

### Analysis

```rust
/// Analyze a HAR file into a deterministic business graph (no AI, no DB).
pub fn analyze(har_path: &str, host_filter: Option<&str>) -> Result<BusinessGraph>

/// Analyze a HAR file with AI-powered business function identification and deep report.
/// Returns (graph, report_text).
pub async fn analyze_with_ai_report(
    har_path: &str,
    host_filter: Option<&str>,
    api_key: &str,
    model: &str,
    api_url: &str,
    deep: bool,
) -> Result<(BusinessGraph, String)>

/// Full pipeline: parse HAR → AI identification → merge into DB project → AI deep analysis.
/// Auto-creates project if it doesn't exist. Supports incremental analysis.
pub async fn analyze_with_project(
    har_path: &str,
    host_filter: Option<&str>,
    project_name_or_id: &str,
    api_key_option: Option<&str>,
    model_option: Option<&str>,
    api_url_option: Option<&str>,
    ai_report: Option<&str>,
) -> Result<AnalysisResult>
```

### Configuration

```rust
/// Load API config from ~/.config/bizgraph/config.toml. Returns (api_key, model, api_url).
/// Errors if api_key is missing.
pub fn load_config() -> Result<(String, String, String)>

/// Same as load_config but returns None instead of erroring when no API key is set.
pub fn try_load_config() -> Option<(String, String, String)>

/// Shorthand: returns only the API key.
pub fn load_api_key() -> Result<String>
```

### Re-exports

```rust
pub use db::Database;       // Direct database access
pub use error::{Error, Result}; // Error enum and Result alias
```

---

## Data Models

All types are in `bizgraph::types` and derive `Serialize`/`Deserialize`.

### Core Graph Types

#### `BusinessGraph`

```rust
pub struct BusinessGraph {
    pub nodes: Vec<BusinessNode>,
    pub edges: Vec<BusinessEdge>,
}
```

#### `BusinessNode`

```rust
pub struct BusinessNode {
    pub id: Uuid,                              // Derived from stable_key via UUIDv5
    pub stable_key: String,                    // Deterministic: "host:<h>", "bf:<h>:<p>", "ep:<m>:<h>:<p>"
    pub label: String,                         // Human-readable name
    pub kind: BusinessNodeKind,                // Host | BusinessFunction | Endpoint
    pub properties: BusinessNodeProperties,    // Tagged enum with kind-specific fields
}
```

#### `BusinessNodeKind`

```rust
pub enum BusinessNodeKind { Host, BusinessFunction, Endpoint }
```

#### `BusinessNodeProperties`

```rust
pub enum BusinessNodeProperties {
    BusinessFunction(BusinessFunctionProperties),
    Endpoint(EndpointProperties),
    Host(BTreeMap<String, serde_json::Value>),
}
```

#### `BusinessFunctionProperties`

```rust
pub struct BusinessFunctionProperties {
    pub host: String,
    pub path_prefix: String,
    pub endpoint_count: usize,
    pub description: Option<String>,           // AI-generated
}
```

#### `EndpointProperties`

```rust
pub struct EndpointProperties {
    pub path_template: String,                 // e.g. "/api/users/{id}"
    pub methods: Vec<String>,                  // e.g. ["GET", "POST"]
    pub status_codes: Vec<u16>,
    pub request_schema: Option<SchemaShape>,
    pub response_schema: Option<SchemaShape>,
    pub parameters: Vec<ParameterDescriptor>,
    pub status_profiles: StatusProfiles,
    pub confidence: f64,                       // 0.0–1.0, AI confidence
    pub normalization_notes: Vec<String>,
}
```

#### `BusinessEdge`

```rust
pub struct BusinessEdge {
    pub id: Uuid,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub label: String,                         // "contains" | "calls_after" | "data_dependency:*"
    pub properties: serde_json::Value,
}
```

### Schema / Parameter Types

```rust
pub enum SchemaType { Null, Boolean, Integer, Number, String, Array, Object, Unknown }

pub struct SchemaShape {
    pub schema_type: SchemaType,
    pub properties: BTreeMap<String, SchemaShape>,
    pub items: Option<Box<SchemaShape>>,
}

pub enum ParameterLocation { Path, Query, Body }

pub enum ParameterKind {
    DynamicSegment, NumericId, Uuid, Token,
    Integer, String, Boolean, Number, Empty, Unknown,
}

pub struct ParameterDescriptor {
    pub name: String,
    pub location: ParameterLocation,
    pub kind: ParameterKind,
    pub occurrence_count: usize,
}

pub struct StatusProfiles {
    pub success: usize,
    pub redirect: usize,
    pub client_error: usize,
    pub server_error: usize,
    pub other: Vec<String>,
}
```

### Project & Analysis

```rust
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

pub struct AnalysisStats {
    pub row_count: usize,
    pub new_nodes: usize,
    pub updated_nodes: usize,
    pub new_edges: usize,
    pub skipped_edges: usize,
}

pub struct AnalysisRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source_path: Option<String>,
    pub host_filter: Option<String>,
    pub ai_report: Option<String>,
    pub row_count: usize,
    pub new_nodes: usize,
    pub updated_nodes: usize,
    pub new_edges: usize,
    pub skipped_edges: usize,
    pub node_snapshot: Option<String>,         // JSON array of stable_keys for diff
    pub created_at: DateTime<Utc>,
}

pub struct AnalysisResult {
    pub project: Project,
    pub graph: BusinessGraph,
    pub stats: AnalysisStats,
    pub ai_report: Option<String>,
}
```

### Import Types

```rust
pub struct BusinessImportRequest {
    pub nodes: Vec<BusinessImportNode>,
    pub edges: Vec<BusinessImportEdge>,
}

pub struct BusinessImportNode {
    pub stable_key: String,
    pub label: String,
    pub kind: BusinessNodeKind,
    pub properties: BusinessNodeProperties,
}

pub struct BusinessImportEdge {
    pub source_key: String,
    pub target_key: String,
    pub label: String,
    pub properties: serde_json::Value,
}

pub struct BusinessImportResult {
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub edges_created: usize,
    pub edges_skipped: usize,
    pub errors: Vec<String>,
}
```

### Error

```rust
pub type Result<T> = std::result::Result<T, Error>;

pub enum Error {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Uuid(uuid::Error),
    Chrono(chrono::ParseError),
    Toml(toml::de::Error),
    // Contextual variants — carry a message + source
    IoContext { context: String, source: std::io::Error },
    SqliteContext { context: String, source: rusqlite::Error },
    JsonContext { context: String, source: serde_json::Error },
    TomlContext { context: String, source: toml::de::Error },
    ApiRequest { context: String, source: reqwest::Error },
    ApiResponse { status: StatusCode, body: String, url: String },
    ApiResponseDecode { context: String, source: reqwest::Error },
    // Domain errors
    MissingNode { kind: &'static str, key: String },
    ProjectNotFound { reference: String },
    ProjectAlreadyExists { name: String },
    AmbiguousProject { reference: String, matches: Vec<String> },
    EmptyProjectName,
    EmptyProjectReference,
    BudgetExceeded { scope: String, used: usize, limit: usize },
    TaskPanicked { task: String, details: String },
    ConfigMissingApiKey,
    ConfigRead { path: PathBuf, source: std::io::Error },
    ConfigParse { path: PathBuf, source: toml::de::Error },
    Validation { message: String },
    InvalidNodeKind { value: String },
    InvalidUuidValue { value: String, source: uuid::Error },
    InvalidTimestampValue { value: String, source: chrono::ParseError },
}
```

### Database (re-exported)

```rust
pub struct Database { /* private */ }

impl Database {
    pub fn open_default() -> Result<Self>;              // ~/.config/bizgraph/bizgraph.db
    pub fn open(path: PathBuf) -> Result<Self>;
    pub fn create_project(&self, name: &str) -> Result<Project>;
    pub fn list_projects(&self) -> Result<Vec<Project>>;
    pub fn get_project(&self, id: Uuid) -> Result<Project>;
    pub fn get_project_by_name(&self, name: &str) -> Result<Option<Project>>;
    pub fn resolve_project(&self, name_or_id: &str) -> Result<Option<Project>>;
    pub fn delete_project(&self, project_id: Uuid) -> Result<()>;
    pub fn merge_graph(&self, project_id: Uuid, graph: &BusinessGraph) -> Result<AnalysisStats>;
    pub fn get_graph(&self, project_id: Uuid) -> Result<BusinessGraph>;
    pub fn get_endpoint_keys(&self, project_id: Uuid) -> Result<HashSet<String>>;
    pub fn get_business_function_summary(&self, project_id: Uuid) -> Result<Vec<(String, Option<String>, usize)>>;
    pub fn record_analysis(&self, project_id: Uuid, source_path: Option<&str>, host_filter: Option<&str>, ai_report: Option<&str>, row_count: usize, stats: &AnalysisStats, node_snapshot: Option<&str>) -> Result<AnalysisRecord>;
    pub fn get_latest_analysis(&self, project_id: Uuid) -> Result<Option<AnalysisRecord>>;
    pub fn get_analysis_history(&self, project_id: Uuid) -> Result<Vec<AnalysisRecord>>;
    pub fn clear_business_functions(&self, project_id: Uuid) -> Result<usize>;
    pub fn upsert_node(&self, project_id: Uuid, node: &BusinessNode) -> Result<bool>;
    pub fn upsert_edge(&self, project_id: Uuid, edge: &BusinessEdge) -> Result<bool>;
}
```

---

## Constants

| Constant | Value | Location |
|---|---|---|
| `STABLE_ID_NAMESPACE` | `Uuid(0x8cb0f43215db53178f8d2f899edf4620)` | `types.rs:8` |
| `DEFAULT_MODEL` | `"deepseek-v4-flash"` | `lib.rs:26` |
| `DEFAULT_API_URL` | `"https://api.deepseek.com/chat/completions"` | `lib.rs:27` |

## Config

Config path: `~/.config/bizgraph/config.toml`

```toml
api_key = "sk-..."             # required for AI features
model = "deepseek-v4-flash"    # optional
api_url = "https://..."        # optional, any OpenAI-format endpoint
```

Database path: `~/.config/bizgraph/bizgraph.db` (SQLite, WAL mode).

## Stable Key Formats

| Node Kind | Pattern | Example |
|---|---|---|
| Host | `host:<normalized-host>` | `host:api.example.com` |
| Business Function | `bf:<host>:<path-prefix>` | `bf:api.example.com:/users` |
| Endpoint | `ep:<method>:<host>:<path-template>` | `ep:GET:api.example.com:/users/{id}` |

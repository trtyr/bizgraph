use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Deterministic namespace used to derive stable UUIDv5 identifiers from a stable key.
pub const STABLE_ID_NAMESPACE: Uuid = Uuid::from_u128(0x8cb0f43215db53178f8d2f899edf4620);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
/// Classification of a node in the business graph.
pub enum BusinessNodeKind {
    /// A domain/hostname (e.g., `api.example.com`).
    Host,
    /// A logical business function grouping related endpoints (e.g., "User Management").
    BusinessFunction,
    /// An individual API endpoint (e.g., `GET /api/v1/users/{id}`).
    Endpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaType {
    Null,
    Boolean,
    Integer,
    Number,
    String,
    Array,
    Object,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaShape {
    pub schema_type: SchemaType,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, SchemaShape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaShape>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterLocation {
    Path,
    Query,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    DynamicSegment,
    NumericId,
    Uuid,
    Token,
    Integer,
    String,
    Boolean,
    Number,
    Empty,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDescriptor {
    pub name: String,
    pub location: ParameterLocation,
    pub kind: ParameterKind,
    pub occurrence_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusProfiles {
    pub success: usize,
    pub redirect: usize,
    pub client_error: usize,
    pub server_error: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointProperties {
    pub path_template: String,
    pub methods: Vec<String>,
    pub status_codes: Vec<u16>,
    pub request_schema: Option<SchemaShape>,
    pub response_schema: Option<SchemaShape>,
    pub parameters: Vec<ParameterDescriptor>,
    pub status_profiles: StatusProfiles,
    pub confidence: f64,
    pub normalization_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessFunctionProperties {
    pub host: String,
    pub path_prefix: String,
    pub endpoint_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum BusinessNodeProperties {
    BusinessFunction(BusinessFunctionProperties),
    Endpoint(EndpointProperties),
    Host(BTreeMap<String, serde_json::Value>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A node in the business graph — one of Host, BusinessFunction, or Endpoint.
///
/// `id` is deterministically derived from `stable_key` via UUIDv5.
pub struct BusinessNode {
    pub id: Uuid,
    /// Deterministic key (e.g., `ep:GET:api.example.com:/users/{id}`). Same input always produces the same key.
    pub stable_key: String,
    pub label: String,
    pub kind: BusinessNodeKind,
    pub properties: BusinessNodeProperties,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A directed edge in the business graph.
///
/// Labels: `contains` (host→bf, bf→endpoint), `calls_after` (sequential flow),
/// `data_dependency:*` (shared data between requests).
pub struct BusinessEdge {
    pub id: Uuid,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub label: String,
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
/// The core output of bizgraph — a deterministic directed graph of business structure.
///
/// Nodes and edges are always sorted by `stable_key` for reproducible serialization.
pub struct BusinessGraph {
    pub nodes: Vec<BusinessNode>,
    pub edges: Vec<BusinessEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BusinessImportRequest {
    pub nodes: Vec<BusinessImportNode>,
    pub edges: Vec<BusinessImportEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessImportNode {
    pub stable_key: String,
    pub label: String,
    pub kind: BusinessNodeKind,
    pub properties: BusinessNodeProperties,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessImportEdge {
    pub source_key: String,
    pub target_key: String,
    pub label: String,
    #[serde(default = "default_properties")]
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BusinessImportResult {
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub edges_created: usize,
    pub edges_skipped: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

pub fn default_properties() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

/// Derive a deterministic UUIDv5 from a stable key.
///
/// Same `stable_key` always produces the same `Uuid` — no randomness.
pub fn deterministic_id(stable_key: &str) -> Uuid {
    Uuid::new_v5(&STABLE_ID_NAMESPACE, stable_key.as_bytes())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A named project that groups multiple analysis runs.
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisStats {
    pub row_count: usize,
    pub new_nodes: usize,
    pub updated_nodes: usize,
    pub new_edges: usize,
    pub skipped_edges: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A single analysis run recorded against a project.
///
/// Contains the AI report, traffic stats, and a node snapshot for diff comparison.
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
    /// JSON array of node stable_keys at analysis time, for diff comparison
    pub node_snapshot: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Complete result of `analyze_with_project` — project info, graph, stats, and optional AI report.
pub struct AnalysisResult {
    pub project: Project,
    pub graph: BusinessGraph,
    pub stats: AnalysisStats,
    pub ai_report: Option<String>,
}

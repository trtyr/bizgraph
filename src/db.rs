use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::types::{
    AnalysisRecord, AnalysisStats, BusinessEdge, BusinessGraph, BusinessNode, BusinessNodeKind,
    BusinessNodeProperties, EndpointProperties, ParameterDescriptor, ParameterKind, Project,
};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open_default() -> Result<Self, String> {
        Self::open(PathBuf::from("bizgraph.db"))
    }

    pub fn open(path: PathBuf) -> Result<Self, String> {
        let conn =
            Connection::open(path).map_err(|e| format!("failed to open bizgraph database: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS business_nodes (
                 id TEXT NOT NULL,
                 project_id TEXT NOT NULL REFERENCES projects(id),
                 stable_key TEXT NOT NULL,
                 label TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 properties TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 PRIMARY KEY(project_id, stable_key)
             );
             CREATE TABLE IF NOT EXISTS business_edges (
                 id TEXT NOT NULL,
                 project_id TEXT NOT NULL REFERENCES projects(id),
                 source_node_id TEXT NOT NULL,
                 target_node_id TEXT NOT NULL,
                 label TEXT NOT NULL,
                 properties TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 PRIMARY KEY(project_id, source_node_id, target_node_id, label)
             );
             CREATE TABLE IF NOT EXISTS analyses (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id),
                 excel_path TEXT,
                 host_filter TEXT,
                 row_count INTEGER NOT NULL DEFAULT 0,
                  new_nodes INTEGER NOT NULL DEFAULT 0,
                  updated_nodes INTEGER NOT NULL DEFAULT 0,
                  new_edges INTEGER NOT NULL DEFAULT 0,
                  skipped_edges INTEGER NOT NULL DEFAULT 0,
                  ai_report TEXT,
                  created_at TEXT NOT NULL
              );",
        )
        .map_err(|e| format!("failed to initialize bizgraph database: {e}"))?;

        ensure_analysis_ai_report_column(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn create_project(&self, name: &str) -> Result<Project, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("project name cannot be empty".to_string());
        }

        let now = Utc::now();
        let project = Project {
            id: Uuid::new_v4(),
            name: name.to_string(),
            created_at: now,
        };

        self.c()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
                params![
                    project.id.to_string(),
                    project.name,
                    project.created_at.to_rfc3339(),
                ],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    format!("project '{}' already exists", name)
                } else {
                    format!("failed to create project '{}': {e}", name)
                }
            })?;

        Ok(project)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, String> {
        let conn = self.c();
        let mut stmt = conn
            .prepare("SELECT id, name, created_at FROM projects ORDER BY created_at ASC, name ASC")
            .map_err(|e| format!("failed to prepare project list query: {e}"))?;
        let rows = stmt
            .query_map([], project_from_row)
            .map_err(|e| format!("failed to list projects: {e}"))?;

        let mut projects = Vec::new();
        for row in rows {
            projects.push(row.map_err(|e| format!("failed to decode project row: {e}"))?);
        }
        Ok(projects)
    }

    pub fn get_project_by_name(&self, name: &str) -> Result<Option<Project>, String> {
        self.c()
            .query_row(
                "SELECT id, name, created_at FROM projects WHERE name = ?1",
                params![name],
                project_from_row,
            )
            .optional()
            .map_err(|e| format!("failed to fetch project '{}': {e}", name))
    }

    pub fn get_project(&self, id: Uuid) -> Result<Project, String> {
        self.c()
            .query_row(
                "SELECT id, name, created_at FROM projects WHERE id = ?1",
                params![id.to_string()],
                project_from_row,
            )
            .optional()
            .map_err(|e| format!("failed to fetch project '{id}': {e}"))?
            .ok_or_else(|| format!("project '{id}' not found"))
    }

    pub fn resolve_project(&self, name_or_id: &str) -> Result<Option<Project>, String> {
        let value = name_or_id.trim();
        if value.is_empty() {
            return Err("project name or id cannot be empty".to_string());
        }

        if let Some(project) = self.get_project_by_name(value)? {
            return Ok(Some(project));
        }

        if let Ok(id) = Uuid::parse_str(value) {
            return self.get_project(id).map(Some).or_else(|error| {
                if error.ends_with("not found") {
                    Ok(None)
                } else {
                    Err(error)
                }
            });
        }

        let projects = self.list_projects()?;
        let matches: Vec<Project> = projects
            .into_iter()
            .filter(|project| {
                project.name.starts_with(value) || project.id.to_string().starts_with(value)
            })
            .collect();

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(format!(
                "project reference '{}' is ambiguous: {}",
                value,
                matches
                    .iter()
                    .map(|project| project.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub fn upsert_node(&self, project_id: Uuid, node: &BusinessNode) -> Result<bool, String> {
        let existing = self
            .c()
            .query_row(
                "SELECT properties FROM business_nodes WHERE project_id = ?1 AND stable_key = ?2",
                params![project_id.to_string(), node.stable_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("failed to query business node '{}': {e}", node.stable_key))?;

        let now = Utc::now().to_rfc3339();
        if let Some(existing_properties) = existing {
            let existing_properties: BusinessNodeProperties =
                serde_json::from_str(&existing_properties).map_err(|e| {
                    format!(
                        "failed to parse stored node properties for '{}': {e}",
                        node.stable_key
                    )
                })?;
            let merged_properties =
                merge_node_properties(existing_properties, node.properties.clone());

            self.c()
                .execute(
                    "UPDATE business_nodes
                     SET label = ?1, kind = ?2, properties = ?3, updated_at = ?4
                     WHERE project_id = ?5 AND stable_key = ?6",
                    params![
                        node.label,
                        node_kind_as_str(&node.kind),
                        serde_json::to_string(&merged_properties).map_err(|e| format!(
                            "failed to serialize merged node properties: {e}"
                        ))?,
                        now,
                        project_id.to_string(),
                        node.stable_key,
                    ],
                )
                .map_err(|e| format!("failed to update node '{}': {e}", node.stable_key))?;

            Ok(false)
        } else {
            self.c()
                .execute(
                    "INSERT INTO business_nodes
                     (id, project_id, stable_key, label, kind, properties, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        node.id.to_string(),
                        project_id.to_string(),
                        node.stable_key,
                        node.label,
                        node_kind_as_str(&node.kind),
                        serde_json::to_string(&node.properties)
                            .map_err(|e| format!("failed to serialize node properties: {e}"))?,
                        now,
                        now,
                    ],
                )
                .map_err(|e| format!("failed to insert node '{}': {e}", node.stable_key))?;

            Ok(true)
        }
    }

    pub fn upsert_edge(&self, project_id: Uuid, edge: &BusinessEdge) -> Result<bool, String> {
        let exists = self
            .c()
            .query_row(
                "SELECT 1 FROM business_edges WHERE project_id = ?1 AND source_node_id = ?2 AND target_node_id = ?3 AND label = ?4",
                params![
                    project_id.to_string(),
                    edge.source_node_id.to_string(),
                    edge.target_node_id.to_string(),
                    edge.label,
                ],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| format!("failed to query business edge '{}': {e}", edge.id))?
            .unwrap_or(false);

        if exists {
            return Ok(false);
        }

        self.c()
            .execute(
                "INSERT INTO business_edges
                 (id, project_id, source_node_id, target_node_id, label, properties, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    edge.id.to_string(),
                    project_id.to_string(),
                    edge.source_node_id.to_string(),
                    edge.target_node_id.to_string(),
                    edge.label,
                    serde_json::to_string(&edge.properties)
                        .map_err(|e| format!("failed to serialize edge properties: {e}"))?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|e| format!("failed to insert edge '{}': {e}", edge.id))?;

        Ok(true)
    }

    pub fn merge_graph(
        &self,
        project_id: Uuid,
        graph: &BusinessGraph,
    ) -> Result<AnalysisStats, String> {
        let mut stats = AnalysisStats::default();

        for node in &graph.nodes {
            if self.upsert_node(project_id, node)? {
                stats.new_nodes += 1;
            } else {
                stats.updated_nodes += 1;
            }
        }

        for edge in &graph.edges {
            if self.upsert_edge(project_id, edge)? {
                stats.new_edges += 1;
            } else {
                stats.skipped_edges += 1;
            }
        }

        self.refresh_business_function_counts(project_id)?;
        Ok(stats)
    }

    pub fn get_graph(&self, project_id: Uuid) -> Result<BusinessGraph, String> {
        let nodes = {
            let conn = self.c();
            let mut stmt = conn
                .prepare(
                    "SELECT id, stable_key, label, kind, properties
                     FROM business_nodes
                     WHERE project_id = ?1
                     ORDER BY stable_key ASC",
                )
                .map_err(|e| format!("failed to prepare node query: {e}"))?;
            let rows = stmt
                .query_map(params![project_id.to_string()], business_node_from_row)
                .map_err(|e| format!("failed to query project graph nodes: {e}"))?;

            let mut nodes = Vec::new();
            for row in rows {
                nodes.push(row.map_err(|e| format!("failed to decode stored node: {e}"))?);
            }
            nodes
        };

        let edges = {
            let conn = self.c();
            let mut stmt = conn
                .prepare(
                    "SELECT id, source_node_id, target_node_id, label, properties
                     FROM business_edges
                     WHERE project_id = ?1
                     ORDER BY label ASC, source_node_id ASC, target_node_id ASC",
                )
                .map_err(|e| format!("failed to prepare edge query: {e}"))?;
            let rows = stmt
                .query_map(params![project_id.to_string()], business_edge_from_row)
                .map_err(|e| format!("failed to query project graph edges: {e}"))?;

            let mut edges = Vec::new();
            for row in rows {
                edges.push(row.map_err(|e| format!("failed to decode stored edge: {e}"))?);
            }
            edges
        };

        Ok(BusinessGraph { nodes, edges })
    }

    pub fn record_analysis(
        &self,
        project_id: Uuid,
        excel_path: Option<&str>,
        host_filter: Option<&str>,
        ai_report: Option<&str>,
        row_count: usize,
        stats: &AnalysisStats,
    ) -> Result<AnalysisRecord, String> {
        let record = AnalysisRecord {
            id: Uuid::new_v4(),
            project_id,
            excel_path: excel_path.map(ToString::to_string),
            host_filter: host_filter.map(ToString::to_string),
            ai_report: ai_report.map(ToString::to_string),
            row_count,
            new_nodes: stats.new_nodes,
            updated_nodes: stats.updated_nodes,
            new_edges: stats.new_edges,
            skipped_edges: stats.skipped_edges,
            created_at: Utc::now(),
        };

        self.c()
            .execute(
                "INSERT INTO analyses
                 (id, project_id, excel_path, host_filter, row_count, new_nodes, updated_nodes, new_edges, skipped_edges, ai_report, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    record.id.to_string(),
                    record.project_id.to_string(),
                    record.excel_path.as_deref(),
                    record.host_filter.as_deref(),
                    record.row_count as i64,
                    record.new_nodes as i64,
                    record.updated_nodes as i64,
                    record.new_edges as i64,
                    record.skipped_edges as i64,
                    record.ai_report.as_deref(),
                    record.created_at.to_rfc3339(),
                ],
            )
            .map_err(|e| format!("failed to record analysis: {e}"))?;

        Ok(record)
    }

    pub fn get_latest_analysis(&self, project_id: Uuid) -> Result<Option<AnalysisRecord>, String> {
        self.c()
            .query_row(
                "SELECT id, project_id, excel_path, host_filter, ai_report, row_count, new_nodes, updated_nodes, new_edges, skipped_edges, created_at
                 FROM analyses
                 WHERE project_id = ?1
                 ORDER BY created_at DESC
                 LIMIT 1",
                params![project_id.to_string()],
                analysis_record_from_row,
            )
            .optional()
            .map_err(|e| format!("failed to fetch latest analysis: {e}"))
    }

    pub fn get_analysis_history(&self, project_id: Uuid) -> Result<Vec<AnalysisRecord>, String> {
        let conn = self.c();
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, excel_path, host_filter, ai_report, row_count, new_nodes, updated_nodes, new_edges, skipped_edges, created_at
                 FROM analyses
                 WHERE project_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("failed to prepare analysis history query: {e}"))?;
        let rows = stmt
            .query_map(params![project_id.to_string()], analysis_record_from_row)
            .map_err(|e| format!("failed to query analysis history: {e}"))?;

        let mut history = Vec::new();
        for row in rows {
            history.push(row.map_err(|e| format!("failed to decode analysis record: {e}"))?);
        }
        Ok(history)
    }

    fn refresh_business_function_counts(&self, project_id: Uuid) -> Result<(), String> {
        let counts = {
            let conn = self.c();
            let mut stmt = conn
                .prepare(
                    "SELECT n.id, COUNT(e.target_node_id)
                     FROM business_nodes n
                     LEFT JOIN business_edges e
                       ON e.project_id = n.project_id
                      AND e.source_node_id = n.id
                      AND e.label = 'contains'
                     WHERE n.project_id = ?1 AND n.kind = 'business_function'
                     GROUP BY n.id",
                )
                .map_err(|e| format!("failed to prepare business function recount query: {e}"))?;
            let rows = stmt
                .query_map(params![project_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| format!("failed to query business function counts: {e}"))?;

            let mut counts = Vec::new();
            for row in rows {
                counts.push(
                    row.map_err(|e| format!("failed to decode business function count: {e}"))?,
                );
            }
            counts
        };

        for (node_id, endpoint_count) in counts {
            let properties: String = self
                .c()
                .query_row(
                    "SELECT properties FROM business_nodes WHERE id = ?1 AND project_id = ?2",
                    params![node_id, project_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("failed to fetch business function properties: {e}"))?;

            let mut properties: BusinessNodeProperties = serde_json::from_str(&properties)
                .map_err(|e| format!("failed to parse business function properties: {e}"))?;

            if let BusinessNodeProperties::BusinessFunction(ref mut details) = properties {
                details.endpoint_count = endpoint_count.max(0) as usize;
                self.c()
                    .execute(
                        "UPDATE business_nodes SET properties = ?1, updated_at = ?2 WHERE id = ?3 AND project_id = ?4",
                        params![
                            serde_json::to_string(&properties).map_err(|e| {
                                format!("failed to serialize refreshed business function properties: {e}")
                            })?,
                            Utc::now().to_rfc3339(),
                            node_id,
                            project_id.to_string(),
                        ],
                    )
                    .map_err(|e| format!("failed to update refreshed business function counts: {e}"))?;
            }
        }

        Ok(())
    }

    fn c(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: parse_uuid_field(row.get::<_, String>(0)?).map_err(to_sql_conversion_error)?,
        name: row.get(1)?,
        created_at: parse_datetime_field(row.get::<_, String>(2)?)
            .map_err(to_sql_conversion_error)?,
    })
}

fn business_node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BusinessNode> {
    Ok(BusinessNode {
        id: parse_uuid_field(row.get::<_, String>(0)?).map_err(to_sql_conversion_error)?,
        stable_key: row.get(1)?,
        label: row.get(2)?,
        kind: parse_node_kind(&row.get::<_, String>(3)?).map_err(to_sql_conversion_error)?,
        properties: serde_json::from_str(&row.get::<_, String>(4)?)
            .map_err(to_sql_conversion_error)?,
    })
}

fn business_edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BusinessEdge> {
    Ok(BusinessEdge {
        id: parse_uuid_field(row.get::<_, String>(0)?).map_err(to_sql_conversion_error)?,
        source_node_id: parse_uuid_field(row.get::<_, String>(1)?)
            .map_err(to_sql_conversion_error)?,
        target_node_id: parse_uuid_field(row.get::<_, String>(2)?)
            .map_err(to_sql_conversion_error)?,
        label: row.get(3)?,
        properties: serde_json::from_str(&row.get::<_, String>(4)?)
            .map_err(to_sql_conversion_error)?,
    })
}

fn analysis_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AnalysisRecord> {
    Ok(AnalysisRecord {
        id: parse_uuid_field(row.get::<_, String>(0)?).map_err(to_sql_conversion_error)?,
        project_id: parse_uuid_field(row.get::<_, String>(1)?).map_err(to_sql_conversion_error)?,
        excel_path: row.get(2)?,
        host_filter: row.get(3)?,
        ai_report: row.get(4)?,
        row_count: row.get::<_, i64>(5)?.max(0) as usize,
        new_nodes: row.get::<_, i64>(6)?.max(0) as usize,
        updated_nodes: row.get::<_, i64>(7)?.max(0) as usize,
        new_edges: row.get::<_, i64>(8)?.max(0) as usize,
        skipped_edges: row.get::<_, i64>(9)?.max(0) as usize,
        created_at: parse_datetime_field(row.get::<_, String>(10)?)
            .map_err(to_sql_conversion_error)?,
    })
}

fn ensure_analysis_ai_report_column(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(analyses)")
        .map_err(|e| format!("failed to inspect analyses schema: {e}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("failed to read analyses schema: {e}"))?;

    let mut has_ai_report = false;
    for column in columns {
        if column.map_err(|e| format!("failed to decode analyses schema row: {e}"))? == "ai_report"
        {
            has_ai_report = true;
            break;
        }
    }

    if !has_ai_report {
        conn.execute("ALTER TABLE analyses ADD COLUMN ai_report TEXT", [])
            .map_err(|e| format!("failed to migrate analyses.ai_report: {e}"))?;
    }

    Ok(())
}

fn parse_uuid_field(value: String) -> Result<Uuid, String> {
    Uuid::parse_str(&value).map_err(|e| format!("invalid uuid '{value}': {e}"))
}

fn parse_datetime_field(value: String) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(&value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|e| format!("invalid timestamp '{value}': {e}"))
}

fn to_sql_conversion_error(error: impl ToString) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

fn parse_node_kind(value: &str) -> Result<BusinessNodeKind, String> {
    match value {
        "host" => Ok(BusinessNodeKind::Host),
        "business_function" => Ok(BusinessNodeKind::BusinessFunction),
        "endpoint" => Ok(BusinessNodeKind::Endpoint),
        _ => Err(format!("unknown business node kind '{value}'")),
    }
}

fn node_kind_as_str(kind: &BusinessNodeKind) -> &'static str {
    match kind {
        BusinessNodeKind::Host => "host",
        BusinessNodeKind::BusinessFunction => "business_function",
        BusinessNodeKind::Endpoint => "endpoint",
    }
}

fn merge_node_properties(
    existing: BusinessNodeProperties,
    incoming: BusinessNodeProperties,
) -> BusinessNodeProperties {
    match (existing, incoming) {
        (
            BusinessNodeProperties::Endpoint(existing),
            BusinessNodeProperties::Endpoint(incoming),
        ) => BusinessNodeProperties::Endpoint(merge_endpoint_properties(existing, incoming)),
        (
            BusinessNodeProperties::BusinessFunction(mut existing),
            BusinessNodeProperties::BusinessFunction(incoming),
        ) => {
            existing.host = incoming.host;
            existing.path_prefix = incoming.path_prefix;
            existing.endpoint_count = existing.endpoint_count.max(incoming.endpoint_count);
            BusinessNodeProperties::BusinessFunction(existing)
        }
        (BusinessNodeProperties::Host(mut existing), BusinessNodeProperties::Host(incoming)) => {
            for (key, value) in incoming {
                existing.insert(key, value);
            }
            BusinessNodeProperties::Host(existing)
        }
        (_, incoming) => incoming,
    }
}

fn merge_endpoint_properties(
    existing: EndpointProperties,
    incoming: EndpointProperties,
) -> EndpointProperties {
    let methods = merge_sorted_unique(existing.methods, incoming.methods);
    let status_codes = merge_sorted_unique(existing.status_codes, incoming.status_codes);
    let mut other_status = existing.status_profiles.other;
    other_status.extend(incoming.status_profiles.other);
    other_status.sort();
    other_status.dedup();

    let parameters = merge_parameters(existing.parameters, incoming.parameters);
    let normalization_notes =
        merge_sorted_unique(existing.normalization_notes, incoming.normalization_notes);

    let mut merged = EndpointProperties {
        path_template: if incoming.path_template.is_empty() {
            existing.path_template
        } else {
            incoming.path_template
        },
        methods,
        status_codes,
        request_schema: existing.request_schema.or(incoming.request_schema),
        response_schema: existing.response_schema.or(incoming.response_schema),
        parameters,
        status_profiles: crate::types::StatusProfiles {
            success: existing.status_profiles.success + incoming.status_profiles.success,
            redirect: existing.status_profiles.redirect + incoming.status_profiles.redirect,
            client_error: existing.status_profiles.client_error
                + incoming.status_profiles.client_error,
            server_error: existing.status_profiles.server_error
                + incoming.status_profiles.server_error,
            other: other_status,
        },
        confidence: 0.0,
        normalization_notes,
    };

    merged.confidence = recalculate_endpoint_confidence(&merged);
    merged
}

fn merge_parameters(
    existing: Vec<ParameterDescriptor>,
    incoming: Vec<ParameterDescriptor>,
) -> Vec<ParameterDescriptor> {
    let mut merged: HashMap<String, ParameterDescriptor> = HashMap::new();

    for parameter in existing.into_iter().chain(incoming) {
        let key = format!("{:?}:{}", parameter.location, parameter.name);
        merged
            .entry(key)
            .and_modify(|current| {
                current.occurrence_count += parameter.occurrence_count;
                current.kind = prefer_parameter_kind(&current.kind, &parameter.kind);
            })
            .or_insert(parameter);
    }

    let mut parameters: Vec<ParameterDescriptor> = merged.into_values().collect();
    parameters.sort_by(|left, right| {
        format!("{:?}", left.location)
            .cmp(&format!("{:?}", right.location))
            .then_with(|| left.name.cmp(&right.name))
    });
    parameters
}

fn prefer_parameter_kind(current: &ParameterKind, candidate: &ParameterKind) -> ParameterKind {
    if parameter_kind_rank(candidate) >= parameter_kind_rank(current) {
        candidate.clone()
    } else {
        current.clone()
    }
}

fn parameter_kind_rank(kind: &ParameterKind) -> usize {
    match kind {
        ParameterKind::Uuid => 7,
        ParameterKind::Token => 6,
        ParameterKind::NumericId => 5,
        ParameterKind::Integer => 4,
        ParameterKind::Boolean => 3,
        ParameterKind::Number => 2,
        ParameterKind::String => 1,
        ParameterKind::DynamicSegment | ParameterKind::Empty | ParameterKind::Unknown => 0,
    }
}

fn recalculate_endpoint_confidence(properties: &EndpointProperties) -> f64 {
    let method_score = (properties.methods.len().min(3) as f64 / 3.0) * 0.25;
    let request_schema_score = if properties.request_schema.is_some() {
        0.2
    } else {
        0.0
    };
    let response_schema_score = if properties.response_schema.is_some() {
        0.2
    } else {
        0.0
    };
    let parameter_score = if properties.parameters.is_empty() {
        0.0
    } else {
        0.15
    };
    let status_score = if properties.status_codes.is_empty() {
        0.0
    } else {
        0.1
    };
    let notes_score = if properties.normalization_notes.is_empty() {
        0.0
    } else {
        0.1
    };

    (method_score
        + request_schema_score
        + response_schema_score
        + parameter_score
        + status_score
        + notes_score)
        .clamp(0.0, 1.0)
}

fn merge_sorted_unique<T: Ord>(existing: Vec<T>, incoming: Vec<T>) -> Vec<T> {
    let mut values: BTreeSet<T> = existing.into_iter().collect();
    values.extend(incoming);
    values.into_iter().collect()
}

fn _merge_host_properties(
    existing: BTreeMap<String, Value>,
    incoming: BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let mut merged = existing;
    for (key, value) in incoming {
        merged.insert(key, value);
    }
    merged
}

fn _normalize_json_object(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let ordered: Map<String, Value> = object.into_iter().collect();
            Value::Object(ordered)
        }
        other => other,
    }
}

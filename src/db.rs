use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{Error, Result};
use crate::types::{
    AnalysisRecord, AnalysisStats, BusinessEdge, BusinessGraph,
    BusinessNode, BusinessNodeKind, BusinessNodeProperties, EndpointProperties,
    ParameterDescriptor, ParameterKind, Project,
};

/// Return `~/.config/bizgraph/`, creating the directory if it does not exist.
fn global_config_dir() -> Result<PathBuf> {
    let dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/bizgraph");
    std::fs::create_dir_all(&dir)
        .map_err(|source| Error::io(format!("failed to create config dir {}", dir.display()), source))?;
    Ok(dir)
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open_default() -> Result<Self> {
        let path = global_config_dir()?.join("bizgraph.db");
        Self::open(path)
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|source| Error::sqlite("failed to open bizgraph database", source))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
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
        .map_err(|source| Error::sqlite("failed to initialize bizgraph database", source))?;

        ensure_analysis_ai_report_column(&conn)?;
        ensure_analysis_node_snapshot_column(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn create_project(&self, name: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::EmptyProjectName);
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
            .map_err(|source| match &source {
                rusqlite::Error::SqliteFailure(error, _)
                    if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
                {
                    Error::ProjectAlreadyExists {
                        name: name.to_string(),
                    }
                }
                _ => Error::sqlite(format!("failed to create project '{name}'"), source),
            })?;

        Ok(project)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let conn = self.c();
        let mut stmt = conn
            .prepare("SELECT id, name, created_at FROM projects ORDER BY created_at ASC, name ASC")
            .map_err(|source| Error::sqlite("failed to prepare project list query", source))?;
        let rows = stmt
            .query_map([], project_from_row)
            .map_err(|source| Error::sqlite("failed to list projects", source))?;

        let mut projects = Vec::new();
        for row in rows {
            projects.push(
                row.map_err(|source| Error::sqlite("failed to decode project row", source))?,
            );
        }
        Ok(projects)
    }

    pub fn get_project_by_name(&self, name: &str) -> Result<Option<Project>> {
        self.c()
            .query_row(
                "SELECT id, name, created_at FROM projects WHERE name = ?1",
                params![name],
                project_from_row,
            )
            .optional()
            .map_err(|source| Error::sqlite(format!("failed to fetch project '{name}'"), source))
    }

    pub fn get_project(&self, id: Uuid) -> Result<Project> {
        self.c()
            .query_row(
                "SELECT id, name, created_at FROM projects WHERE id = ?1",
                params![id.to_string()],
                project_from_row,
            )
            .optional()
            .map_err(|source| Error::sqlite(format!("failed to fetch project '{id}'"), source))?
            .ok_or_else(|| Error::ProjectNotFound {
                reference: id.to_string(),
            })
    }

    pub fn resolve_project(&self, name_or_id: &str) -> Result<Option<Project>> {
        let value = name_or_id.trim();
        if value.is_empty() {
            return Err(Error::EmptyProjectReference);
        }

        if let Some(project) = self.get_project_by_name(value)? {
            return Ok(Some(project));
        }

        if let Ok(id) = Uuid::parse_str(value) {
            return self.get_project(id).map(Some).or_else(|error| {
                match error {
                    Error::ProjectNotFound { .. } => Ok(None),
                    other => Err(other),
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
            _ => Err(Error::AmbiguousProject {
                reference: value.to_string(),
                matches: matches.iter().map(|project| project.name.clone()).collect(),
            }),
        }
    }

    pub fn upsert_node(&self, project_id: Uuid, node: &BusinessNode) -> Result<bool> {
        let existing = self
            .c()
            .query_row(
                "SELECT properties FROM business_nodes WHERE project_id = ?1 AND stable_key = ?2",
                params![project_id.to_string(), node.stable_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| {
                Error::sqlite(
                    format!("failed to query business node '{}'", node.stable_key),
                    source,
                )
            })?;

        let now = Utc::now().to_rfc3339();
        if let Some(existing_properties) = existing {
            let existing_properties: BusinessNodeProperties = serde_json::from_str(&existing_properties)
                .map_err(|source| {
                    Error::json(
                        format!("failed to parse stored node properties for '{}'", node.stable_key),
                        source,
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
                        serde_json::to_string(&merged_properties).map_err(|source| {
                            Error::json("failed to serialize merged node properties", source)
                        })?,
                        now,
                        project_id.to_string(),
                        node.stable_key,
                    ],
                )
                .map_err(|source| {
                    Error::sqlite(format!("failed to update node '{}'", node.stable_key), source)
                })?;

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
                            .map_err(|source| Error::json("failed to serialize node properties", source))?,
                        now,
                        now,
                    ],
                )
                .map_err(|source| {
                    Error::sqlite(format!("failed to insert node '{}'", node.stable_key), source)
                })?;

            Ok(true)
        }
    }

    pub fn upsert_edge(&self, project_id: Uuid, edge: &BusinessEdge) -> Result<bool> {
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
            .map_err(|source| {
                Error::sqlite(format!("failed to query business edge '{}'", edge.id), source)
            })?
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
                        .map_err(|source| Error::json("failed to serialize edge properties", source))?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|source| {
                Error::sqlite(format!("failed to insert edge '{}'", edge.id), source)
            })?;

        Ok(true)
    }

    pub fn merge_graph(
        &self,
        project_id: Uuid,
        graph: &BusinessGraph,
    ) -> Result<AnalysisStats> {
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

    /// Delete all business_function nodes and their edges for a project.
    /// Used when AI identification replaces URL-based grouping.
    pub fn clear_business_functions(&self, project_id: Uuid) -> Result<usize> {
        let conn = self.c();
        // Delete edges that reference business_function nodes
        conn.execute(
            "DELETE FROM business_edges WHERE project_id = ?1 AND (
                source_node_id IN (SELECT id FROM business_nodes WHERE project_id = ?1 AND kind = 'business_function')
                OR target_node_id IN (SELECT id FROM business_nodes WHERE project_id = ?1 AND kind = 'business_function')
            )",
            params![project_id.to_string()],
        )
        .map_err(|source| Error::sqlite("failed to delete business function edges", source))?;

        // Delete the business_function nodes
        let deleted = conn
            .execute(
                "DELETE FROM business_nodes WHERE project_id = ?1 AND kind = 'business_function'",
                params![project_id.to_string()],
            )
            .map_err(|source| Error::sqlite("failed to delete business function nodes", source))?;

        Ok(deleted)
    }

    pub fn get_graph(&self, project_id: Uuid) -> Result<BusinessGraph> {
        let nodes = {
            let conn = self.c();
            let mut stmt = conn
                .prepare(
                    "SELECT id, stable_key, label, kind, properties
                     FROM business_nodes
                     WHERE project_id = ?1
                     ORDER BY stable_key ASC",
                )
                .map_err(|source| Error::sqlite("failed to prepare node query", source))?;
            let rows = stmt
                .query_map(params![project_id.to_string()], business_node_from_row)
                .map_err(|source| Error::sqlite("failed to query project graph nodes", source))?;

            let mut nodes = Vec::new();
            for row in rows {
                nodes.push(
                    row.map_err(|source| Error::sqlite("failed to decode stored node", source))?,
                );
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
                .map_err(|source| Error::sqlite("failed to prepare edge query", source))?;
            let rows = stmt
                .query_map(params![project_id.to_string()], business_edge_from_row)
                .map_err(|source| Error::sqlite("failed to query project graph edges", source))?;

            let mut edges = Vec::new();
            for row in rows {
                edges.push(
                    row.map_err(|source| Error::sqlite("failed to decode stored edge", source))?,
                );
            }
            edges
        };

        Ok(BusinessGraph { nodes, edges })
    }

    /// Get stable_key set of all endpoint nodes for a project.
    /// Used for incremental analysis to detect new endpoints.
    pub fn get_endpoint_keys(&self, project_id: Uuid) -> Result<HashSet<String>> {
        let conn = self.c();
        let mut stmt = conn
            .prepare(
                "SELECT stable_key FROM business_nodes
                 WHERE project_id = ?1 AND kind = 'endpoint'",
            )
            .map_err(|source| Error::sqlite("failed to prepare endpoint key query", source))?;
        let rows = stmt
            .query_map(params![project_id.to_string()], |row| row.get::<_, String>(0))
            .map_err(|source| Error::sqlite("failed to query endpoint keys", source))?;

        let mut keys = HashSet::new();
        for row in rows {
            keys.insert(row.map_err(|source| Error::sqlite("failed to read endpoint key", source))?);
        }
        Ok(keys)
    }

    /// Get a summary of existing business functions for a project.
    /// Returns (bf_label, description, endpoint_count) sorted by label.
    pub fn get_business_function_summary(&self, project_id: Uuid) -> Result<Vec<(String, Option<String>, usize)>> {
        let graph = self.get_graph(project_id)?;
        let mut bfs: BTreeMap<String, (Option<String>, usize)> = BTreeMap::new();

        for node in &graph.nodes {
            if node.kind == BusinessNodeKind::BusinessFunction {
                if let BusinessNodeProperties::BusinessFunction(props) = &node.properties {
                    bfs.insert(node.label.clone(), (props.description.clone(), props.endpoint_count));
                }
            }
        }

        Ok(bfs.into_iter().map(|(label, (desc, count))| (label, desc, count)).collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_analysis(
        &self,
        project_id: Uuid,
        source_path: Option<&str>,
        host_filter: Option<&str>,
        ai_report: Option<&str>,
        row_count: usize,
        stats: &AnalysisStats,
        node_snapshot: Option<&str>,
    ) -> Result<AnalysisRecord> {
        let record = AnalysisRecord {
            id: Uuid::new_v4(),
            project_id,
            source_path: source_path.map(ToString::to_string),
            host_filter: host_filter.map(ToString::to_string),
            ai_report: ai_report.map(ToString::to_string),
            row_count,
            new_nodes: stats.new_nodes,
            updated_nodes: stats.updated_nodes,
            new_edges: stats.new_edges,
            skipped_edges: stats.skipped_edges,
            node_snapshot: node_snapshot.map(ToString::to_string),
            created_at: Utc::now(),
        };

        self.c()
            .execute(
                "INSERT INTO analyses
                 (id, project_id, excel_path, host_filter, row_count, new_nodes, updated_nodes, new_edges, skipped_edges, ai_report, node_snapshot, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    record.id.to_string(),
                    record.project_id.to_string(),
                    record.source_path.as_deref(),
                    record.host_filter.as_deref(),
                    record.row_count as i64,
                    record.new_nodes as i64,
                    record.updated_nodes as i64,
                    record.new_edges as i64,
                    record.skipped_edges as i64,
                    record.ai_report.as_deref(),
                    record.node_snapshot.as_deref(),
                    record.created_at.to_rfc3339(),
                ],
            )
            .map_err(|source| Error::sqlite("failed to record analysis", source))?;

        Ok(record)
    }

    pub fn get_latest_analysis(&self, project_id: Uuid) -> Result<Option<AnalysisRecord>> {
        self.c()
            .query_row(
                "SELECT id, project_id, excel_path, host_filter, ai_report, row_count, new_nodes, updated_nodes, new_edges, skipped_edges, node_snapshot, created_at
                 FROM analyses
                 WHERE project_id = ?1
                 ORDER BY created_at DESC
                 LIMIT 1",
                params![project_id.to_string()],
                analysis_record_from_row,
            )
            .optional()
            .map_err(|source| Error::sqlite("failed to fetch latest analysis", source))
    }

    pub fn get_analysis_history(&self, project_id: Uuid) -> Result<Vec<AnalysisRecord>> {
        let conn = self.c();
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, excel_path, host_filter, ai_report, row_count, new_nodes, updated_nodes, new_edges, skipped_edges, node_snapshot, created_at
                 FROM analyses
                 WHERE project_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|source| Error::sqlite("failed to prepare analysis history query", source))?;
        let rows = stmt
            .query_map(params![project_id.to_string()], analysis_record_from_row)
            .map_err(|source| Error::sqlite("failed to query analysis history", source))?;

        let mut history = Vec::new();
        for row in rows {
            history.push(
                row.map_err(|source| Error::sqlite("failed to decode analysis record", source))?,
            );
        }
        Ok(history)
    }

    /// Delete a project and all its associated data (nodes, edges, analyses).
    pub fn delete_project(&self, project_id: Uuid) -> Result<()> {
        let conn = self.c();
        let id_str = project_id.to_string();
        conn.execute("DELETE FROM business_edges WHERE project_id = ?1", params![id_str])
            .map_err(|source| Error::sqlite("failed to delete project edges", source))?;
        conn.execute("DELETE FROM business_nodes WHERE project_id = ?1", params![id_str])
            .map_err(|source| Error::sqlite("failed to delete project nodes", source))?;
        conn.execute("DELETE FROM analyses WHERE project_id = ?1", params![id_str])
            .map_err(|source| Error::sqlite("failed to delete project analyses", source))?;
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id_str])
            .map_err(|source| Error::sqlite("failed to delete project", source))?;
        Ok(())
    }

    fn refresh_business_function_counts(&self, project_id: Uuid) -> Result<()> {
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
                .map_err(|source| {
                    Error::sqlite("failed to prepare business function recount query", source)
                })?;
            let rows = stmt
                .query_map(params![project_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|source| Error::sqlite("failed to query business function counts", source))?;

            let mut counts = Vec::new();
            for row in rows {
                counts.push(
                    row.map_err(|source| {
                        Error::sqlite("failed to decode business function count", source)
                    })?,
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
                .map_err(|source| {
                    Error::sqlite("failed to fetch business function properties", source)
                })?;

            let mut properties: BusinessNodeProperties = serde_json::from_str(&properties)
                .map_err(|source| {
                    Error::json("failed to parse business function properties", source)
                })?;

            if let BusinessNodeProperties::BusinessFunction(ref mut details) = properties {
                details.endpoint_count = endpoint_count.max(0) as usize;
                self.c()
                    .execute(
                        "UPDATE business_nodes SET properties = ?1, updated_at = ?2 WHERE id = ?3 AND project_id = ?4",
                        params![
                            serde_json::to_string(&properties).map_err(|source| {
                                Error::json(
                                    "failed to serialize refreshed business function properties",
                                    source,
                                )
                            })?,
                            Utc::now().to_rfc3339(),
                            node_id,
                            project_id.to_string(),
                        ],
                    )
                    .map_err(|source| {
                        Error::sqlite(
                            "failed to update refreshed business function counts",
                            source,
                        )
                    })?;
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
        source_path: row.get(2)?,
        host_filter: row.get(3)?,
        ai_report: row.get(4)?,
        row_count: row.get::<_, i64>(5)?.max(0) as usize,
        new_nodes: row.get::<_, i64>(6)?.max(0) as usize,
        updated_nodes: row.get::<_, i64>(7)?.max(0) as usize,
        new_edges: row.get::<_, i64>(8)?.max(0) as usize,
        skipped_edges: row.get::<_, i64>(9)?.max(0) as usize,
        node_snapshot: row.get(10)?,
        created_at: parse_datetime_field(row.get::<_, String>(11)?)
            .map_err(to_sql_conversion_error)?,
    })
}

fn ensure_analysis_ai_report_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(analyses)")
        .map_err(|source| Error::sqlite("failed to inspect analyses schema", source))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|source| Error::sqlite("failed to read analyses schema", source))?;

    let mut has_ai_report = false;
    for column in columns {
        if column.map_err(|source| Error::sqlite("failed to decode analyses schema row", source))?
            == "ai_report"
        {
            has_ai_report = true;
            break;
        }
    }

    if !has_ai_report {
        conn.execute("ALTER TABLE analyses ADD COLUMN ai_report TEXT", [])
            .map_err(|source| Error::sqlite("failed to migrate analyses.ai_report", source))?;
    }

    Ok(())
}

fn ensure_analysis_node_snapshot_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(analyses)")
        .map_err(|source| Error::sqlite("failed to inspect analyses schema", source))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|source| Error::sqlite("failed to read analyses schema", source))?;

    let mut has_column = false;
    for column in columns {
        if column.map_err(|source| Error::sqlite("failed to decode analyses schema row", source))?
            == "node_snapshot"
        {
            has_column = true;
            break;
        }
    }

    if !has_column {
        conn.execute("ALTER TABLE analyses ADD COLUMN node_snapshot TEXT", [])
            .map_err(|source| Error::sqlite("failed to migrate analyses.node_snapshot", source))?;
    }

    Ok(())
}

fn parse_uuid_field(value: String) -> Result<Uuid> {
    Uuid::parse_str(&value).map_err(|source| Error::InvalidUuidValue { value, source })
}

fn parse_datetime_field(value: String) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|source| Error::InvalidTimestampValue { value, source })
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

fn parse_node_kind(value: &str) -> Result<BusinessNodeKind> {
    match value {
        "host" => Ok(BusinessNodeKind::Host),
        "business_function" => Ok(BusinessNodeKind::BusinessFunction),
        "endpoint" => Ok(BusinessNodeKind::Endpoint),
        _ => Err(Error::InvalidNodeKind {
            value: value.to_string(),
        }),
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
            if incoming.description.is_some() {
                existing.description = incoming.description;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn in_memory_db() -> Database {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
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
        .unwrap();
        super::ensure_analysis_ai_report_column(&conn).unwrap();
        super::ensure_analysis_node_snapshot_column(&conn).unwrap();
        Database {
            conn: Mutex::new(conn),
        }
    }

    #[test]
    fn create_and_list_projects() {
        let db = in_memory_db();
        let p = db.create_project("test-proj").unwrap();
        assert_eq!(p.name, "test-proj");

        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "test-proj");
    }

    #[test]
    fn duplicate_project_name_rejected() {
        let db = in_memory_db();
        db.create_project("dup").unwrap();
        let err = db.create_project("dup").unwrap_err();
        assert!(matches!(err, Error::ProjectAlreadyExists { .. }));
    }

    #[test]
    fn get_project_by_name_and_id() {
        let db = in_memory_db();
        let p = db.create_project("lookup").unwrap();

        let by_name = db.get_project_by_name("lookup").unwrap().unwrap();
        assert_eq!(by_name.id, p.id);

        let by_id = db.get_project(p.id).unwrap();
        assert_eq!(by_id.name, "lookup");
    }

    #[test]
    fn resolve_project_by_name() {
        let db = in_memory_db();
        let p = db.create_project("resolve-me").unwrap();
        let resolved = db.resolve_project("resolve-me").unwrap().unwrap();
        assert_eq!(resolved.id, p.id);
    }

    #[test]
    fn resolve_project_not_found() {
        let db = in_memory_db();
        let result = db.resolve_project("nope").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn upsert_node_and_get_graph() {
        let db = in_memory_db();
        let p = db.create_project("graph-test").unwrap();

        let node = BusinessNode {
            id: Uuid::new_v4(),
            stable_key: "host:example.com".to_string(),
            label: "example.com".to_string(),
            kind: BusinessNodeKind::Host,
            properties: BusinessNodeProperties::Host(BTreeMap::new()),
        };
        let changed = db.upsert_node(p.id, &node).unwrap();
        assert!(changed, "first insert should report changed");

        let changed = db.upsert_node(p.id, &node).unwrap();
        assert!(!changed, "identical re-insert should not report changed");

        let graph = db.get_graph(p.id).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].stable_key, "host:example.com");
    }

    #[test]
    fn upsert_edge_and_get_graph() {
        let db = in_memory_db();
        let p = db.create_project("edge-test").unwrap();

        let n1 = BusinessNode {
            id: Uuid::new_v4(),
            stable_key: "host:a.com".to_string(),
            label: "a.com".to_string(),
            kind: BusinessNodeKind::Host,
            properties: BusinessNodeProperties::Host(BTreeMap::new()),
        };
        let n2 = BusinessNode {
            id: Uuid::new_v4(),
            stable_key: "bf:a.com:/api".to_string(),
            label: "/api".to_string(),
            kind: BusinessNodeKind::BusinessFunction,
            properties: BusinessNodeProperties::BusinessFunction(BusinessFunctionProperties {
                host: "a.com".to_string(),
                path_prefix: "/api".to_string(),
                endpoint_count: 0,
                description: None,
            }),
        };
        db.upsert_node(p.id, &n1).unwrap();
        db.upsert_node(p.id, &n2).unwrap();

        let edge = BusinessEdge {
            id: Uuid::new_v4(),
            source_node_id: n1.id,
            target_node_id: n2.id,
            label: "contains".to_string(),
            properties: serde_json::Value::Object(serde_json::Map::new()),
        };
        let changed = db.upsert_edge(p.id, &edge).unwrap();
        assert!(changed);

        let graph = db.get_graph(p.id).unwrap();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].label, "contains");
    }

    #[test]
    fn merge_graph_creates_nodes_and_edges() {
        let db = in_memory_db();
        let p = db.create_project("merge-test").unwrap();

        let graph = BusinessGraph {
            nodes: vec![
                BusinessNode {
                    id: Uuid::new_v4(),
                    stable_key: "host:x.com".to_string(),
                    label: "x.com".to_string(),
                    kind: BusinessNodeKind::Host,
                    properties: BusinessNodeProperties::Host(BTreeMap::new()),
                },
                BusinessNode {
                    id: Uuid::new_v4(),
                    stable_key: "ep:GET:x.com:/users".to_string(),
                    label: "GET /users".to_string(),
                    kind: BusinessNodeKind::Endpoint,
                    properties: BusinessNodeProperties::Endpoint(EndpointProperties {
                            path_template: "/users".to_string(),
                            methods: vec!["GET".to_string()],
                            status_codes: vec![200],
                            request_schema: None,
                            response_schema: None,
                            parameters: vec![],
                            status_profiles: StatusProfiles::default(),
                            confidence: 1.0,
                            normalization_notes: vec![],
                        }),
                },
            ],
            edges: vec![],
        };

        let stats = db.merge_graph(p.id, &graph).unwrap();
        assert_eq!(stats.new_nodes, 2);
        assert_eq!(stats.updated_nodes, 0);

        let saved = db.get_graph(p.id).unwrap();
        assert_eq!(saved.nodes.len(), 2);
    }

    #[test]
    fn record_and_retrieve_analysis() {
        let db = in_memory_db();
        let p = db.create_project("analysis-test").unwrap();

        let stats = AnalysisStats {
            row_count: 10,
            new_nodes: 5,
            updated_nodes: 0,
            new_edges: 4,
            skipped_edges: 0,
        };
        db.record_analysis(
            p.id,
            Some("test.har"),
            None,
            Some("# Report"),
            10,
            &stats,
            Some("[]"),
        )
        .unwrap();

        let latest = db.get_latest_analysis(p.id).unwrap().unwrap();
        assert_eq!(latest.row_count, 10);
        assert_eq!(latest.ai_report.as_deref(), Some("# Report"));

        let history = db.get_analysis_history(p.id).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn delete_project_cascades() {
        let db = in_memory_db();
        let p = db.create_project("delete-me").unwrap();

        let node = BusinessNode {
            id: Uuid::new_v4(),
            stable_key: "host:d.com".to_string(),
            label: "d.com".to_string(),
            kind: BusinessNodeKind::Host,
            properties: BusinessNodeProperties::Host(BTreeMap::new()),
        };
        db.upsert_node(p.id, &node).unwrap();

        db.delete_project(p.id).unwrap();
        assert!(db.get_project(p.id).is_err());
        assert!(db.get_graph(p.id).unwrap().nodes.is_empty());
    }

    #[test]
    fn clear_business_functions() {
        let db = in_memory_db();
        let p = db.create_project("clear-bf").unwrap();

        let host = BusinessNode {
            id: Uuid::new_v4(),
            stable_key: "host:e.com".to_string(),
            label: "e.com".to_string(),
            kind: BusinessNodeKind::Host,
            properties: BusinessNodeProperties::Host(BTreeMap::new()),
        };
        let bf = BusinessNode {
            id: Uuid::new_v4(),
            stable_key: "bf:e.com:/api".to_string(),
            label: "/api".to_string(),
            kind: BusinessNodeKind::BusinessFunction,
            properties: BusinessNodeProperties::BusinessFunction(BusinessFunctionProperties {
                host: "e.com".to_string(),
                path_prefix: "/api".to_string(),
                endpoint_count: 1,
                description: None,
            }),
        };
        db.upsert_node(p.id, &host).unwrap();
        db.upsert_node(p.id, &bf).unwrap();

        db.clear_business_functions(p.id).unwrap();
        let graph = db.get_graph(p.id).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert!(matches!(graph.nodes[0].kind, BusinessNodeKind::Host));
    }
}

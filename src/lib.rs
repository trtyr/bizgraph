pub mod error;
pub mod ai;
pub mod db;
pub mod graph;
pub mod parser;
pub mod types;

use std::{env, fs, path::PathBuf};

use ai::{analyze_with_ai, analyze_with_ai_deep, identify_business_functions};
pub use db::Database;
pub use error::{Error, Result};
use graph::{build_business_graph, build_business_graph_from_ai, is_static_resource, normalize_path_template};
use parser::parse_har;
use types::{AnalysisResult, BusinessGraph};

#[derive(serde::Deserialize)]
struct Config {
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
}

const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_API_URL: &str = "https://api.deepseek.com/chat/completions";

/// Analyze a HAR traffic capture into a deterministic business graph.
pub fn analyze(har_path: &str, host_filter: Option<&str>) -> Result<BusinessGraph> {
    let rows = parse_har(har_path, host_filter)?;
    build_business_graph(&rows)
}

/// Analyze a HAR file and generate an AI-powered business analysis report.
///
/// First identifies business functions via AI, then builds the graph using those
/// groupings (falling back to URL-based grouping on failure). Finally generates
/// either a single-shot or deep multi-phase analysis report.
pub async fn analyze_with_ai_report(
    har_path: &str,
    host_filter: Option<&str>,
    api_key: &str,
    model: &str,
    api_url: &str,
    deep: bool,
) -> Result<(BusinessGraph, String)> {
    let rows = parse_har(har_path, host_filter)?;

    // AI identifies business functions first
    let (graph, business_context) = match identify_business_functions(&rows, api_key, model, api_url).await {
        Ok(identification) => {
            let context = build_business_context(&identification);
            (build_business_graph_from_ai(&rows, &identification)?, Some(context))
        }
        Err(_) => (build_business_graph(&rows)?, None),
    };

    let report = if deep {
        analyze_with_ai_deep(&graph, api_key, model, api_url, business_context.as_deref()).await?
    } else {
        analyze_with_ai(&graph, api_key, model, api_url).await?
    };
    Ok((graph, report))
}

/// Full analysis pipeline with project persistence.
///
/// Parses HAR → builds graph → merges into project DB → runs AI analysis → records history.
/// Supports incremental analysis: only new endpoints are sent to AI on subsequent runs.
pub async fn analyze_with_project(
    har_path: &str,
    host_filter: Option<&str>,
    project_name_or_id: &str,
    api_key_option: Option<&str>,
    model_option: Option<&str>,
    api_url_option: Option<&str>,
    ai_report: Option<&str>,
) -> Result<AnalysisResult> {
    let rows = parse_har(har_path, host_filter)?;

    let db = Database::open_default()?;
    let project = match db.resolve_project(project_name_or_id)? {
        Some(project) => project,
        None => db.create_project(project_name_or_id)?,
    };

    // Incremental analysis: detect new vs existing endpoints
    let existing_keys = db.get_endpoint_keys(project.id)?;
    let is_incremental = !existing_keys.is_empty();

    let new_rows: Vec<&parser::TrafficRow> = rows
        .iter()
        .filter(|row| {
            if is_static_resource(&row.path) {
                return false;
            }
            let normalized = normalize_path_template(&row.path);
            let key = format!("ep:{}:{}:{}", row.method, row.host, normalized);
            !existing_keys.contains(&key)
        })
        .collect();

    if is_incremental {
        eprintln!(
            "  Incremental: {} existing endpoints, {} new endpoints in HAR",
            existing_keys.len(),
            new_rows.len()
        );
        if new_rows.is_empty() {
            eprintln!("  No new endpoints — updating existing data only");
        }
    }

    // Step 1: AI identifies business functions (if API key available and there are new endpoints)
    let (graph, business_context) = if new_rows.is_empty() && is_incremental {
        // No new endpoints — skip AI identification, just rebuild from existing
        (build_business_graph(&rows)?, None)
    } else if let Some(api_key) = api_key_option {
        let model = model_option.unwrap_or(DEFAULT_MODEL);
        let api_url = api_url_option.unwrap_or(DEFAULT_API_URL);
        match identify_business_functions(&rows, api_key, model, api_url).await {
            Ok(identification) => {
                let context = build_business_context(&identification);
                let graph = build_business_graph_from_ai(&rows, &identification)?;
                (graph, Some(context))
            }
            Err(_) => (build_business_graph(&rows)?, None),
        }
    } else {
        (build_business_graph(&rows)?, None)
    };

    // If AI identification succeeded, clear old URL-based business functions
    if business_context.is_some() {
        let cleared = db.clear_business_functions(project.id)?;
        if cleared > 0 {
            eprintln!("  Cleared {} old URL-based business functions", cleared);
        }
    }

    let stats = db.merge_graph(project.id, &graph)?;
    let mut stats = stats;
    stats.row_count = rows.len();
    let graph = db.get_graph(project.id)?;

    // Step 2: AI deep analysis — skip if incremental with no new endpoints
    let ai_report = if new_rows.is_empty() && is_incremental {
        // No new data — reuse latest existing AI report
        db.get_latest_analysis(project.id)?
            .and_then(|r| r.ai_report)
            .or_else(|| ai_report.map(|s| s.to_string()))
    } else {
        match (ai_report, api_key_option) {
            (Some(report), _) => Some(report.to_string()),
            (None, Some(api_key)) => Some(
                analyze_with_ai_deep(
                    &graph,
                    api_key,
                    model_option.unwrap_or(DEFAULT_MODEL),
                    api_url_option.unwrap_or(DEFAULT_API_URL),
                    business_context.as_deref(),
                )
                .await?,
            ),
            (None, None) => None,
        }
    };

    // Build node snapshot for diff comparison
    let snapshot_keys: Vec<&str> = graph.nodes.iter().map(|n| n.stable_key.as_str()).collect();
    let node_snapshot = serde_json::to_string(&snapshot_keys).ok();

    db.record_analysis(
        project.id,
        Some(har_path),
        host_filter,
        ai_report.as_deref(),
        rows.len(),
        &stats,
        node_snapshot.as_deref(),
    )?;

    Ok(AnalysisResult {
        project,
        graph,
        stats,
        ai_report,
    })
}

/// Build a human-readable context string from business identification for the deep analysis agent.
fn build_business_context(identification: &ai::BusinessIdentification) -> String {
    let mut lines = Vec::new();
    for bf in &identification.business_functions {
        lines.push(format!("- {}: {} ({} endpoints)", bf.name, bf.description, bf.endpoints.len()));
        for ep in &bf.endpoints {
            lines.push(format!("    {} {} {}", ep.method, ep.path, ep.host));
        }
    }
    lines.join("\n")
}

/// Load API configuration from `~/.config/bizgraph/config.toml`.
///
/// Also checks `BIZGRAPH_API_KEY` env var as fallback for the API key.
/// Returns `(api_key, model, api_url)`.
pub fn load_config() -> Result<(String, String, String)> {
    let config = read_config_from_path(config_path_in_home())?;

    // Check env var first, then config file
    let api_key = env::var("BIZGRAPH_API_KEY")
        .ok()
        .and_then(|v| normalize_config_value(&v))
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.api_key.as_deref())
                .and_then(normalize_config_value)
        })
        .ok_or(Error::ConfigMissingApiKey)?;

    let model = config
        .as_ref()
        .and_then(|config| config.model.as_deref())
        .and_then(normalize_config_value)
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let api_url = config
        .as_ref()
        .and_then(|config| config.api_url.as_deref())
        .and_then(normalize_config_value)
        .unwrap_or_else(|| DEFAULT_API_URL.to_string());

    Ok((api_key, model, api_url))
}

pub fn load_api_key() -> Result<String> {
    load_config().map(|(api_key, _, _)| api_key)
}

/// Try to load API config. Returns None if no API key is configured (not an error).
pub fn try_load_config() -> Option<(String, String, String)> {
    load_config().ok()
}

fn config_path_in_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".config/bizgraph/config.toml"))
        .unwrap_or_else(|| PathBuf::from(".config/bizgraph/config.toml"))
}

fn read_config_from_path(path: PathBuf) -> Result<Option<Config>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path).map_err(|source| Error::ConfigRead {
        path: path.clone(),
        source,
    })?;
    let config: Config = toml::from_str(&raw).map_err(|source| Error::ConfigParse {
        path: path.clone(),
        source,
    })?;

    Ok(Some(config))
}

fn normalize_config_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

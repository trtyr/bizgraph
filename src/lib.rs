pub mod ai;
pub mod db;
pub mod graph;
pub mod parser;
pub mod server;
pub mod types;

use std::{env, fs, path::PathBuf};

use ai::analyze_with_ai;
pub use db::Database;
use graph::build_business_graph;
use parser::parse_yakit_excel;
use types::{AnalysisResult, BusinessGraph};

#[derive(serde::Deserialize)]
struct Config {
    deepseek_api_key: Option<String>,
}

/// Analyze a Yakit Excel traffic export into a deterministic business graph.
pub fn analyze(yakit_excel_path: &str, host_filter: Option<&str>) -> Result<BusinessGraph, String> {
    let rows = parse_yakit_excel(yakit_excel_path, host_filter)?;
    build_business_graph(&rows)
}

pub async fn analyze_with_ai_report(
    yakit_excel_path: &str,
    host_filter: Option<&str>,
    api_key: &str,
) -> Result<(BusinessGraph, String), String> {
    let graph = analyze(yakit_excel_path, host_filter)?;
    let report = analyze_with_ai(&graph, api_key).await?;
    Ok((graph, report))
}

pub async fn analyze_with_project(
    yakit_excel_path: &str,
    host_filter: Option<&str>,
    project_name_or_id: &str,
    api_key_option: Option<&str>,
) -> Result<AnalysisResult, String> {
    let rows = parse_yakit_excel(yakit_excel_path, host_filter)?;
    let graph = build_business_graph(&rows)?;

    let db = Database::open_default()?;
    let project = match db.resolve_project(project_name_or_id)? {
        Some(project) => project,
        None => db.create_project(project_name_or_id)?,
    };

    let stats = db.merge_graph(project.id, &graph)?;
    let mut stats = stats;
    stats.row_count = rows.len();
    db.record_analysis(
        project.id,
        Some(yakit_excel_path),
        host_filter,
        rows.len(),
        &stats,
    )?;

    let graph = db.get_graph(project.id)?;
    let ai_report = match api_key_option {
        Some(api_key) => Some(analyze_with_ai(&graph, api_key).await?),
        None => None,
    };

    Ok(AnalysisResult {
        project,
        graph,
        stats,
        ai_report,
    })
}

pub fn load_api_key(cli_api_key: Option<&str>) -> Result<String, String> {
    if let Some(api_key) = cli_api_key.filter(|value| !value.trim().is_empty()) {
        return Ok(api_key.to_string());
    }

    if let Ok(api_key) = env::var("BIZGRAPH_DEEPSEEK_API_KEY") {
        if !api_key.trim().is_empty() {
            return Ok(api_key);
        }
    }

    if let Some(api_key) = read_api_key_from_path(config_path_in_home())? {
        return Ok(api_key);
    }

    if let Some(api_key) = read_api_key_from_path(PathBuf::from("bizgraph.toml"))? {
        return Ok(api_key);
    }

    Err(
        "DeepSeek API key not found. Pass --api-key, set BIZGRAPH_DEEPSEEK_API_KEY, or configure deepseek_api_key in ~/.config/bizgraph/config.toml or ./bizgraph.toml".to_string(),
    )
}

fn config_path_in_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".config/bizgraph/config.toml"))
        .unwrap_or_else(|| PathBuf::from(".config/bizgraph/config.toml"))
}

fn read_api_key_from_path(path: PathBuf) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
    let config: Config = toml::from_str(&raw)
        .map_err(|e| format!("Failed to parse config file {}: {e}", path.display()))?;

    Ok(config
        .deepseek_api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

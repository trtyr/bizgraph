pub mod ai;
pub mod db;
pub mod graph;
pub mod parser;
pub mod server;
pub mod types;

use std::{env, fs, path::PathBuf};

use ai::{analyze_with_ai, analyze_with_ai_deep};
pub use db::Database;
use graph::build_business_graph;
use parser::parse_yakit_excel;
use types::{AnalysisResult, BusinessGraph};

#[derive(serde::Deserialize)]
struct Config {
    deepseek_api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
}

const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_API_URL: &str = "https://api.deepseek.com/chat/completions";

/// Analyze a Yakit Excel traffic export into a deterministic business graph.
pub fn analyze(yakit_excel_path: &str, host_filter: Option<&str>) -> Result<BusinessGraph, String> {
    let rows = parse_yakit_excel(yakit_excel_path, host_filter)?;
    build_business_graph(&rows)
}

pub async fn analyze_with_ai_report(
    yakit_excel_path: &str,
    host_filter: Option<&str>,
    api_key: &str,
    model: &str,
    api_url: &str,
    deep: bool,
) -> Result<(BusinessGraph, String), String> {
    let graph = analyze(yakit_excel_path, host_filter)?;
    let report = if deep {
        analyze_with_ai_deep(&graph, api_key, model, api_url).await?
    } else {
        analyze_with_ai(&graph, api_key, model, api_url).await?
    };
    Ok((graph, report))
}

pub async fn analyze_with_project(
    yakit_excel_path: &str,
    host_filter: Option<&str>,
    project_name_or_id: &str,
    api_key_option: Option<&str>,
    model_option: Option<&str>,
    api_url_option: Option<&str>,
    ai_report: Option<&str>,
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
    let graph = db.get_graph(project.id)?;
    let ai_report = match (ai_report, api_key_option) {
        (Some(report), _) => Some(report.to_string()),
        (None, Some(api_key)) => Some(
            analyze_with_ai_deep(
                &graph,
                api_key,
                model_option.unwrap_or(DEFAULT_MODEL),
                api_url_option.unwrap_or(DEFAULT_API_URL),
            )
            .await?,
        ),
        (None, None) => None,
    };

    db.record_analysis(
        project.id,
        Some(yakit_excel_path),
        host_filter,
        ai_report.as_deref(),
        rows.len(),
        &stats,
    )?;

    Ok(AnalysisResult {
        project,
        graph,
        stats,
        ai_report,
    })
}

pub fn load_config() -> Result<(String, String, String), String> {
    let home_config = read_config_from_path(config_path_in_home())?;
    let local_config = read_config_from_path(PathBuf::from("bizgraph.toml"))?;

    let api_key = home_config
        .as_ref()
        .and_then(|config| config.deepseek_api_key.as_deref())
        .and_then(normalize_config_value)
        .or_else(|| {
            local_config
                .as_ref()
                .and_then(|config| config.deepseek_api_key.as_deref())
                .and_then(normalize_config_value)
        })
        .ok_or_else(|| {
            "API key not found. Configure deepseek_api_key in ~/.config/bizgraph/config.toml or ./bizgraph.toml".to_string()
        })?;

    let model = home_config
        .as_ref()
        .and_then(|config| config.model.as_deref())
        .and_then(normalize_config_value)
        .or_else(|| {
            local_config
                .as_ref()
                .and_then(|config| config.model.as_deref())
                .and_then(normalize_config_value)
        })
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let api_url = home_config
        .as_ref()
        .and_then(|config| config.api_url.as_deref())
        .and_then(normalize_config_value)
        .or_else(|| {
            local_config
                .as_ref()
                .and_then(|config| config.api_url.as_deref())
                .and_then(normalize_config_value)
        })
        .unwrap_or_else(|| DEFAULT_API_URL.to_string());

    Ok((api_key, model, api_url))
}

pub fn load_api_key() -> Result<String, String> {
    load_config().map(|(api_key, _, _)| api_key)
}

fn config_path_in_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".config/bizgraph/config.toml"))
        .unwrap_or_else(|| PathBuf::from(".config/bizgraph/config.toml"))
}

fn read_config_from_path(path: PathBuf) -> Result<Option<Config>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
    let config: Config = toml::from_str(&raw)
        .map_err(|e| format!("Failed to parse config file {}: {e}", path.display()))?;

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

pub mod error;
pub mod ai;
pub mod db;
pub mod graph;
pub mod parser;
pub mod types;

use std::{env, fs, path::PathBuf};

use ai::{analyze_with_ai, analyze_with_ai_deep, chat, identify_business_functions, AGENT_STATE_TOKEN_LIMIT};
pub use db::Database;
pub use error::{Error, Result};
use graph::{build_business_graph, build_business_graph_from_ai, is_static_resource, normalize_path_template};
use parser::parse_har;
use types::{AnalysisResult, BusinessGraph, BusinessNodeProperties};

#[derive(serde::Deserialize)]
struct Config {
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
    /// Optional token budget for AI agent state (default: 100000)
    #[serde(default)]
    token_budget: Option<usize>,
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
        analyze_with_ai_deep(&graph, api_key, model, api_url, business_context.as_deref(), load_token_budget()).await?
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

    // Step 2: AI deep analysis — always run if API key is available
    let ai_report = match (ai_report, api_key_option) {
        (Some(report), _) => Some(report.to_string()),
        (None, Some(api_key)) => Some(
            analyze_with_ai_deep(
                &graph,
                api_key,
                model_option.unwrap_or(DEFAULT_MODEL),
                api_url_option.unwrap_or(DEFAULT_API_URL),
                business_context.as_deref(),
                load_token_budget(),
            )
            .await?,
        ),
        (None, None) => None,
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

/// Stop words to filter from questions (Chinese + English)
const STOP_WORDS: &[&str] = &[
    "的", "是", "在", "了", "和", "与", "什么", "哪些", "怎么", "如何",
    "吗", "呢", "吧", "啊", "这个", "那个", "有", "没有", "可以",
    "the", "is", "are", "what", "which", "how", "does", "do", "and", "or", "a", "an",
];

/// Extract meaningful keywords from a question
fn extract_keywords(question: &str) -> Vec<String> {
    question
        .split(|c: char| c.is_whitespace() || c == '?' || c == '？' || c == ',' || c == '，' || c == '。')
        .map(|w| w.trim())
        .filter(|w| w.len() >= 2 && !STOP_WORDS.contains(w))
        .map(|w| w.to_lowercase())
        .collect()
}

/// Check if a question is broad/complex (needs full context) vs specific (needs scoped context)
fn is_complex_question(question: &str) -> bool {
    let q = question.to_lowercase();
    let complex_markers = [
        "流程", "业务", "关系", "依赖", "核心", "概述", "整体",
        "overview", "flow", "relationship", "dependency", "summary", "all",
        "主要", "全部", "整体", "架构", "分析", "完整",
    ];
    complex_markers.iter().any(|m| q.contains(m))
}

/// Build RAG-style context: match question keywords against graph nodes, return scoped or full context
fn build_ask_context(graph: &BusinessGraph, question: &str) -> String {
    let keywords = extract_keywords(question);

    // Always use full context for complex questions
    if keywords.is_empty() || is_complex_question(question) {
        return ai::summarization::build_graph_summary(graph);
    }

    // Match keywords against endpoint nodes
    let mut matched_ids = std::collections::BTreeSet::new();
    for node in &graph.nodes {
        if let BusinessNodeProperties::Endpoint(details) = &node.properties {
            let haystack = format!(
                "{} {} {}",
                node.label.to_lowercase(),
                details.path_template.to_lowercase(),
                details.normalization_notes.join(" ").to_lowercase()
            );
            if keywords.iter().any(|kw| haystack.contains(kw)) {
                matched_ids.insert(node.id);
            }
        }
    }

    // Walk edges to find parent functions/hosts
    for edge in &graph.edges {
        if edge.label == "contains" && matched_ids.contains(&edge.target_node_id) {
            matched_ids.insert(edge.source_node_id);
        }
    }

    if matched_ids.is_empty() {
        // No matches — fall back to full context
        return ai::summarization::build_graph_summary(graph);
    }

    // Build scoped summary from matched nodes
    let mut output = String::from("Matched endpoints for your question:\n");
    output.push_str("| Label | Path | Methods | Params | Confidence |\n");
    output.push_str("|-------|------|---------|--------|------------|\n");

    let mut count = 0;
    for node in &graph.nodes {
        if !matched_ids.contains(&node.id) {
            continue;
        }
        match &node.properties {
            BusinessNodeProperties::Endpoint(details) => {
                let methods = details.methods.join(", ");
                let params = ai::summarization::summarize_parameters(&details.parameters);
                output.push_str(&format!(
                    "| {} | {} | {} | {} | {:.2} |\n",
                    node.label, details.path_template, methods, params, details.confidence
                ));
                count += 1;
            }
            BusinessNodeProperties::BusinessFunction(details) => {
                output.push_str(&format!(
                    "\nBusiness function: {} (host={}, prefix={})\n",
                    node.label, details.host, details.path_prefix
                ));
            }
            BusinessNodeProperties::Host(_) => {
                output.push_str(&format!("\nHost: {}\n", node.label));
            }
        }
    }

    output.push_str(&format!("\n({} endpoints matched)\n", count));
    output
}
/// Conversational Q&A: ask a question about a project's traffic data with full context.
/// Maintains conversation history in SQLite for multi-turn dialogue.
pub async fn ask(project_name_or_id: &str, question: &str) -> Result<String> {
    let config = load_config()?;
    let (api_key, model, api_url) = config;
    let db = db::Database::open_default()?;
    let project = db.resolve_project(project_name_or_id)?
        .ok_or_else(|| Error::ProjectNotFound { reference: project_name_or_id.to_string() })?;

    let pid = project.id.to_string();
    let graph = db.get_graph(project.id)?;

    // RAG-style context retrieval
    let graph_context = build_ask_context(&graph, question);
    let complex = is_complex_question(question);

    // Get and compress conversation history
    let mut history = db.get_conversation_history(&pid)?;
    history = ai::compress_history(history, 8);

    // Build messages
    let mut messages = Vec::new();
    let system_prompt = format!(
        "<identity>\n{}\n</identity>\n\n<reference_data>\nProject: {}\n\n{}\n</reference_data>\n\n<task>\nAnswer the user\'s question concisely and precisely. Reference specific endpoint paths, methods, and parameters.\nIf the information is not available, say so clearly.\n</task>",
        ai::AGENT_IDENTITY_PROMPT,
        project.name,
        graph_context
    );
    messages.push(chat::ChatMessage::system(system_prompt));
    messages.extend(history);

    let question_msg = question.to_string();
    messages.push(chat::ChatMessage::user(question_msg.clone()));
    db.add_conversation_message(&pid, "user", &question_msg)?;

    // Send to AI
    let response = chat::chat_fresh(messages, &api_key, &model, &api_url, None).await?;

    // Self-correction: retry with full context if answer is too short for complex questions
    let final_response = if complex && response.len() < 200 {
        eprintln!("  ↻ Answer too short for complex question, retrying with full context...");
        let full_context = ai::summarization::build_graph_summary(&graph);
        let retry_prompt = format!(
            "<identity>\n{}\n</identity>\n\n<reference_data>\nProject: {}\n\n{}\n</reference_data>\n\n<task>\nThe user asked: {}\nYour previous answer was too brief. Provide a comprehensive answer with full details.\n</task>",
            ai::AGENT_IDENTITY_PROMPT,
            project.name,
            full_context,
            question
        );
        let mut retry_msgs = vec![chat::ChatMessage::system(retry_prompt)];
        retry_msgs.push(chat::ChatMessage::user(question_msg.clone()));
        match chat::chat_fresh(retry_msgs, &api_key, &model, &api_url, None).await {
            Ok(retry_resp) if retry_resp.len() > response.len() => retry_resp,
            _ => response,
        }
    } else {
        response
    };

    db.add_conversation_message(&pid, "assistant", &final_response)?;
    Ok(final_response)
}

/// Clear conversation history for a project
pub fn clear_conversation(project_name_or_id: &str) -> Result<()> {
    let db = db::Database::open_default()?;
    let project = db.resolve_project(project_name_or_id)?
        .ok_or_else(|| Error::ProjectNotFound { reference: project_name_or_id.to_string() })?;
    db.clear_conversation(&project.id.to_string())?;
    Ok(())
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
/// Returns `(api_key, model, api_url)`.
pub fn load_config() -> Result<(String, String, String)> {
    let config = read_config_from_path(config_path_in_home())?;

    let api_key = config
        .as_ref()
        .and_then(|config| config.api_key.as_deref())
        .and_then(normalize_config_value)
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

/// Load token budget from config. Returns default (100000) if not configured.
pub fn load_token_budget() -> usize {
    read_config_from_path(config_path_in_home())
        .ok()
        .flatten()
        .and_then(|c| c.token_budget)
        .unwrap_or(AGENT_STATE_TOKEN_LIMIT)
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

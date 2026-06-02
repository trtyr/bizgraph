use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};
use crate::types::{BusinessGraph, BusinessNodeProperties};

use super::chat::{chat_fresh, send_chat_request, ChatMessage, ChatRequest};
use super::prompts::*;
use super::summarization::{
    build_cross_domain_summary, build_function_detail, build_graph_overview,
    extract_cross_cutting_items, parse_observations_from_response, prioritized_function_names,
    summarize_text,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub phase: String,
    pub domains: Vec<DomainAnalysis>,
    pub observations: Vec<BusinessObservation>,
    pub cross_cutting: Vec<String>,
    pub progress: Progress,
    pub token_budget: TokenBudget,
    pub turn_responses: Vec<TurnResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResponse {
    pub phase: String,
    pub domain: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainAnalysis {
    pub name: String,
    pub priority: u8,
    pub endpoint_count: usize,
    pub analyzed: bool,
    pub observations_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessObservation {
    pub title: String,
    pub evidence: String,
    pub endpoints: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub completed: Vec<String>,
    pub remaining: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    pub used: usize,
    pub limit: usize,
}

impl AgentState {
    pub fn new(graph: &BusinessGraph) -> Self {
        let mut domains: Vec<_> = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.properties {
                BusinessNodeProperties::BusinessFunction(details) => {
                    Some((node.label.clone(), details.endpoint_count))
                }
                _ => None,
            })
            .collect();

        domains.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

        let domain_entries: Vec<_> = domains
            .into_iter()
            .enumerate()
            .map(|(index, (name, endpoint_count))| DomainAnalysis {
                name,
                priority: (index + 1).min(u8::MAX as usize) as u8,
                endpoint_count,
                analyzed: false,
                observations_summary: None,
            })
            .collect();

        let progress = Progress {
            completed: Vec::new(),
            remaining: domain_entries
                .iter()
                .map(|domain| domain.name.clone())
                .collect(),
        };

        let mut state = Self {
            phase: "overview".to_string(),
            domains: domain_entries,
            observations: Vec::new(),
            cross_cutting: Vec::new(),
            progress,
            token_budget: TokenBudget {
                used: 0,
                limit: AGENT_STATE_TOKEN_LIMIT,
            },
            turn_responses: Vec::new(),
        };
        refresh_token_budget(&mut state);
        state
    }
}

pub async fn run_agent(
    graph: &BusinessGraph,
    api_key: &str,
    model: &str,
    api_url: &str,
    business_context: Option<&str>,
) -> Result<String> {
    let mut state = AgentState::new(graph);
    let mut calls_made = 0usize;

    // Build context preamble with business identification if available
    let context_preamble = business_context
        .map(|ctx| format!("BUSINESS CONTEXT (pre-identified business functions):\n{ctx}\n\n"))
        .unwrap_or_default();

    eprintln!("Phase 1/3: Business overview...");
    let overview_input = format!("{context_preamble}{}", build_graph_overview(graph));
    let overview_context = build_turn_context(&state, "OVERVIEW", &overview_input);
    let overview = chat_fresh(overview_context, api_key, model, api_url).await?;
    calls_made += 1;
    record_turn(&mut state, "overview", None, &overview);
    update_state_from_overview(graph, &mut state, &overview);
    parse_observations_into_state(&mut state, None, &overview)?;

    state.phase = "domain".to_string();
    let domain_budget = MAX_DEEP_AI_CALLS.saturating_sub(3);
    let selected_domains: Vec<_> = state.domains.iter().take(domain_budget).cloned().collect();

    eprintln!(
        "Phase 2/3: Deep-diving {} prioritized domains in parallel...",
        selected_domains.len()
    );
    let mut domain_tasks = Vec::new();
    for domain in &selected_domains {
        let name = domain.name.clone();
        let detail = build_function_detail(&name, graph);
        let context = build_turn_context(&state, "DOMAIN_DEEP_DIVE", &detail);
        let api_key = api_key.to_string();
        let model = model.to_string();
        let api_url = api_url.to_string();
        domain_tasks.push(tokio::spawn(async move {
            eprintln!("  > Analyzing domain: {name}");
            let response = chat_fresh(context, &api_key, &model, &api_url).await?;
            Ok::<_, Error>((name, response))
        }));
    }

    let mut domain_failures = 0usize;
    for (i, task) in domain_tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok((domain, response))) => {
                calls_made += 1;
                record_turn(&mut state, "domain", Some(&domain), &response);
                if let Err(error) = parse_observations_into_state(&mut state, Some(&domain), &response) {
                    eprintln!("  Warning: failed to parse observations for {domain}: {error}");
                }
                eprintln!(
                    "  Done [{}/{}] {}",
                    i + 1,
                    selected_domains.len(),
                    domain
                );
            }
            Ok(Err(error)) => {
                domain_failures += 1;
                eprintln!("  Failed [{}/{}]: {error}", i + 1, selected_domains.len());
                if domain_failures >= MAX_DOMAIN_FAILURES {
                    eprintln!("  Too many domain failures ({domain_failures}), stopping domain analysis.");
                    break;
                }
            }
            Err(error) => {
                domain_failures += 1;
                eprintln!("  Panicked [{}/{}]: {error}", i + 1, selected_domains.len());
                if domain_failures >= MAX_DOMAIN_FAILURES {
                    eprintln!("  Too many domain failures ({domain_failures}), stopping domain analysis.");
                    break;
                }
            }
        }
    }

    state.phase = "cross_final".to_string();
    eprintln!("Phase 3/3: Cross-domain correlation & final report...");
    force_summarize_context(&mut state, api_key, model, api_url).await?;
    let cross_final_data = format!(
        "{}\n\n---\n\n{}",
        build_cross_summary(&state, graph),
        render_state_for_final(&state)
    );
    let cross_final_context = build_turn_context(&state, "CROSS_FINAL", &cross_final_data);
    let report = chat_fresh(cross_final_context, api_key, model, api_url).await?;
    calls_made += 1;

    if calls_made > MAX_DEEP_AI_CALLS {
        return Err(Error::BudgetExceeded {
            scope: "deep AI analysis API call budget".to_string(),
            used: calls_made,
            limit: MAX_DEEP_AI_CALLS,
        });
    }

    Ok(extract_final_report(&report))
}

fn build_turn_context(state: &AgentState, task: &str, data: &str) -> Vec<ChatMessage> {
    let accumulated = state
        .turn_responses
        .iter()
        .map(|response| {
            format!(
                "## {} ({}) Results\n\n{}",
                response.phase,
                response.domain.as_deref().unwrap_or("overview"),
                response.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    let context_block = if accumulated.is_empty() {
        String::new()
    } else {
        format!("Previous analysis results:\n\n{}\n\n---\n\n", accumulated)
    };
    let char_count = data.chars().count();
    let data = if char_count > TURN_DATA_CHAR_LIMIT {
        eprintln!(
            "Warning: task {task} context payload is {char_count} chars, above limit {TURN_DATA_CHAR_LIMIT}; truncating."
        );
        let truncated: String = data.chars().take(TURN_DATA_CHAR_LIMIT).collect();
        format!("{truncated}\n\n[truncated — {char_count} chars total, showing first {TURN_DATA_CHAR_LIMIT}]")
    } else {
        data.to_string()
    };

    vec![
        ChatMessage::system(AGENT_IDENTITY_PROMPT),
        ChatMessage::user(format!(
            "{context_block}Task: {task}\n\n{}\n\nData:\n{data}",
            build_task_instruction(task)
        )),
    ]
}

fn build_task_instruction(task: &str) -> &'static str {
    match task {
        "OVERVIEW" => {
            "Analyze the graph overview to identify business domains.\n\n\
             Required output:\n\
             1. List each business domain with its purpose (1-2 sentences)\n\
             2. Rank domains by business importance (not just endpoint count)\n\
             3. Identify cross-cutting concerns (auth, logging, shared services)\n\
             4. Note any patterns in the endpoint structure\n\n\
             Return Markdown with clear ## headings per domain."
        }
        "DOMAIN_DEEP_DIVE" => {
            "Deep-dive into a single business domain.\n\n\
             Required output:\n\
             1. Domain purpose — what business function does it serve?\n\
             2. Endpoint catalog — for each endpoint:\n\
                - What it does (action + data)\n\
                - Input parameters and their meaning\n\
                - Response structure and key fields\n\
                - Role in the user workflow\n\
             3. Data flow — what data enters, transforms, and exits this domain\n\
             4. Internal relationships — how endpoints within this domain connect\n\n\
             Cite specific endpoint paths, methods, and parameters as evidence."
        }
        "CROSS_FINAL" => {
            "Two-part task:\n\n\
             Part 1 — Cross-domain correlation:\n\
             - How do business functions connect? What data flows between modules?\n\
             - What are the key user journeys that span multiple domains?\n\
             - Are there shared data entities across domains?\n\n\
             Part 2 — Final report:\n\
             - Executive summary (3-5 sentences)\n\
             - Business functions overview (grouped by domain)\n\
             - User flow analysis (key journeys)\n\
             - Data flow map (how data moves through the system)\n\
             - Endpoint purpose catalog (each endpoint's role)\n\n\
             Return a polished Markdown report with all sections populated."
        }
        "FINAL_REPORT" => {
            "Compile a comprehensive business understanding report.\n\n\
             Required sections:\n\
             1. Executive Summary — what does this application do?\n\
             2. Business Functions — grouped by domain, with endpoint details\n\
             3. User Flow Analysis — key user journeys through the system\n\
             4. Data Flow Map — how data moves between modules\n\
             5. Endpoint Purpose Catalog — each endpoint's role and data\n\n\
             Return polished Markdown."
        }
        _ => "Analyze the supplied business-graph context and respond in natural Markdown.",
    }
}

fn record_turn(state: &mut AgentState, phase: &str, domain: Option<&str>, content: &str) {
    state.turn_responses.push(TurnResponse {
        phase: phase.to_string(),
        domain: domain.map(ToString::to_string),
        content: content.to_string(),
    });
    refresh_token_budget(state);
}

/// Force summarization of accumulated turn responses before the final phase.
/// Always triggers summarization to keep context manageable for the AI.
async fn force_summarize_context(
    state: &mut AgentState,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<()> {
    let total_chars: usize = state
        .turn_responses
        .iter()
        .map(|response| response.content.len())
        .sum();

    // If context is small enough, no need to summarize
    if total_chars < TURN_DATA_CHAR_LIMIT {
        return Ok(());
    }

    let full_history = state
        .turn_responses
        .iter()
        .map(|response| {
            format!(
                "## {} - {}\n\n{}",
                response.phase,
                response.domain.as_deref().unwrap_or("overview"),
                response.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    eprintln!(
        "  ⟳ Summarizing {} chars of analysis history...",
        total_chars
    );

    let summary_prompt = format!(
        "Summarize the following business analysis history into a concise report.\n\
         Preserve ALL business observations, endpoint purposes, data flows, and functional relationships.\n\
         Do NOT omit or truncate any business finding.\n\
         \n{}",
        full_history
    );

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(format!(
                "{AGENT_IDENTITY_PROMPT}\n\n\
                 You are now in SUMMARIZATION mode. Your task: condense the provided analysis history \
                 into a concise but complete summary. Preserve ALL business observations, endpoint purposes, \
                 data flows, and functional relationships. Be concise but lose NO information."
            )),
            ChatMessage::user(summary_prompt),
        ],
        stream: false,
    };

    let summary = send_chat_request(&request, api_key, api_url)
        .await
        .map_err(|error| match error {
            Error::ApiRequest { source, .. } => Error::ApiRequest {
                context: "Summarization API request failed".to_string(),
                source,
            },
            Error::ApiResponseDecode { source, .. } => Error::ApiResponseDecode {
                context: "Summarization API response decode failed".to_string(),
                source,
            },
            other => other,
        })?;

    state.turn_responses.clear();
    state.turn_responses.push(TurnResponse {
        phase: "summary".to_string(),
        domain: None,
        content: summary,
    });
    refresh_token_budget(state);

    Ok(())
}

fn update_state_from_overview(
    graph: &BusinessGraph,
    state: &mut AgentState,
    overview_response: &str,
) {
    let prioritized = parse_prioritized_domains_from_overview(graph, overview_response);
    if prioritized.is_empty() {
        return;
    }

    let order: BTreeMap<_, _> = prioritized
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();

    state.domains.sort_by(|left, right| {
        let left_rank = order.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_rank = order.get(&right.name).copied().unwrap_or(usize::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| right.endpoint_count.cmp(&left.endpoint_count))
            .then_with(|| left.name.cmp(&right.name))
    });

    for (index, domain) in state.domains.iter_mut().enumerate() {
        domain.priority = (index + 1).min(u8::MAX as usize) as u8;
    }

    state.progress.remaining = state
        .domains
        .iter()
        .map(|domain| domain.name.clone())
        .collect();
    let overview_summary = summarize_text(overview_response, FINDING_SUMMARY_CHAR_LIMIT);
    if !overview_summary.is_empty() {
        push_unique(&mut state.cross_cutting, overview_summary);
    }
    for item in extract_cross_cutting_items(overview_response) {
        push_unique(&mut state.cross_cutting, item);
    }
    refresh_token_budget(state);
}

fn parse_observations_into_state(state: &mut AgentState, domain: Option<&str>, response: &str) -> Result<()> {
    check_budget(state, response, state.token_budget.limit)?;

    let summary = summarize_text(response, FINDING_SUMMARY_CHAR_LIMIT);
    let extracted_observations = parse_observations_from_response(response);

    if let Some(domain) = domain {
        if let Some(domain_state) = state.domains.iter_mut().find(|item| item.name == domain) {
            domain_state.analyzed = true;
            domain_state.observations_summary = Some(summary);
        }

        move_to_completed(&mut state.progress, domain);
    }

    for observation in extracted_observations {
        push_observation(&mut state.observations, observation);
    }

    for item in extract_cross_cutting_items(response) {
        push_unique(&mut state.cross_cutting, item);
    }

    refresh_token_budget(state);
    Ok(())
}

fn build_cross_summary(state: &AgentState, graph: &BusinessGraph) -> String {
    format!(
        "Structured business observations state:\n{}\n\nObserved cross-domain topology:\n{}",
        render_state_snapshot(state),
        build_cross_domain_summary(graph)
    )
}

fn render_state_snapshot(state: &AgentState) -> String {
    let mut out = String::new();
    out.push_str("Domains:\n");
    for domain in &state.domains {
        out.push_str(&format!(
            "- [{}] {} | endpoints={} | analyzed={} | summary={}\n",
            domain.priority,
            domain.name,
            domain.endpoint_count,
            domain.analyzed,
            domain.observations_summary.as_deref().unwrap_or("pending")
        ));
    }

    out.push_str("\nBusiness observations:\n");
    if state.observations.is_empty() {
        out.push_str("- none\n");
    } else {
        for observation in &state.observations {
            out.push_str(&format!(
                "- {} | endpoints={} | evidence={} | notes={}\n",
                observation.title,
                join_or_none(&observation.endpoints),
                observation.evidence,
                observation.notes
            ));
        }
    }

    out.push_str("\nCross-cutting:\n");
    if state.cross_cutting.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in state.cross_cutting.iter().take(CROSS_CUTTING_LIMIT) {
            out.push_str(&format!("- {}\n", item));
        }
    }

    out.push_str(&format!(
        "\nProgress:\n- completed: {}\n- remaining: {}\n",
        join_or_none(&state.progress.completed),
        join_or_none(&state.progress.remaining)
    ));
    out
}

fn render_state_for_final(state: &AgentState) -> String {
    let mut out = String::new();
    out.push_str("BizGraph structured analysis state for final report\n\n");
    out.push_str(&format!(
        "Phase: {}\nToken budget: {}/{}\n\n",
        state.phase, state.token_budget.used, state.token_budget.limit
    ));
    out.push_str("Business domains:\n");
    for domain in &state.domains {
        out.push_str(&format!(
            "- priority={} | {} | endpoints={} | analyzed={} | summary={}\n",
            domain.priority,
            domain.name,
            domain.endpoint_count,
            domain.analyzed,
            domain.observations_summary.as_deref().unwrap_or("pending")
        ));
    }

    out.push_str("\nStructured business observations:\n");
    if state.observations.is_empty() {
        out.push_str("- none\n");
    } else {
        for observation in &state.observations {
            out.push_str(&format!(
                "- title={} | endpoints={} | evidence={} | notes={}\n",
                observation.title,
                join_or_none(&observation.endpoints),
                observation.evidence,
                observation.notes
            ));
        }
    }

    out.push_str("\nCross-cutting observations:\n");
    if state.cross_cutting.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in &state.cross_cutting {
            out.push_str(&format!("- {}\n", item));
        }
    }

    out.push_str(&format!(
        "\nProgress summary:\n- completed: {}\n- remaining: {}\n",
        join_or_none(&state.progress.completed),
        join_or_none(&state.progress.remaining)
    ));
    out
}

fn parse_prioritized_domains_from_overview(
    graph: &BusinessGraph,
    overview_response: &str,
) -> Vec<String> {
    prioritized_function_names(graph, overview_response)
}

fn extract_final_report(response: &str) -> String {
    // Strip any preamble before the first Markdown heading
    let trimmed = response.trim();
    if let Some(start) = trimmed.find('\n') {
        let first_line = &trimmed[..start];
        // If first line looks like a preamble (no heading), skip to first heading
        if !first_line.starts_with('#') {
            if let Some(heading_pos) = trimmed.find("\n#") {
                return trimmed[heading_pos + 1..].to_string();
            }
        }
    }
    trimmed.to_string()
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn move_to_completed(progress: &mut Progress, domain: &str) {
    if !progress.completed.iter().any(|item| item == domain) {
        progress.completed.push(domain.to_string());
    }
    progress.remaining.retain(|item| item != domain);
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !value.is_empty() && !items.iter().any(|item| item == &value) {
        items.push(value);
    }
}

fn push_observation(observations: &mut Vec<BusinessObservation>, observation: BusinessObservation) {
    let duplicate = observations.iter().any(|existing| {
        existing.title.eq_ignore_ascii_case(&observation.title)
            && existing
                .evidence
                .eq_ignore_ascii_case(&observation.evidence)
    });

    if !duplicate {
        observations.push(observation);
        observations.sort_by(|left, right| left.title.cmp(&right.title));
    }
}

fn estimate_tokens(text: &str) -> usize {
    // Use char count / 3 for better CJK accuracy (1 CJK char ≈ 2-3 tokens)
    text.chars().count() * 4 / 3
}

fn check_budget(state: &AgentState, new_text: &str, limit: usize) -> Result<()> {
    let projected = state.token_budget.used + estimate_tokens(new_text);
    if projected > limit {
        Err(Error::BudgetExceeded {
            scope: "agent state token budget".to_string(),
            used: projected,
            limit,
        })
    } else {
        Ok(())
    }
}

fn refresh_token_budget(state: &mut AgentState) {
    let serialized = serde_json::to_string(state).unwrap_or_default();
    state.token_budget.used = estimate_tokens(&serialized);
}

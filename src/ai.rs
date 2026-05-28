use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::{
    BusinessEdge, BusinessGraph, BusinessNodeKind, BusinessNodeProperties, ParameterDescriptor,
};

const SYSTEM_PROMPT: &str = r#"You are a senior business analyst performing application structure analysis from HTTP traffic.

Your job:
- Infer what business capabilities the target application serves.
- Reconstruct likely user and operator flows from endpoints and graph edges.
- Map the business domain structure: what functions exist, how they're organized, what data they handle.
- Identify the purpose of each endpoint and its role in the overall business.

IMPORTANT: You are NOT a security analyst.
- Do NOT identify vulnerabilities, attack vectors, or security weaknesses.
- Do NOT use words like "vulnerability", "exploit", "bypass", "injection", "IDOR".
- Do NOT rate severity or suggest fixes.

Output requirements:
- Return Markdown only.
- Include: Executive Summary, Business Functions, User Flow Analysis, Data Flow Map, Endpoint Purpose Catalog.
- Be specific and evidence-based. Reference endpoint paths, methods, parameters, status patterns.
- If evidence is weak, state confidence level."#;

const AGENT_IDENTITY_PROMPT: &str = r#"You are BizGraph Analysis Agent — a business analyst specializing in understanding application structure from HTTP traffic.

Your ONLY job: analyze traffic patterns to build a deep understanding of the target's business logic.

You are NOT a security analyst. You do NOT identify vulnerabilities, attack vectors, or security weaknesses.
- Do NOT use words like "vulnerability", "exploit", "attack", "bypass", "injection", "IDOR", "critical", "high risk"
- Do NOT rate or classify anything by severity
- Do NOT suggest remediation or fixes
- Do NOT propose penetration testing steps

You ARE a business analyst. You describe:
- What the application does for its users
- How it's organized into functional domains
- What data flows through which endpoints
- How users navigate through the system

Your workflow:
1. OVERVIEW: Identify business domains from endpoint groupings. What services does this application provide?
2. DOMAIN: Per-domain deep dive — what does each endpoint DO? What data flows through it? How do users interact with it?
3. CROSS: Cross-domain correlation — how do business functions connect? What data moves between modules?
4. FINAL: Compile a comprehensive business understanding report.

Output rules:
- Be specific: cite endpoint paths, methods, parameters
- Be evidence-based: link observations to traffic patterns
- Describe WHAT the system does, HOW it's organized, and WHAT data it handles
- Respond in natural Markdown with clear headings and concise analysis."#;
const MAX_DEEP_AI_CALLS: usize = 7;
const AGENT_STATE_TOKEN_LIMIT: usize = 2_500;
const TURN_DATA_CHAR_LIMIT: usize = 3_200;
/// Threshold before triggering LLM summarization (in characters).
/// Target: ~50% of 200K token context window.
/// Chinese text: 1 char ≈ 0.5-1 token → 200K chars ≈ 100K-200K tokens.
const TURN_RESPONSE_SUMMARY_THRESHOLD: usize = 200_000;
const FINDING_SUMMARY_CHAR_LIMIT: usize = 500;
const CROSS_CUTTING_LIMIT: usize = 20;

#[derive(Clone, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

impl ChatMessage {
    fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageContent,
}

#[derive(Deserialize)]
struct ChatMessageContent {
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentState {
    phase: String,
    domains: Vec<DomainAnalysis>,
    observations: Vec<BusinessObservation>,
    cross_cutting: Vec<String>,
    progress: Progress,
    token_budget: TokenBudget,
    turn_responses: Vec<TurnResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnResponse {
    phase: String,
    domain: Option<String>,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DomainAnalysis {
    name: String,
    priority: u8,
    endpoint_count: usize,
    analyzed: bool,
    observations_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BusinessObservation {
    title: String,
    evidence: String,
    endpoints: Vec<String>,
    notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Progress {
    completed: Vec<String>,
    remaining: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenBudget {
    used: usize,
    limit: usize,
}

impl AgentState {
    fn new(graph: &BusinessGraph) -> Self {
        let mut domains: Vec<_> = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.properties {
                BusinessNodeProperties::BusinessFunction(details) => Some((
                    node.label.clone(),
                    details.endpoint_count,
                )),
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
            remaining: domain_entries.iter().map(|domain| domain.name.clone()).collect(),
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

/// Send the business graph to the configured chat completion API for AI analysis.
/// Returns a Markdown report.
pub async fn analyze_with_ai(
    graph: &BusinessGraph,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String, String> {
    let summary = build_graph_summary(graph);

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: SYSTEM_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!(
                    "Analyze the following structured summary of a business graph extracted from network traffic.\n\
                     The summary contains BusinessFunction nodes (grouped by host+path prefix) and \
                     Endpoint nodes (HTTP endpoints with methods, status codes, and schemas).\n\
                     \n\
                     Describe:\n\
                     1. What business functions does this application serve?\n\
                     2. What's the user flow / navigation pattern?\n\
                     3. How are data and operations organized across business functions?\n\
                     4. What is the purpose of each endpoint?\n\
                     \n\
                     Respond in Markdown.\n\
                     \n{summary}"
                ),
            },
        ],
        stream: false,
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(api_url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("AI API request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("AI API error ({status}): {body}"));
    }

    let chat_resp: ChatResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse AI response: {e}"))?;

    let content = chat_resp
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    Ok(content)
}

pub async fn analyze_with_ai_deep(
    graph: &BusinessGraph,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String, String> {
    run_agent(graph, api_key, model, api_url).await
}

async fn run_agent(
    graph: &BusinessGraph,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String, String> {
    let mut state = AgentState::new(graph);
    let mut calls_made = 0usize;

    eprintln!("Phase 1/4: Business overview...");
    let overview_context = build_turn_context(
        &state,
        "OVERVIEW",
        &build_graph_overview(graph),
    );
    let overview = chat_fresh(overview_context, api_key, model, api_url).await?;
    calls_made += 1;
    record_turn(&mut state, "overview", None, &overview);
    update_state_from_overview(graph, &mut state, &overview);
    parse_observations_into_state(&mut state, None, &overview);

    state.phase = "domain".to_string();
    let domain_budget = MAX_DEEP_AI_CALLS.saturating_sub(3);
    let selected_domains: Vec<_> = state.domains.iter().take(domain_budget).cloned().collect();

    eprintln!(
        "Phase 2/4: Deep-diving {} prioritized domains in parallel...",
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
            Ok::<_, String>((name, response))
        }));
    }

    for task in domain_tasks {
        let (domain, response) = task
            .await
            .map_err(|e| format!("Domain deep-dive task panicked: {e}"))??;
        calls_made += 1;
        record_turn(&mut state, "domain", Some(&domain), &response);
        parse_observations_into_state(&mut state, Some(&domain), &response);
    }

    state.phase = "cross".to_string();
    eprintln!("Phase 3/4: Cross-domain correlation...");
    maybe_summarize_context(&mut state, api_key, model, api_url).await?;
    let cross_context = build_turn_context(
        &state,
        "CROSS_DOMAIN",
        &build_cross_summary(&state, graph),
    );
    let cross = chat_fresh(cross_context, api_key, model, api_url).await?;
    calls_made += 1;
    record_turn(&mut state, "cross", None, &cross);
    compress_cross_into_state(&mut state, &cross);

    state.phase = "final".to_string();
    eprintln!("Phase 4/4: Final report synthesis...");
    maybe_summarize_context(&mut state, api_key, model, api_url).await?;
    let final_context = build_turn_context(&state, "FINAL_REPORT", &render_state_for_final(&state));
    let report = chat_fresh(final_context, api_key, model, api_url).await?;
    calls_made += 1;

    if calls_made > MAX_DEEP_AI_CALLS {
        return Err(format!(
            "deep AI analysis exceeded API call budget: {calls_made} > {MAX_DEEP_AI_CALLS}"
        ));
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
    if char_count > TURN_DATA_CHAR_LIMIT {
        eprintln!(
            "Soft warning: task {task} context payload is {char_count} chars, above soft limit {TURN_DATA_CHAR_LIMIT}; keeping full data."
        );
    }

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
            "Scan the graph overview, identify business domains, categorize by function, and explain the business context."
        }
        "DOMAIN_DEEP_DIVE" => {
            "Analyze this single business domain. Describe what each endpoint does, what data it handles, what user actions it supports, and how endpoints relate to each other within this domain."
        }
        "CROSS_DOMAIN" => {
            "Correlate domain findings with the topology. Describe how business functions connect, what data flows between modules, and the overall business architecture."
        }
        "FINAL_REPORT" => {
            "Compile a comprehensive business understanding report. Synthesize all domain analysis into a clear picture of what this application does, organized by business function. Return a polished Markdown report."
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

async fn maybe_summarize_context(
    state: &mut AgentState,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String, String> {
    let total_chars: usize = state.turn_responses.iter().map(|response| response.content.len()).sum();

    if total_chars < TURN_RESPONSE_SUMMARY_THRESHOLD {
        return Ok(state
            .turn_responses
            .iter()
            .map(|response| {
                format!(
                    "## {} ({})\n\n{}",
                    response.phase,
                    response.domain.as_deref().unwrap_or("overview"),
                    response.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"));
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
            ChatMessage::system("You are a precise technical summarizer. Preserve all business analysis findings, endpoint descriptions, data flows, and functional observations. Be concise but lose NO information."),
            ChatMessage::user(summary_prompt),
        ],
        stream: false,
    };

    let summary = send_chat_request(&request, api_key, api_url)
        .await
        .map_err(|error| format!("Summarization API request failed: {error}"))?;

    state.turn_responses.clear();
    state.turn_responses.push(TurnResponse {
        phase: "summary".to_string(),
        domain: None,
        content: summary.clone(),
    });
    refresh_token_budget(state);

    Ok(summary)
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

    state.progress.remaining = state.domains.iter().map(|domain| domain.name.clone()).collect();
    let overview_summary = summarize_text(overview_response, FINDING_SUMMARY_CHAR_LIMIT);
    if !overview_summary.is_empty() {
        push_unique(&mut state.cross_cutting, overview_summary);
    }
    for item in extract_cross_cutting_items(overview_response) {
        push_unique(&mut state.cross_cutting, item);
    }
    refresh_token_budget(state);
}

fn parse_observations_into_state(state: &mut AgentState, domain: Option<&str>, response: &str) {
    let _ = check_budget(state, response, state.token_budget.limit);

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
}

fn compress_cross_into_state(state: &mut AgentState, response: &str) {
    let _ = check_budget(state, response, state.token_budget.limit);

    for item in extract_cross_cutting_items(response) {
        push_unique(&mut state.cross_cutting, item);
    }

    for observation in parse_observations_from_response(response) {
        push_observation(&mut state.observations, observation);
    }

    refresh_token_budget(state);
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
            domain
                .observations_summary
                .as_deref()
                .unwrap_or("pending")
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
            domain
                .observations_summary
                .as_deref()
                .unwrap_or("pending")
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

fn summarize_text(text: &str, limit: usize) -> String {
    let bullets = extract_key_bullets(text, 4);
    if bullets.is_empty() {
        soft_limit_text(text, limit)
    } else {
        soft_limit_text(&bullets.join(" | "), limit)
    }
}

fn extract_key_bullets(text: &str, limit: usize) -> Vec<String> {
    text.lines()
        .map(clean_line)
        .filter(|line| {
            !line.is_empty()
                && (line.starts_with("- ")
                    || line.starts_with("* ")
                    || line.starts_with("1. ")
                    || line.starts_with("2. ")
                    || line.starts_with("3. ")
                    || line.to_ascii_lowercase().contains("business")
                    || line.to_ascii_lowercase().contains("domain")
                    || line.to_ascii_lowercase().contains("flow")
                    || line.to_ascii_lowercase().contains("endpoint")
                    || line.to_ascii_lowercase().contains("data")
                    || line.to_ascii_lowercase().contains("user"))
        })
        .take(limit)
        .collect()
}

fn parse_observations_from_response(response: &str) -> Vec<BusinessObservation> {
    let mut blocks = Vec::new();
    let mut current = String::new();

    for raw_line in response.lines() {
        let line = raw_line.trim();
        let starts_new_block = line.starts_with("##")
            || line.starts_with("###")
            || line.starts_with("####")
            || line.starts_with("- ");

        if starts_new_block && !current.trim().is_empty() {
            blocks.push(current.trim().to_string());
            current.clear();
        }

        if !line.is_empty() {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }

    if !current.trim().is_empty() {
        blocks.push(current.trim().to_string());
    }

    let mut observations = Vec::new();
    for block in blocks {
        let title = extract_title(&block);
        let evidence = block.clone();
        let endpoints = extract_endpoints(&block);
        let notes = extract_notes(&block, &endpoints);

        observations.push(BusinessObservation {
            title,
            evidence,
            endpoints,
            notes,
        });
    }

    observations
}

fn extract_cross_cutting_items(response: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in response.lines() {
        let cleaned = clean_line(line);
        if cleaned.is_empty() {
            continue;
        }

        let lowered = cleaned.to_ascii_lowercase();
        if lowered.contains("cross")
            || lowered.contains("shared")
            || lowered.contains("workflow")
            || lowered.contains("data flow")
            || lowered.contains("user flow")
            || lowered.contains("module")
            || lowered.contains("domain")
            || lowered.contains("business")
            || lowered.contains("relationship")
        {
            items.push(cleaned);
        }
    }
    items.truncate(CROSS_CUTTING_LIMIT);
    items
}

fn extract_title(block: &str) -> String {
    let first_line = block.lines().next().unwrap_or("Observation");
    let cleaned = clean_line(first_line)
        .trim_start_matches('#')
        .trim_start_matches('-')
        .trim()
        .to_string();

    let normalized = cleaned
        .trim_start_matches(':')
        .trim_start_matches('-')
        .trim();

    if normalized.is_empty() {
        "Observation".to_string()
    } else {
        normalized.to_string()
    }
}

fn extract_notes(block: &str, endpoints: &[String]) -> String {
    for line in block.lines() {
        let cleaned = clean_line(line);
        let lowered = cleaned.to_ascii_lowercase();
        if lowered.contains("data")
            || lowered.contains("flow")
            || lowered.contains("user")
            || lowered.contains("domain")
            || lowered.contains("module")
            || lowered.contains("business")
        {
            return cleaned;
        }
    }

    if endpoints.is_empty() {
        "Additional business context inferred from the surrounding traffic and workflow structure.".to_string()
    } else {
        format!(
            "Relevant endpoint context grouped from {}.",
            join_or_none(endpoints)
        )
    }
}

fn extract_endpoints(text: &str) -> Vec<String> {
    const HTTP_METHODS: [&str; 9] = [
        "GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD", "TRACE", "CONNECT",
    ];

    let mut endpoints = Vec::new();
    for line in text.lines() {
        let normalized = line.replace(['(', ')', '[', ']', ',', ';'], " ");
        let tokens: Vec<_> = normalized.split_whitespace().collect();
        let mut path_candidate = None;
        let mut method_candidate = None;

        for token in &tokens {
            let cleaned = token.trim_matches(|ch: char| ch == '.' || ch == ':' || ch == '"');
            if cleaned.starts_with('/') && path_candidate.is_none() {
                path_candidate = Some(cleaned.to_string());
            }

            let upper = cleaned.to_ascii_uppercase();
            if HTTP_METHODS.contains(&upper.as_str()) && method_candidate.is_none() {
                method_candidate = Some(upper);
            }
        }

        if let Some(path) = path_candidate {
            let endpoint = if let Some(method) = method_candidate {
                format!("{} {}", method, path)
            } else {
                path
            };
            push_unique(&mut endpoints, endpoint);
        }
    }

    endpoints.truncate(6);
    endpoints
}

fn clean_line(line: &str) -> String {
    line.trim()
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
        .trim()
        .to_string()
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
            && existing.evidence.eq_ignore_ascii_case(&observation.evidence)
    });

    if !duplicate {
        observations.push(observation);
        observations.sort_by(|left, right| left.title.cmp(&right.title));
    }
}

fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

fn check_budget(state: &AgentState, new_text: &str, limit: usize) -> Result<(), String> {
    let projected = state.token_budget.used + estimate_tokens(new_text);
    if projected > limit {
        Err(format!(
            "agent state token budget would be exceeded: {projected} > {limit}"
        ))
    } else {
        Ok(())
    }
}

fn refresh_token_budget(state: &mut AgentState) {
    let serialized = serde_json::to_string(state).unwrap_or_default();
    state.token_budget.used = estimate_tokens(&serialized);
}

fn soft_limit_text(text: &str, limit: usize) -> String {
    let normalized = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    let char_count = normalized.chars().count();
    if char_count > limit {
        eprintln!(
            "Soft warning: text block is {char_count} chars, above soft limit {limit}; preserving full content."
        );
    }
    normalized
}

fn parse_prioritized_domains_from_overview(
    graph: &BusinessGraph,
    overview_response: &str,
) -> Vec<String> {
    prioritized_function_names(graph, overview_response)
}

fn extract_final_report(response: &str) -> String {
    response.to_string()
}

fn build_graph_summary(graph: &BusinessGraph) -> String {
    let mut lines = Vec::new();
    let mut endpoint_labels = BTreeMap::new();
    let mut function_labels = BTreeMap::new();
    let mut edge_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut data_dependencies = Vec::new();
    let mut call_sequences = Vec::new();
    let mut identity_candidates = Vec::new();
    let mut operator_candidates = Vec::new();
    let mut data_rich_candidates = Vec::new();
    let mut high_error_endpoints = Vec::new();

    for node in &graph.nodes {
        match &node.properties {
            BusinessNodeProperties::BusinessFunction(details) => {
                function_labels.insert(node.id, node.label.clone());
                lines.push(format!(
                    "- {} | host={} | prefix={} | endpoints={}",
                    node.label, details.host, details.path_prefix, details.endpoint_count
                ));
            }
            BusinessNodeProperties::Endpoint(details) => {
                endpoint_labels.insert(node.id, node.label.clone());
                let parameters = summarize_parameters(&details.parameters);
                let request_schema = summarize_schema_presence(details.request_schema.is_some());
                let response_schema = summarize_schema_presence(details.response_schema.is_some());
                let methods = details.methods.join(", ");
                let statuses = details
                    .status_codes
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");

                lines.push(format!(
                    "- {} | path={} | methods=[{}] | status_codes=[{}] | parameters={} | request_schema={} | response_schema={} | confidence={:.2}",
                    node.label,
                    details.path_template,
                    methods,
                    statuses,
                    parameters,
                    request_schema,
                    response_schema,
                    details.confidence
                ));

                if looks_interesting(&node.label, &details.path_template, &["login", "auth", "token", "session", "oauth", "signin", "signup"]) {
                    identity_candidates.push(format!(
                        "{} [{}] params={} statuses=[{}]",
                        node.label, methods, parameters, statuses
                    ));
                }
                if looks_interesting(&node.label, &details.path_template, &["admin", "internal", "manage", "console", "backoffice", "root"]) {
                    operator_candidates.push(format!(
                        "{} [{}] statuses=[{}]",
                        node.label, methods, statuses
                    ));
                }
                if has_sensitive_parameters(&details.parameters) || looks_interesting(&node.label, &details.path_template, &["password", "secret", "key", "token", "code", "otp", "phone", "email", "user", "account", "invoice", "pay", "order", "refund", "balance"]) {
                    data_rich_candidates.push(format!(
                        "{} [{}] params={} request_schema={} response_schema={}",
                        node.label, methods, parameters, request_schema, response_schema
                    ));
                }
                if details.status_profiles.client_error > 0 || details.status_profiles.server_error > 0 {
                    high_error_endpoints.push(format!(
                        "{} success={} redirect={} client_error={} server_error={}",
                        node.label,
                        details.status_profiles.success,
                        details.status_profiles.redirect,
                        details.status_profiles.client_error,
                        details.status_profiles.server_error
                    ));
                }
            }
            BusinessNodeProperties::Host(details) => {
                lines.push(format!(
                    "- {} | host_metadata_keys={}",
                    node.label,
                    details.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }

    lines.sort();

    for edge in &graph.edges {
        *edge_counts.entry(edge.label.as_str()).or_insert(0) += 1;
        collect_edge_summary(
            edge,
            &endpoint_labels,
            &function_labels,
            &mut call_sequences,
            &mut data_dependencies,
        );
    }

    let mut summary = String::new();
    summary.push_str("BusinessGraph traffic analysis summary\n\n");
    summary.push_str(&format!(
        "Node counts: total={} hosts={} business_functions={} endpoints={}\n",
        graph.nodes.len(),
        count_kind(graph, BusinessNodeKind::Host),
        count_kind(graph, BusinessNodeKind::BusinessFunction),
        count_kind(graph, BusinessNodeKind::Endpoint)
    ));
    summary.push_str(&format!("Edge counts: total={}\n", graph.edges.len()));

    summary.push_str("\nEdge label statistics:\n");
    for (label, count) in edge_counts {
        summary.push_str(&format!("- {}: {}\n", label, count));
    }

    summary.push_str("\nBusiness functions and endpoints:\n");
    for line in lines {
        summary.push_str(&line);
        summary.push('\n');
    }

    summary.push_str("\nObserved call sequences (up to 25):\n");
    append_limited(&mut summary, &call_sequences, 25);

    summary.push_str("\nObserved data dependencies (up to 25):\n");
    append_limited(&mut summary, &data_dependencies, 25);

    summary.push_str("\nIdentity and session-related endpoints:\n");
    append_limited(&mut summary, &identity_candidates, 20);

    summary.push_str("\nOperator or management-oriented endpoints:\n");
    append_limited(&mut summary, &operator_candidates, 20);

    summary.push_str("\nData-rich business endpoints:\n");
    append_limited(&mut summary, &data_rich_candidates, 25);

    summary.push_str("\nEndpoints with notable error activity:\n");
    append_limited(&mut summary, &high_error_endpoints, 20);

    summary
}

fn build_graph_overview(graph: &BusinessGraph) -> String {
    let mut function_lines = Vec::new();
    let mut edge_counts: BTreeMap<&str, usize> = BTreeMap::new();

    for node in &graph.nodes {
        if let BusinessNodeProperties::BusinessFunction(details) = &node.properties {
            function_lines.push(format!(
                "- {} | host={} | prefix={} | endpoints={}",
                node.label, details.host, details.path_prefix, details.endpoint_count
            ));
        }
    }

    function_lines.sort();

    for edge in &graph.edges {
        *edge_counts.entry(edge.label.as_str()).or_insert(0) += 1;
    }

    let mut summary = String::new();
    summary.push_str("BusinessGraph overview for domain discovery\n\n");
    summary.push_str(&format!(
        "Node counts: total={} hosts={} business_functions={} endpoints={}\n",
        graph.nodes.len(),
        count_kind(graph, BusinessNodeKind::Host),
        count_kind(graph, BusinessNodeKind::BusinessFunction),
        count_kind(graph, BusinessNodeKind::Endpoint)
    ));
    summary.push_str(&format!("Edge counts: total={}\n", graph.edges.len()));
    summary.push_str("\nBusiness functions:\n");
    for line in function_lines {
        summary.push_str(&line);
        summary.push('\n');
    }
    summary.push_str("\nEdge label statistics:\n");
    for (label, count) in edge_counts {
        summary.push_str(&format!("- {}: {}\n", label, count));
    }

    summary
}

fn build_function_detail(function_name: &str, graph: &BusinessGraph) -> String {
    let function_node = graph.nodes.iter().find(|node| {
        node.kind == BusinessNodeKind::BusinessFunction && node.label == function_name
    });

    let Some(function_node) = function_node else {
        return format!("Business module '{}' not found in graph.", function_name);
    };

    let BusinessNodeProperties::BusinessFunction(function_details) = &function_node.properties else {
        return format!("Node '{}' is not a business function.", function_name);
    };

    let endpoint_by_id: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.properties {
            BusinessNodeProperties::Endpoint(details) => Some((node.id, (node, details))),
            _ => None,
        })
        .collect();

    let mut endpoint_ids = Vec::new();
    for edge in &graph.edges {
        if edge.label == "contains" && edge.source_node_id == function_node.id {
            endpoint_ids.push(edge.target_node_id);
        }
    }

    if endpoint_ids.is_empty() {
        for node in &graph.nodes {
            if let BusinessNodeProperties::Endpoint(details) = &node.properties {
                if details.path_template.starts_with(&function_details.path_prefix) {
                    endpoint_ids.push(node.id);
                }
            }
        }
    }

    endpoint_ids.sort();
    endpoint_ids.dedup();

    let endpoint_set: std::collections::BTreeSet<_> = endpoint_ids.iter().copied().collect();
    let mut lines = Vec::new();
    lines.push(format!(
        "Module: {} | host={} | prefix={} | endpoint_count={}",
        function_node.label,
        function_details.host,
        function_details.path_prefix,
        function_details.endpoint_count
    ));

    let mut endpoint_sections = Vec::new();
    for endpoint_id in &endpoint_ids {
        let Some((endpoint_node, endpoint_details)) = endpoint_by_id.get(endpoint_id) else {
            continue;
        };

        let methods = endpoint_details.methods.join(", ");
        let statuses = endpoint_details
            .status_codes
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let parameters = summarize_parameters(&endpoint_details.parameters);
        let request_schema = summarize_schema(&endpoint_details.request_schema);
        let response_schema = summarize_schema(&endpoint_details.response_schema);
        let call_sequences = build_endpoint_call_sequences(*endpoint_id, &endpoint_by_id, graph, &endpoint_set);

        endpoint_sections.push(format!(
            "### {}\n- path: {}\n- methods: [{}]\n- status_codes: [{}]\n- parameters: {}\n- request_schema: {}\n- response_schema: {}\n- call_sequences (max 10):\n{}\n- normalization_notes: {}\n- confidence: {:.2}",
            endpoint_node.label,
            endpoint_details.path_template,
            methods,
            statuses,
            parameters,
            request_schema,
            response_schema,
            format_sequence_lines(&call_sequences),
            if endpoint_details.normalization_notes.is_empty() {
                "none".to_string()
            } else {
                endpoint_details.normalization_notes.join(" | ")
            },
            endpoint_details.confidence
        ));
    }

    lines.push("\nEndpoints:\n".to_string());
    lines.extend(endpoint_sections);
    lines.join("\n")
}

fn build_cross_domain_summary(graph: &BusinessGraph) -> String {
    let function_by_id: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.properties {
            BusinessNodeProperties::BusinessFunction(details) => Some((node.id, (node, details))),
            _ => None,
        })
        .collect();
    let endpoint_to_function = map_endpoint_to_function(graph);
    let endpoint_labels: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            BusinessNodeKind::Endpoint => Some((node.id, node.label.clone())),
            _ => None,
        })
        .collect();
    let mut domain_links: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();

    for edge in &graph.edges {
        if edge.label != "calls_after" && !edge.label.starts_with("data_dependency:") {
            continue;
        }

        let Some(source_function_id) = endpoint_to_function.get(&edge.source_node_id) else {
            continue;
        };
        let Some(target_function_id) = endpoint_to_function.get(&edge.target_node_id) else {
            continue;
        };
        if source_function_id == target_function_id {
            continue;
        }

        let Some((source_function, _)) = function_by_id.get(source_function_id) else {
            continue;
        };
        let Some((target_function, _)) = function_by_id.get(target_function_id) else {
            continue;
        };
        let source_endpoint = endpoint_labels
            .get(&edge.source_node_id)
            .cloned()
            .unwrap_or_else(|| edge.source_node_id.to_string());
        let target_endpoint = endpoint_labels
            .get(&edge.target_node_id)
            .cloned()
            .unwrap_or_else(|| edge.target_node_id.to_string());

        domain_links
            .entry((source_function.label.clone(), target_function.label.clone()))
            .or_default()
            .push(format!("{} -> {} via {}", source_endpoint, target_endpoint, edge.label));
    }

    let mut summary = String::new();
    summary.push_str("Cross-domain topology\n\nBusiness functions:\n");
    for (_, (node, details)) in &function_by_id {
        summary.push_str(&format!(
            "- {} | host={} | prefix={} | endpoints={}\n",
            node.label, details.host, details.path_prefix, details.endpoint_count
        ));
    }

    summary.push_str("\nCross-domain relationships:\n");
    if domain_links.is_empty() {
        summary.push_str("- none observed\n");
    } else {
        for ((source, target), links) in domain_links {
            summary.push_str(&format!("- {} -> {}\n", source, target));
            for link in links.iter().take(10) {
                summary.push_str(&format!("  - {}\n", link));
            }
        }
    }

    summary
}

fn count_kind(graph: &BusinessGraph, kind: BusinessNodeKind) -> usize {
    graph.nodes.iter().filter(|node| node.kind == kind).count()
}

fn collect_edge_summary(
    edge: &BusinessEdge,
    endpoint_labels: &BTreeMap<uuid::Uuid, String>,
    function_labels: &BTreeMap<uuid::Uuid, String>,
    call_sequences: &mut Vec<String>,
    data_dependencies: &mut Vec<String>,
) {
    let source = endpoint_labels
        .get(&edge.source_node_id)
        .or_else(|| function_labels.get(&edge.source_node_id))
        .cloned()
        .unwrap_or_else(|| edge.source_node_id.to_string());
    let target = endpoint_labels
        .get(&edge.target_node_id)
        .or_else(|| function_labels.get(&edge.target_node_id))
        .cloned()
        .unwrap_or_else(|| edge.target_node_id.to_string());

    if edge.label == "calls_after" {
        call_sequences.push(format!("- {} -> {}", source, target));
    } else if edge.label.starts_with("data_dependency:") {
        data_dependencies.push(format!("- {} -> {} via {}", source, target, edge.label));
    }
}

fn summarize_parameters(parameters: &[ParameterDescriptor]) -> String {
    if parameters.is_empty() {
        return "none".to_string();
    }

    parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}:{:?}:{:?}:{}",
                parameter.name, parameter.location, parameter.kind, parameter.occurrence_count
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn summarize_schema(schema: &Option<crate::types::SchemaShape>) -> String {
    match schema {
        Some(shape) => serde_json::to_string(shape).unwrap_or_else(|_| "<schema unavailable>".to_string()),
        None => "none".to_string(),
    }
}

fn summarize_schema_presence(present: bool) -> &'static str {
    if present {
        "present"
    } else {
        "none"
    }
}

fn has_sensitive_parameters(parameters: &[ParameterDescriptor]) -> bool {
    parameters.iter().any(|parameter| {
        let name = parameter.name.to_ascii_lowercase();
        ["token", "password", "secret", "key", "code", "otp", "auth", "session", "email", "phone", "user", "account", "amount", "price", "balance", "order", "refund", "coupon"]
            .iter()
            .any(|needle| name.contains(needle))
    })
}

fn looks_interesting(label: &str, path: &str, needles: &[&str]) -> bool {
    let haystack = format!("{} {}", label.to_ascii_lowercase(), path.to_ascii_lowercase());
    needles.iter().any(|needle| haystack.contains(needle))
}

fn append_limited(summary: &mut String, items: &[String], limit: usize) {
    if items.is_empty() {
        summary.push_str("- none observed\n");
        return;
    }

    for item in items.iter().take(limit) {
        summary.push_str(item);
        summary.push('\n');
    }
}

fn map_endpoint_to_function(graph: &BusinessGraph) -> BTreeMap<uuid::Uuid, uuid::Uuid> {
    let mut mapping = BTreeMap::new();
    let functions: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == BusinessNodeKind::BusinessFunction)
        .collect();

    for edge in &graph.edges {
        if edge.label == "contains" {
            mapping.insert(edge.target_node_id, edge.source_node_id);
        }
    }

    for node in &graph.nodes {
        let BusinessNodeProperties::Endpoint(details) = &node.properties else {
            continue;
        };

        if mapping.contains_key(&node.id) {
            continue;
        }

        if let Some(function) = functions.iter().find(|function| match &function.properties {
            BusinessNodeProperties::BusinessFunction(function_details) => {
                details.path_template.starts_with(&function_details.path_prefix)
            }
            _ => false,
        }) {
            mapping.insert(node.id, function.id);
        }
    }

    mapping
}

fn build_endpoint_call_sequences(
    endpoint_id: uuid::Uuid,
    endpoint_by_id: &BTreeMap<uuid::Uuid, (&crate::types::BusinessNode, &crate::types::EndpointProperties)>,
    graph: &BusinessGraph,
    endpoint_scope: &std::collections::BTreeSet<uuid::Uuid>,
) -> Vec<String> {
    let mut sequences = Vec::new();

    for edge in &graph.edges {
        if edge.label != "calls_after" {
            continue;
        }

        let source_in_scope = endpoint_scope.contains(&edge.source_node_id);
        let target_in_scope = endpoint_scope.contains(&edge.target_node_id);
        if !source_in_scope || !target_in_scope {
            continue;
        }
        if edge.source_node_id != endpoint_id && edge.target_node_id != endpoint_id {
            continue;
        }

        let source = endpoint_by_id
            .get(&edge.source_node_id)
            .map(|(node, _)| node.label.clone())
            .unwrap_or_else(|| edge.source_node_id.to_string());
        let target = endpoint_by_id
            .get(&edge.target_node_id)
            .map(|(node, _)| node.label.clone())
            .unwrap_or_else(|| edge.target_node_id.to_string());
        sequences.push(format!("- {} -> {}", source, target));
    }

    sequences.sort();
    sequences.truncate(10);
    sequences
}

fn format_sequence_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        "- none observed".to_string()
    } else {
        lines.join("\n")
    }
}

fn prioritized_function_names(graph: &BusinessGraph, overview_response: &str) -> Vec<String> {
    let mut functions: Vec<_> = graph
        .nodes
        .iter()
        .filter_map(|node| match &node.properties {
            BusinessNodeProperties::BusinessFunction(details) => {
                Some((node.label.clone(), details.endpoint_count))
            }
            _ => None,
        })
        .collect();

    functions.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let lowered = overview_response.to_ascii_lowercase();
    let mut prioritized = Vec::new();
    let mut remaining = Vec::new();

    for (label, count) in functions {
        if lowered.contains(&label.to_ascii_lowercase()) {
            prioritized.push((label, count));
        } else {
            remaining.push((label, count));
        }
    }

    prioritized.extend(remaining);
    prioritized.into_iter().take(5).map(|(label, _)| label).collect()
}

async fn chat_fresh(
    messages: Vec<ChatMessage>,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String, String> {
    let request = ChatRequest {
        model: model.to_string(),
        messages,
        stream: false,
    };

    send_chat_request(&request, api_key, api_url).await
}

async fn send_chat_request(
    request: &ChatRequest,
    api_key: &str,
    api_url: &str,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(api_url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(request)
        .send()
        .await
        .map_err(|e| format!("AI API request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("AI API error ({status}): {body}"));
    }

    let chat_resp: ChatResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse AI response: {e}"))?;

    Ok(chat_resp
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default())
}

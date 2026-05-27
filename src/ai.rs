use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::{
    BusinessEdge, BusinessGraph, BusinessNodeKind, BusinessNodeProperties, ParameterDescriptor,
};

const SYSTEM_PROMPT: &str = r#"You are a senior penetration tester performing business logic analysis from HTTP traffic structure.

Your job:
- Infer what business capabilities the target application serves.
- Reconstruct likely user and operator flows from endpoints and graph edges.
- Identify business logic and application security concerns, especially exposed admin/internal endpoints, weak or missing auth patterns, sensitive data handling, high-risk workflow transitions, object reference abuse, dangerous state changes, and interesting attack surface.
- Suggest concrete next steps for deeper manual testing.

Output requirements:
- Return Markdown only.
- Use clear sections with headings.
- Include: Executive Summary, Business Functions, User Flow Hypotheses, Potential Security Concerns, High-Value Endpoints, Recommended Next Tests.
- Be specific and evidence-based. Reference endpoint paths, methods, parameters, status patterns, and graph relationships.
- If evidence is weak, say so explicitly instead of overstating certainty."#;

const DEEP_OVERVIEW_SYSTEM_PROMPT: &str = "You are doing business domain discovery.";
const DEEP_DOMAIN_SYSTEM_PROMPT: &str =
    "You are analyzing this specific business module for security vulnerabilities.";
const DEEP_CROSS_DOMAIN_SYSTEM_PROMPT: &str =
    "You are analyzing cross-domain security relationships across business modules.";
const DEEP_FINAL_SYSTEM_PROMPT: &str =
    "Compile ALL findings into a final structured penetration test report. Merge overlapping findings, deduplicate, prioritize by severity. Format in Markdown with Executive Summary, Business Functions, Security Concerns (grouped by severity), Detailed Findings per Endpoint, Recommended Next Steps.";
const MAX_DEEP_AI_CALLS: usize = 7;

#[derive(Clone, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
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
                     The summary contains BusinessFunction nodes (grouped by host+path prefix), \
                     Endpoint nodes (HTTP endpoints with methods, status codes, and schemas), \
                     and edge statistics.\n\
                     \n\
                     Identify:\n\
                     1. What business functions does this application serve?\n\
                     2. What's the user flow / navigation pattern?\n\
                     3. Potential security issues (exposed admin endpoints, missing auth, IDOR, sensitive data leak, etc.)\n\
                     4. High-value targets for manual penetration testing\n\
                     5. Concrete next steps for deeper testing\n\
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
    let mut history = Vec::new();
    let mut findings = Vec::new();
    let mut calls_made = 0usize;

    eprintln!("Turn 1/4: Business overview...");
    let overview_response = chat_with_history(
        &mut history,
        DEEP_OVERVIEW_SYSTEM_PROMPT,
        format!(
            "Analyze the business graph at a high level. Identify the business domains present and prioritize which modules should be analyzed first.\n\nReturn:\n1. Identified business domains\n2. Prioritized analysis order\n3. Why each prioritized module matters\n\nUse only this overview data for now:\n\n{}",
            build_graph_overview(graph)
        ),
        api_key,
        model,
        api_url,
    )
    .await?;
    calls_made += 1;
    findings.push(("Business overview".to_string(), overview_response.clone()));

    let function_names = prioritized_function_names(graph, &overview_response);
    let deep_dive_budget = MAX_DEEP_AI_CALLS.saturating_sub(3);

    for function_name in function_names.into_iter().take(deep_dive_budget) {
        eprintln!("Turn 2/4: Analyzing {function_name} module...");
        let detail = build_function_detail(&function_name, graph);
        let response = chat_with_history(
            &mut history,
            DEEP_DOMAIN_SYSTEM_PROMPT,
            format!(
                "Now analyze the following business module in detail:\n\n{}\n\n{}\n\nFocus on security vulnerabilities specific to these endpoints.",
                function_name, detail
            ),
            api_key,
            model,
            api_url,
        )
        .await?;
        calls_made += 1;
        findings.push((function_name, response));
    }

    eprintln!("Turn 3/4: Cross-domain analysis...");
    let cross_domain_response = chat_with_history(
        &mut history,
        DEEP_CROSS_DOMAIN_SYSTEM_PROMPT,
        format!(
            "Review the business-module findings below together with the cross-domain topology. Identify cross-cutting concerns such as auth bypass across domains, data leakage between modules, and privilege escalation paths.\n\nFindings so far:\n\n{}\n\nCross-domain topology:\n\n{}",
            format_findings(&findings),
            build_cross_domain_summary(graph)
        ),
        api_key,
        model,
        api_url,
    )
    .await?;
    calls_made += 1;
    findings.push(("Cross-domain".to_string(), cross_domain_response));

    eprintln!("Turn 4/4: Final report synthesis...");
    let final_response = chat_with_history(
        &mut history,
        DEEP_FINAL_SYSTEM_PROMPT,
        format!(
            "Compile ALL findings into the final report. Use the accumulated findings below, merge overlaps, deduplicate, and prioritize by severity.\n\n{}",
            format_findings(&findings)
        ),
        api_key,
        model,
        api_url,
    )
    .await?;
    calls_made += 1;

    if calls_made > MAX_DEEP_AI_CALLS {
        return Err(format!(
            "deep AI analysis exceeded API call budget: {calls_made} > {MAX_DEEP_AI_CALLS}"
        ));
    }

    Ok(final_response)
}

fn build_graph_summary(graph: &BusinessGraph) -> String {
    let mut lines = Vec::new();
    let mut endpoint_labels = BTreeMap::new();
    let mut function_labels = BTreeMap::new();
    let mut edge_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut data_dependencies = Vec::new();
    let mut call_sequences = Vec::new();
    let mut auth_candidates = Vec::new();
    let mut admin_candidates = Vec::new();
    let mut sensitive_candidates = Vec::new();
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
                    auth_candidates.push(format!(
                        "{} [{}] params={} statuses=[{}]",
                        node.label, methods, parameters, statuses
                    ));
                }
                if looks_interesting(&node.label, &details.path_template, &["admin", "internal", "manage", "console", "backoffice", "root"]) {
                    admin_candidates.push(format!(
                        "{} [{}] statuses=[{}]",
                        node.label, methods, statuses
                    ));
                }
                if has_sensitive_parameters(&details.parameters) || looks_interesting(&node.label, &details.path_template, &["password", "secret", "key", "token", "code", "otp", "phone", "email", "user", "account", "invoice", "pay", "order", "refund", "balance"]) {
                    sensitive_candidates.push(format!(
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

    summary.push_str("\nAuthentication-related candidates:\n");
    append_limited(&mut summary, &auth_candidates, 20);

    summary.push_str("\nAdmin/internal candidates:\n");
    append_limited(&mut summary, &admin_candidates, 20);

    summary.push_str("\nSensitive or high-value endpoint candidates:\n");
    append_limited(&mut summary, &sensitive_candidates, 25);

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

fn format_findings(findings: &[(String, String)]) -> String {
    let mut out = String::new();
    for (title, content) in findings {
        out.push_str(&format!("## {}\n{}\n\n", title, content));
    }
    out
}

async fn chat_with_history(
    history: &mut Vec<ChatMessage>,
    system_prompt: &str,
    user_content: String,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String, String> {
    let mut messages = history.clone();
    messages.push(ChatMessage {
        role: "system".to_string(),
        content: system_prompt.to_string(),
    });
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_content.clone(),
    });

    let request = ChatRequest {
        model: model.to_string(),
        messages,
        stream: false,
    };

    let content = send_chat_request(&request, api_key, api_url).await?;

    history.push(ChatMessage {
        role: "user".to_string(),
        content: user_content,
    });
    history.push(ChatMessage {
        role: "assistant".to_string(),
        content: content.clone(),
    });

    Ok(content)
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

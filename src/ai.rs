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

#[derive(Serialize)]
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
    let graph_json = serde_json::to_string(graph)
        .map_err(|e| format!("Failed to serialize graph: {e}"))?;

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
                    "Analyze the following business graph JSON extracted from network traffic.\n\
                     The graph contains BusinessFunction nodes (grouped by host+path prefix), \
                     Endpoint nodes (HTTP endpoints), and edges (contains, calls_after, data_dependency).\n\
                     \n\
                     Identify:\n\
                     1. What business functions does this application serve?\n\
                     2. What's the user flow / navigation pattern?\n\
                     3. Potential security issues (exposed admin endpoints, missing auth, IDOR, sensitive data leak, etc.)\n\
                     4. High-value targets for manual penetration testing\n\
                     5. Concrete next steps for deeper testing\n\
                     \n\
                     Respond in Markdown.\n\
                     \n\
                     ```json\n{graph_json}\n```"
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

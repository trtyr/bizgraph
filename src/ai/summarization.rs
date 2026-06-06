use std::collections::BTreeMap;

use super::prompts::{MAX_ENDPOINTS_PER_DOMAIN, SUMMARY_HARD_CHAR_LIMIT};
use crate::types::{
    BusinessEdge, BusinessGraph, BusinessNode, BusinessNodeKind, BusinessNodeProperties,
    EndpointProperties, ParameterDescriptor,
};

pub fn build_graph_summary(graph: &BusinessGraph) -> String {
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

                // Include auth headers if detected
                for hdr in &details.request_headers {
                    let lower = hdr.to_lowercase();
                    if lower.starts_with("authorization")
                        || lower.starts_with("cookie")
                        || lower.starts_with("x-api-key")
                        || lower.starts_with("x-token")
                    {
                        lines.push(format!("  auth: {}", hdr));
                        break;
                    }
                }
                // Include response body sample if available
                if let Some(body) = details.response_bodies.first() {
                    let truncated = if body.len() > 500 {
                        format!("{}...", &body[..500])
                    } else {
                        body.clone()
                    };
                    lines.push(format!("  response_sample: {}", truncated));
                }

                if looks_interesting(
                    &node.label,
                    &details.path_template,
                    &[
                        "login", "auth", "token", "session", "oauth", "signin", "signup",
                    ],
                ) {
                    identity_candidates.push(format!(
                        "{} [{}] params={} statuses=[{}]",
                        node.label, methods, parameters, statuses
                    ));
                }
                if looks_interesting(
                    &node.label,
                    &details.path_template,
                    &[
                        "admin",
                        "internal",
                        "manage",
                        "console",
                        "backoffice",
                        "root",
                    ],
                ) {
                    operator_candidates.push(format!(
                        "{} [{}] statuses=[{}]",
                        node.label, methods, statuses
                    ));
                }
                if has_sensitive_parameters(&details.parameters)
                    || looks_interesting(
                        &node.label,
                        &details.path_template,
                        &[
                            "password", "secret", "key", "token", "code", "otp", "phone", "email",
                            "user", "account", "invoice", "pay", "order", "refund", "balance",
                        ],
                    )
                {
                    data_rich_candidates.push(format!(
                        "{} [{}] params={} request_schema={} response_schema={}",
                        node.label, methods, parameters, request_schema, response_schema
                    ));
                }
                if details.status_profiles.client_error > 0
                    || details.status_profiles.server_error > 0
                {
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
    for line in &lines {
        summary.push_str(line);
        summary.push('\n');
        if summary.len() > SUMMARY_HARD_CHAR_LIMIT {
            summary.push_str(&format!(
                "\n[truncated — {} total lines, showing first portion to stay within context limit]\n",
                lines.len()
            ));
            break;
        }
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

/// Build a compact summary of only the subgraph relevant to a domain prefix.
/// Used by Phase 2 domain deep-dives to send focused context instead of the full graph.
pub fn build_scoped_subgraph_summary(graph: &BusinessGraph, domain_prefix: &str) -> String {
    let prefix_lower = domain_prefix.to_lowercase();

    // Collect endpoint node IDs that match this domain
    let mut matched_endpoint_ids = std::collections::BTreeSet::new();
    let mut matched_function_ids = std::collections::BTreeSet::new();
    let mut matched_host_ids = std::collections::BTreeSet::new();

    for node in &graph.nodes {
        match &node.properties {
            BusinessNodeProperties::Endpoint(details) => {
                let path_match = details.path_template.to_lowercase().contains(&prefix_lower);
                let label_match = node.label.to_lowercase().contains(&prefix_lower);
                if path_match || label_match {
                    matched_endpoint_ids.insert(node.id);
                }
            }
            BusinessNodeProperties::BusinessFunction(details) => {
                if details.path_prefix.to_lowercase().contains(&prefix_lower)
                    || details.host.to_lowercase().contains(&prefix_lower)
                    || node.label.to_lowercase().contains(&prefix_lower)
                {
                    matched_function_ids.insert(node.id);
                }
            }
            BusinessNodeProperties::Host(details) => {
                if node.label.to_lowercase().contains(&prefix_lower)
                    || details.keys().any(|k| k.to_lowercase().contains(&prefix_lower))
                {
                    matched_host_ids.insert(node.id);
                }
            }
        }
    }

    // Walk contains edges to find parent functions/hosts of matched endpoints
    for edge in &graph.edges {
        if edge.label == "contains" {
            if matched_endpoint_ids.contains(&edge.target_node_id) {
                matched_function_ids.insert(edge.source_node_id);
            }
            if matched_function_ids.contains(&edge.target_node_id) {
                matched_host_ids.insert(edge.source_node_id);
            }
        }
    }

    let all_matched: std::collections::BTreeSet<_> = matched_endpoint_ids
        .iter()
        .chain(matched_function_ids.iter())
        .chain(matched_host_ids.iter())
        .copied()
        .collect();

    if all_matched.is_empty() {
        return format!("No nodes matched domain prefix '{}'.", domain_prefix);
    }

    // Build endpoint table
    let mut endpoint_rows = Vec::new();
    for node in &graph.nodes {
        if !matched_endpoint_ids.contains(&node.id) {
            continue;
        }
        if let BusinessNodeProperties::Endpoint(details) = &node.properties {
            let methods = details.methods.join(", ");
            let params = summarize_parameters(&details.parameters);
            let desc = details
                .normalization_notes
                .first()
                .map(|s| s.as_str())
                .unwrap_or("");
            endpoint_rows.push(format!(
                "| {} | {} | {} | {} | {} | {:.2} |",
                node.label, details.path_template, methods, params, desc, details.confidence
            ));
            // Add auth header if detected
            for hdr in &details.request_headers {
                let lower = hdr.to_lowercase();
                if lower.starts_with("authorization")
                    || lower.starts_with("cookie")
                    || lower.starts_with("x-api-key")
                {
                    endpoint_rows.push(format!("  auth: {}", hdr));
                    break;
                }
            }
            // Add response body sample
            if let Some(body) = details.response_bodies.first() {
                let truncated = if body.len() > 300 {
                    format!("{}...", &body[..300])
                } else {
                    body.clone()
                };
                endpoint_rows.push(format!("  response: {}", truncated));
            }
            if endpoint_rows.len() >= MAX_ENDPOINTS_PER_DOMAIN {
                endpoint_rows.push("| ... | (truncated) | | | | |".to_string());
                break;
            }
        }
    }

    // Build function list
    let mut function_rows = Vec::new();
    for node in &graph.nodes {
        if !matched_function_ids.contains(&node.id) {
            continue;
        }
        if let BusinessNodeProperties::BusinessFunction(details) = &node.properties {
            function_rows.push(format!(
                "- {} | host={} | prefix={} | endpoints={}",
                node.label, details.host, details.path_prefix, details.endpoint_count
            ));
        }
    }

    // Build filtered edges (only between matched nodes)
    let mut edge_rows = Vec::new();
    let id_to_label: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .map(|n| (n.id, n.label.clone()))
        .collect();
    for edge in &graph.edges {
        if all_matched.contains(&edge.source_node_id)
            && all_matched.contains(&edge.target_node_id)
        {
            let from = id_to_label.get(&edge.source_node_id).map(String::as_str).unwrap_or("?");
            let to = id_to_label.get(&edge.target_node_id).map(String::as_str).unwrap_or("?");
            edge_rows.push(format!("| {} | {} | {} |", from, to, edge.label));
            if edge_rows.len() >= 50 {
                edge_rows.push("| ... | ... | (truncated) |".to_string());
                break;
            }
        }
    }

    // Assemble
    let mut summary = String::new();
    summary.push_str(&format!("Domain scope: {}\n", domain_prefix));
    summary.push_str(&format!(
        "Nodes: {} endpoints, {} business functions, {} hosts\n\n",
        matched_endpoint_ids.len(), matched_function_ids.len(), matched_host_ids.len()
    ));

    if !function_rows.is_empty() {
        summary.push_str("Business functions:\n");
        for row in &function_rows {
            summary.push_str(row);
            summary.push('\n');
        }
        summary.push('\n');
    }

    if !endpoint_rows.is_empty() {
        summary.push_str("Endpoints:\n");
        summary.push_str("| Label | Path | Methods | Params | Description | Confidence |\n");
        summary.push_str("|-------|------|---------|--------|-------------|------------|\n");
        for row in &endpoint_rows {
            summary.push_str(row);
            summary.push('\n');
        }
        summary.push('\n');
    }

    if !edge_rows.is_empty() {
        summary.push_str("Relationships:\n");
        summary.push_str("| From | To | Type |\n");
        summary.push_str("|------|-----|------|\n");
        for row in &edge_rows {
            summary.push_str(row);
            summary.push('\n');
        }
    }

    summary
}

pub fn build_graph_overview(graph: &BusinessGraph) -> String {
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

pub fn build_function_detail(function_name: &str, graph: &BusinessGraph) -> String {
    let function_node = graph.nodes.iter().find(|node| {
        node.kind == BusinessNodeKind::BusinessFunction && node.label == function_name
    });

    let Some(function_node) = function_node else {
        return format!("Business module '{}' not found in graph.", function_name);
    };

    let BusinessNodeProperties::BusinessFunction(function_details) = &function_node.properties
    else {
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
                if details
                    .path_template
                    .starts_with(&function_details.path_prefix)
                {
                    endpoint_ids.push(node.id);
                }
            }
        }
    }

    endpoint_ids.sort();
    endpoint_ids.dedup();

    // Prioritize endpoints by richness: parameters, schemas, error activity
    endpoint_ids.sort_by(|a, b| {
        let score = |id: &uuid::Uuid| -> usize {
            endpoint_by_id.get(id).map(|(_, d)| {
                let mut s = d.parameters.len() * 2;
                if d.request_schema.is_some() { s += 3; }
                if d.response_schema.is_some() { s += 3; }
                s += d.status_profiles.success.min(10);
                s += d.status_profiles.client_error.min(5) * 2;
                s
            }).unwrap_or(0)
        };
        score(b).cmp(&score(a))
    });

    let total_endpoints = endpoint_ids.len();
    let truncated = total_endpoints > MAX_ENDPOINTS_PER_DOMAIN;
    endpoint_ids.truncate(MAX_ENDPOINTS_PER_DOMAIN);

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
        let request_schema = summarize_schema_presence(endpoint_details.request_schema.is_some());
        let response_schema = summarize_schema_presence(endpoint_details.response_schema.is_some());
        let call_sequences =
            build_endpoint_call_sequences(*endpoint_id, &endpoint_by_id, graph, &endpoint_set);

        endpoint_sections.push(format!(
            "### {}\n- path: {}\n- methods: [{}]\n- status_codes: [{}]\n- parameters: {}\n- request_schema: {}\n- response_schema: {}\n- call_sequences (max 5):\n{}\n- normalization_notes: {}\n- confidence: {:.2}",
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
    if truncated {
        lines.push(format!(
            "\n[Showing top {} of {} endpoints — prioritized by parameter richness and schema coverage]",
            MAX_ENDPOINTS_PER_DOMAIN, total_endpoints
        ));
    }
    lines.join("\n")
}

pub fn build_cross_domain_summary(graph: &BusinessGraph) -> String {
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
            .push(format!(
                "{} -> {} via {}",
                source_endpoint, target_endpoint, edge.label
            ));
    }

    let mut summary = String::new();
    summary.push_str("Cross-domain topology\n\nBusiness functions:\n");
    for (node, details) in function_by_id.values() {
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

pub fn count_kind(graph: &BusinessGraph, kind: BusinessNodeKind) -> usize {
    graph.nodes.iter().filter(|node| node.kind == kind).count()
}

pub fn collect_edge_summary(
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

pub fn summarize_parameters(parameters: &[ParameterDescriptor]) -> String {
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

pub fn summarize_schema_presence(present: bool) -> &'static str {
    if present {
        "present"
    } else {
        "none"
    }
}

pub fn has_sensitive_parameters(parameters: &[ParameterDescriptor]) -> bool {
    parameters.iter().any(|parameter| {
        let name = parameter.name.to_ascii_lowercase();
        [
            "token", "password", "secret", "key", "code", "otp", "auth", "session", "email",
            "phone", "user", "account", "amount", "price", "balance", "order", "refund", "coupon",
        ]
        .iter()
        .any(|needle| name.contains(needle))
    })
}

pub fn looks_interesting(label: &str, path: &str, needles: &[&str]) -> bool {
    let haystack = format!(
        "{} {}",
        label.to_ascii_lowercase(),
        path.to_ascii_lowercase()
    );
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

        if let Some(function) = functions
            .iter()
            .find(|function| match &function.properties {
                BusinessNodeProperties::BusinessFunction(function_details) => details
                    .path_template
                    .starts_with(&function_details.path_prefix),
                _ => false,
            })
        {
            mapping.insert(node.id, function.id);
        }
    }

    mapping
}

fn build_endpoint_call_sequences(
    endpoint_id: uuid::Uuid,
    endpoint_by_id: &BTreeMap<uuid::Uuid, (&BusinessNode, &EndpointProperties)>,
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
    sequences.truncate(5);
    sequences
}

fn format_sequence_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        "- none observed".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn prioritized_function_names(graph: &BusinessGraph, overview_response: &str) -> Vec<String> {
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
    prioritized
        .into_iter()
        .take(5)
        .map(|(label, _)| label)
        .collect()
}

pub fn summarize_text(text: &str, limit: usize) -> String {
    let bullets = extract_key_bullets(text, 4);
    if bullets.is_empty() {
        soft_limit_text(text, limit)
    } else {
        soft_limit_text(&bullets.join(" | "), limit)
    }
}

pub fn extract_key_bullets(text: &str, limit: usize) -> Vec<String> {
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

pub fn parse_observations_from_response(response: &str) -> Vec<super::agent::BusinessObservation> {
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

        observations.push(super::agent::BusinessObservation {
            title,
            evidence,
            endpoints,
            notes,
        });
    }

    observations
}

pub fn extract_cross_cutting_items(response: &str) -> Vec<String> {
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
    items.truncate(super::CROSS_CUTTING_LIMIT);
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
        "Additional business context inferred from the surrounding traffic and workflow structure."
            .to_string()
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

pub fn soft_limit_text(text: &str, limit: usize) -> String {
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

fn push_unique(items: &mut Vec<String>, value: String) {
    if !value.is_empty() && !items.iter().any(|item| item == &value) {
        items.push(value);
    }
}

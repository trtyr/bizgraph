use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::json;

use crate::{Error, Result};
use crate::ai::BusinessIdentification;
use crate::parser::TrafficRow;
use crate::types::{
    deterministic_id, BusinessEdge, BusinessFunctionProperties, BusinessGraph, BusinessNode,
    BusinessNodeKind, BusinessNodeProperties, EndpointProperties, ParameterDescriptor,
    ParameterKind, ParameterLocation, SchemaShape, SchemaType, StatusProfiles,
};

/// Extensions of static resources to exclude from graph building.
const STATIC_EXTENSIONS: &[&str] = &[
    "js", "css", "png", "jpg", "jpeg", "gif", "svg", "ico", "woff", "woff2", "ttf", "eot",
    "otf", "mp4", "mp3", "webp", "avif",
];

/// Returns true if the path looks like a static resource that should be excluded
/// from business analysis. Keeps `.map`, `.json`, `.ashx`, `.aspx` files.
pub fn is_static_resource(path: &str) -> bool {
    // Skip /cdn/ prefix paths
    if path.starts_with("/cdn/") {
        return true;
    }

    // Check file extension
    if let Some(dot_pos) = path.rfind('.') {
        let ext = &path[dot_pos + 1..];
        // Keep .map (source maps) and .json (API responses)
        if ext == "map" || ext == "json" {
            return false;
        }
        // Keep .ashx and .aspx (server handlers)
        if ext == "ashx" || ext == "aspx" {
            return false;
        }
        return STATIC_EXTENSIONS.contains(&ext);
    }

    false
}

pub fn build_business_graph(rows: &[TrafficRow]) -> Result<BusinessGraph> {
    if rows.is_empty() {
        return Ok(BusinessGraph::default());
    }

    // Filter out static resources (JS, CSS, images, fonts, etc.)
    let rows: Vec<&TrafficRow> = rows
        .iter()
        .filter(|row| !is_static_resource(&row.path))
        .collect();

    if rows.is_empty() {
        return Ok(BusinessGraph::default());
    }

    let mut endpoint_state: BTreeMap<String, EndpointAccumulator> = BTreeMap::new();
    let mut business_functions: BTreeMap<String, BusinessFunctionAccumulator> = BTreeMap::new();
    let mut sequence: Vec<String> = Vec::new();
    let mut response_candidates_by_endpoint: BTreeMap<String, Vec<(String, String)>> =
        BTreeMap::new();

    for row in rows {
        let normalized_path = normalize_path_template(&row.path);
        let endpoint_key = format!("ep:{}:{}:{}", row.method, row.host, normalized_path);
        endpoint_state
            .entry(endpoint_key.clone())
            .or_insert_with(|| EndpointAccumulator::new(&row.host, &row.method, &normalized_path))
            .observe(row, &normalized_path);
        sequence.push(endpoint_key.clone());

        let path_prefix = business_path_prefix(&normalized_path);
        let business_key = format!("bf:{}:{}", row.host, path_prefix);
        business_functions
            .entry(business_key.clone())
            .or_insert_with(|| BusinessFunctionAccumulator::new(&row.host, &path_prefix))
            .endpoint_keys
            .insert(endpoint_key.clone());

        let candidates = extract_response_candidates(&row.response_body);
        if !candidates.is_empty() {
            response_candidates_by_endpoint
                .entry(endpoint_key)
                .or_default()
                .extend(candidates);
        }
    }

    let mut nodes = Vec::new();
    let mut node_ids = HashMap::new();

    for (stable_key, business) in &business_functions {
        let node = BusinessNode {
            id: deterministic_id(stable_key),
            stable_key: stable_key.clone(),
            label: format!("{} {}", business.host, business.path_prefix),
            kind: BusinessNodeKind::BusinessFunction,
            properties: BusinessNodeProperties::BusinessFunction(BusinessFunctionProperties {
                host: business.host.clone(),
                path_prefix: business.path_prefix.clone(),
                endpoint_count: business.endpoint_keys.len(),
                description: None,
            }),
        };
        node_ids.insert(stable_key.clone(), node.id);
        nodes.push(node);
    }

    for (stable_key, endpoint) in &endpoint_state {
        let node = BusinessNode {
            id: deterministic_id(stable_key),
            stable_key: stable_key.clone(),
            label: format!(
                "{} {}{}",
                endpoint.primary_method, endpoint.host, endpoint.path_template
            ),
            kind: BusinessNodeKind::Endpoint,
            properties: BusinessNodeProperties::Endpoint(endpoint.to_properties()),
        };
        node_ids.insert(stable_key.clone(), node.id);
        nodes.push(node);
    }

    nodes.sort_by(|left, right| left.stable_key.cmp(&right.stable_key));

    let mut edges = Vec::new();
    let mut edge_seen = HashSet::new();

    for (business_key, business) in &business_functions {
        for endpoint_key in &business.endpoint_keys {
            let edge_key = format!("contains:{business_key}:{endpoint_key}");
            if edge_seen.insert(edge_key.clone()) {
                edges.push(BusinessEdge {
                    id: deterministic_id(&edge_key),
                    source_node_id: *node_ids.get(business_key).ok_or_else(|| Error::MissingNode {
                        kind: "business function",
                        key: business_key.clone(),
                    })?,
                    target_node_id: *node_ids
                        .get(endpoint_key)
                        .ok_or_else(|| Error::MissingNode {
                            kind: "endpoint",
                            key: endpoint_key.clone(),
                        })?,
                    label: "contains".to_string(),
                    properties: json!({}),
                });
            }
        }
    }

    for window in sequence.windows(2) {
        let [source_key, target_key] = window else {
            continue;
        };
        if source_key == target_key {
            continue;
        }

        let edge_key = format!("calls_after:{source_key}:{target_key}");
        if edge_seen.insert(edge_key.clone()) {
            edges.push(BusinessEdge {
                id: deterministic_id(&edge_key),
                source_node_id: *node_ids
                    .get(source_key)
                    .ok_or_else(|| Error::MissingNode {
                        kind: "sequence source",
                        key: source_key.clone(),
                    })?,
                target_node_id: *node_ids
                    .get(target_key)
                    .ok_or_else(|| Error::MissingNode {
                        kind: "sequence target",
                        key: target_key.clone(),
                    })?,
                label: "calls_after".to_string(),
                properties: json!({}),
            });
        }
    }

    for (index, current_key) in sequence.iter().enumerate() {
        let current = endpoint_state
            .get(current_key)
            .ok_or_else(|| Error::MissingNode {
                kind: "endpoint accumulator",
                key: current_key.clone(),
            })?;
        let haystack = current.request_haystack();

        for previous_key in &sequence[..index] {
            let Some(candidates) = response_candidates_by_endpoint.get(previous_key) else {
                continue;
            };

            for (field, value) in candidates {
                if value.len() < 3 || !haystack.contains(value) {
                    continue;
                }

                let label = format!("data_dependency:{field}");
                let edge_key = format!("{label}:{previous_key}:{current_key}");
                if edge_seen.insert(edge_key.clone()) {
                    edges.push(BusinessEdge {
                        id: deterministic_id(&edge_key),
                        source_node_id: *node_ids.get(previous_key).ok_or_else(|| Error::MissingNode {
                            kind: "dependency source",
                            key: previous_key.clone(),
                        })?,
                        target_node_id: *node_ids.get(current_key).ok_or_else(|| Error::MissingNode {
                            kind: "dependency target",
                            key: current_key.clone(),
                        })?,
                        label,
                        properties: json!({ "matched_value_length": value.len() }),
                    });
                }
            }
        }
    }

    edges.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then_with(|| left.source_node_id.cmp(&right.source_node_id))
            .then_with(|| left.target_node_id.cmp(&right.target_node_id))
    });

    Ok(BusinessGraph { nodes, edges })
}

/// Build a business graph using AI-identified business functions instead of URL prefix grouping.
pub fn build_business_graph_from_ai(
    rows: &[TrafficRow],
    identification: &BusinessIdentification,
) -> Result<BusinessGraph> {
    if rows.is_empty() {
        return Ok(BusinessGraph::default());
    }

    // 1. Accumulate endpoint data (same as build_business_graph)
    let mut endpoint_state: BTreeMap<String, EndpointAccumulator> = BTreeMap::new();
    let mut sequence: Vec<String> = Vec::new();
    let mut response_candidates_by_endpoint: BTreeMap<String, Vec<(String, String)>> =
        BTreeMap::new();

    for row in rows {
        let normalized_path = normalize_path_template(&row.path);
        let endpoint_key = format!("ep:{}:{}:{}", row.method, row.host, normalized_path);
        endpoint_state
            .entry(endpoint_key.clone())
            .or_insert_with(|| EndpointAccumulator::new(&row.host, &row.method, &normalized_path))
            .observe(row, &normalized_path);
        sequence.push(endpoint_key.clone());

        let candidates = extract_response_candidates(&row.response_body);
        if !candidates.is_empty() {
            response_candidates_by_endpoint
                .entry(endpoint_key)
                .or_default()
                .extend(candidates);
        }
    }

    // 2. Build business functions from AI identification
    let mut business_functions: BTreeMap<String, BusinessFunctionAccumulator> = BTreeMap::new();

    for group in &identification.business_functions {
        let business_key = format!("bf:{}", group.name);
        let acc = business_functions
            .entry(business_key.clone())
            .or_insert_with(|| BusinessFunctionAccumulator::new("", &group.name));
        acc.description = Some(group.description.clone());

        for ep in &group.endpoints {
            let normalized_path = normalize_path_template(&ep.path);
            let endpoint_key = format!("ep:{}:{}:{}", ep.method, ep.host, normalized_path);
            acc.endpoint_keys.insert(endpoint_key);
        }
    }

    // 3. Create nodes
    let mut nodes = Vec::new();
    let mut node_ids = HashMap::new();

    for (stable_key, business) in &business_functions {
        let node = BusinessNode {
            id: deterministic_id(stable_key),
            stable_key: stable_key.clone(),
            label: business.path_prefix.clone(), // business name
            kind: BusinessNodeKind::BusinessFunction,
            properties: BusinessNodeProperties::BusinessFunction(BusinessFunctionProperties {
                host: business.host.clone(),
                path_prefix: business.path_prefix.clone(),
                endpoint_count: business.endpoint_keys.len(),
                description: business.description.clone(),
            }),
        };
        node_ids.insert(stable_key.clone(), node.id);
        nodes.push(node);
    }

    for (stable_key, endpoint) in &endpoint_state {
        let node = BusinessNode {
            id: deterministic_id(stable_key),
            stable_key: stable_key.clone(),
            label: format!(
                "{} {}{}",
                endpoint.primary_method, endpoint.host, endpoint.path_template
            ),
            kind: BusinessNodeKind::Endpoint,
            properties: BusinessNodeProperties::Endpoint(endpoint.to_properties()),
        };
        node_ids.insert(stable_key.clone(), node.id);
        nodes.push(node);
    }

    nodes.sort_by(|left, right| left.stable_key.cmp(&right.stable_key));

    // 4. Create edges
    let mut edges = Vec::new();
    let mut edge_seen = HashSet::new();

    // contains edges: business function → endpoint
    for (business_key, business) in &business_functions {
        for endpoint_key in &business.endpoint_keys {
            if !node_ids.contains_key(endpoint_key) {
                continue; // endpoint not in this traffic capture
            }
            let edge_key = format!("contains:{business_key}:{endpoint_key}");
            if edge_seen.insert(edge_key.clone()) {
                edges.push(BusinessEdge {
                    id: deterministic_id(&edge_key),
                    source_node_id: *node_ids.get(business_key).ok_or_else(|| Error::MissingNode {
                        kind: "business function",
                        key: business_key.clone(),
                    })?,
                    target_node_id: *node_ids
                        .get(endpoint_key)
                        .ok_or_else(|| Error::MissingNode {
                            kind: "endpoint",
                            key: endpoint_key.clone(),
                        })?,
                    label: "contains".to_string(),
                    properties: json!({}),
                });
            }
        }
    }

    // calls_after edges (same logic as build_business_graph)
    for window in sequence.windows(2) {
        let [source_key, target_key] = window else {
            continue;
        };
        if source_key == target_key {
            continue;
        }

        let edge_key = format!("calls_after:{source_key}:{target_key}");
        if edge_seen.insert(edge_key.clone()) {
            edges.push(BusinessEdge {
                id: deterministic_id(&edge_key),
                source_node_id: *node_ids
                    .get(source_key)
                    .ok_or_else(|| Error::MissingNode {
                        kind: "sequence source",
                        key: source_key.clone(),
                    })?,
                target_node_id: *node_ids
                    .get(target_key)
                    .ok_or_else(|| Error::MissingNode {
                        kind: "sequence target",
                        key: target_key.clone(),
                    })?,
                label: "calls_after".to_string(),
                properties: json!({}),
            });
        }
    }

    // data_dependency edges (same logic as build_business_graph)
    for (index, current_key) in sequence.iter().enumerate() {
        let current = endpoint_state
            .get(current_key)
            .ok_or_else(|| Error::MissingNode {
                kind: "endpoint accumulator",
                key: current_key.clone(),
            })?;
        let haystack = current.request_haystack();

        for previous_key in &sequence[..index] {
            let Some(candidates) = response_candidates_by_endpoint.get(previous_key) else {
                continue;
            };

            for (field, value) in candidates {
                if value.len() < 3 || !haystack.contains(value) {
                    continue;
                }

                let label = format!("data_dependency:{field}");
                let edge_key = format!("{label}:{previous_key}:{current_key}");
                if edge_seen.insert(edge_key.clone()) {
                    edges.push(BusinessEdge {
                        id: deterministic_id(&edge_key),
                        source_node_id: *node_ids.get(previous_key).ok_or_else(|| Error::MissingNode {
                            kind: "dependency source",
                            key: previous_key.clone(),
                        })?,
                        target_node_id: *node_ids.get(current_key).ok_or_else(|| Error::MissingNode {
                            kind: "dependency target",
                            key: current_key.clone(),
                        })?,
                        label,
                        properties: json!({ "matched_value_length": value.len() }),
                    });
                }
            }
        }
    }

    edges.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then_with(|| left.source_node_id.cmp(&right.source_node_id))
            .then_with(|| left.target_node_id.cmp(&right.target_node_id))
    });

    Ok(BusinessGraph { nodes, edges })
}

#[derive(Debug, Default)]
struct BusinessFunctionAccumulator {
    host: String,
    path_prefix: String,
    endpoint_keys: BTreeSet<String>,
    description: Option<String>,
}

impl BusinessFunctionAccumulator {
    fn new(host: &str, path_prefix: &str) -> Self {
        Self {
            host: host.to_string(),
            path_prefix: path_prefix.to_string(),
            endpoint_keys: BTreeSet::new(),
            description: None,
        }
    }
}

#[derive(Debug, Default)]
struct EndpointAccumulator {
    host: String,
    path_template: String,
    primary_method: String,
    methods: BTreeSet<String>,
    status_codes: BTreeSet<u16>,
    request_schema: Option<SchemaShape>,
    response_schema: Option<SchemaShape>,
    parameters: BTreeMap<String, ParameterAccumulator>,
    status_profiles: StatusProfiles,
    normalization_notes: BTreeSet<String>,
    observation_count: usize,
    request_samples: Vec<String>,
    request_header_samples: Vec<String>,
    response_body_samples: Vec<String>,
    query_samples: Vec<String>,
}

impl EndpointAccumulator {
    fn new(host: &str, method: &str, path_template: &str) -> Self {
        Self {
            host: host.to_string(),
            path_template: path_template.to_string(),
            primary_method: method.to_string(),
            ..Self::default()
        }
    }

    fn observe(&mut self, row: &TrafficRow, normalized_path: &str) {
        self.observation_count += 1;
        self.methods.insert(row.method.clone());

        if let Some(status_code) = row.status_code {
            self.status_codes.insert(status_code);
            match status_code / 100 {
                2 => self.status_profiles.success += 1,
                3 => self.status_profiles.redirect += 1,
                4 => self.status_profiles.client_error += 1,
                5 => self.status_profiles.server_error += 1,
                _ => self.status_profiles.other.push(status_code.to_string()),
            }
        }

        if self.request_schema.is_none() {
            self.request_schema = extract_schema_shape(&row.request_body);
        }
        if self.response_schema.is_none() {
            self.response_schema = extract_schema_shape(&row.response_body);
        }

        merge_parameters(
            &mut self.parameters,
            normalized_path,
            row.query.as_deref(),
            &row.request_body,
        );

        if normalized_path != row.path {
            self.normalization_notes.insert(format!(
                "normalized path '{}' to '{}' for deterministic endpoint grouping",
                row.path, normalized_path
            ));
        } else {
            self.normalization_notes
                .insert("path required no normalization".to_string());
        }

        if !row.content_type.is_empty() {
            self.normalization_notes
                .insert(format!("observed content type '{}'", row.content_type));
        }
        if let Some(port) = row.port {
            self.normalization_notes
                .insert(format!("observed port {}", port));
        }
        if !row.response_headers.is_empty() {
            self.normalization_notes
                .insert("response headers observed and ignored for schema extraction".to_string());
        }
        self.normalization_notes.insert(format!(
            "source row indices aggregated into this endpoint include {}",
            row.row_index
        ));

        if !row.request_body.is_empty() && self.request_samples.len() < 3 {
            self.request_samples.push(row.request_body.clone());
        }
        if !row.request_headers.is_empty() && self.request_header_samples.len() < 3 {
            self.request_header_samples
                .push(row.request_headers.clone());
        }
        if !row.response_body.is_empty() && self.response_body_samples.len() < 3 {
            let truncated = if row.response_body.len() > 2000 {
                format!("{}...[truncated]", &row.response_body[..2000])
            } else {
                row.response_body.clone()
            };
            self.response_body_samples.push(truncated);
        }
        if let Some(query) = &row.query {
            if !query.is_empty() && self.query_samples.len() < 3 {
                self.query_samples.push(query.clone());
            }
        }
    }

    fn to_properties(&self) -> EndpointProperties {
        let mut methods: Vec<String> = self.methods.iter().cloned().collect();
        methods.sort();

        let mut status_codes: Vec<u16> = self.status_codes.iter().copied().collect();
        status_codes.sort_unstable();

        EndpointProperties {
            path_template: self.path_template.clone(),
            methods,
            status_codes,
            request_schema: self.request_schema.clone(),
            response_schema: self.response_schema.clone(),
            parameters: self
                .parameters
                .values()
                .map(ParameterAccumulator::to_descriptor)
                .collect(),
            status_profiles: self.status_profiles.clone(),
            confidence: calculate_confidence(self),
            normalization_notes: self.normalization_notes.iter().cloned().collect(),
            request_headers: self.request_header_samples.clone(),
            response_bodies: self.response_body_samples.clone(),
        }
    }

    fn request_haystack(&self) -> String {
        let mut haystack = String::new();
        for value in &self.request_samples {
            haystack.push_str(value);
            haystack.push('\n');
        }
        for value in &self.request_header_samples {
            haystack.push_str(value);
            haystack.push('\n');
        }
        for value in &self.query_samples {
            haystack.push_str(value);
            haystack.push('\n');
        }
        haystack
    }
}

#[derive(Debug, Clone)]
struct ParameterAccumulator {
    name: String,
    location: ParameterLocation,
    kind: ParameterKind,
    occurrence_count: usize,
}

impl ParameterAccumulator {
    fn new(name: String, location: ParameterLocation, kind: ParameterKind) -> Self {
        Self {
            name,
            location,
            kind,
            occurrence_count: 0,
        }
    }

    fn observe(&mut self, kind: ParameterKind) {
        self.occurrence_count += 1;
        self.kind = choose_parameter_kind(&self.kind, &kind);
    }

    fn to_descriptor(&self) -> ParameterDescriptor {
        ParameterDescriptor {
            name: self.name.clone(),
            location: self.location.clone(),
            kind: self.kind.clone(),
            occurrence_count: self.occurrence_count,
        }
    }
}

pub fn normalize_path_template(path: &str) -> String {
    let path_without_query = path.split('?').next().unwrap_or(path);
    let segments: Vec<String> = path_without_query
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(normalize_path_segment)
        .collect();

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn normalize_path_segment(segment: &str) -> String {
    if let Some((stem, extension)) = split_extension(segment) {
        if is_all_digits(stem) {
            return format!(":id.{extension}");
        }
        if is_uuid_like(stem) {
            return format!(":uuid.{extension}");
        }
        if is_hash_like(stem) {
            return format!(":param.{extension}");
        }
    }

    if is_all_digits(segment) {
        ":id".to_string()
    } else if is_uuid_like(segment) {
        ":uuid".to_string()
    } else if is_hash_like(segment) {
        ":param".to_string()
    } else {
        segment.to_string()
    }
}

fn split_extension(segment: &str) -> Option<(&str, &str)> {
    let (stem, extension) = segment.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() {
        return None;
    }
    if !extension
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        return None;
    }
    Some((stem, extension))
}

fn is_all_digits(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|character| character.is_ascii_digit())
}

fn is_uuid_like(value: &str) -> bool {
    let compact: String = value
        .chars()
        .filter(|character| *character != '-')
        .collect();
    (32..=36).contains(&compact.len())
        && compact
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn is_hash_like(value: &str) -> bool {
    if value.len() < 8 {
        return false;
    }

    if value.chars().all(|character| character.is_ascii_hexdigit()) {
        return true;
    }

    value.len() >= 16
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn business_path_prefix(normalized_path: &str) -> String {
    let mut segments = Vec::new();
    for segment in normalized_path
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        segments.push(segment);
        if !segment.starts_with(':') {
            break;
        }
    }

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn merge_parameters(
    parameters: &mut BTreeMap<String, ParameterAccumulator>,
    path_template: &str,
    query: Option<&str>,
    request_body: &str,
) {
    for segment in path_template
        .split('/')
        .filter(|segment| segment.starts_with(':'))
    {
        let name = segment
            .trim_start_matches(':')
            .split('.')
            .next()
            .unwrap_or(segment)
            .to_string();
        let kind = path_parameter_kind(segment);
        parameters
            .entry(format!("path:{name}"))
            .or_insert_with(|| {
                ParameterAccumulator::new(name.clone(), ParameterLocation::Path, kind.clone())
            })
            .observe(kind);
    }

    if let Some(query) = query {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            if name.is_empty() {
                continue;
            }
            let kind = infer_value_kind(value);
            parameters
                .entry(format!("query:{name}"))
                .or_insert_with(|| {
                    ParameterAccumulator::new(
                        name.to_string(),
                        ParameterLocation::Query,
                        kind.clone(),
                    )
                })
                .observe(kind);
        }
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(request_body) {
        if let Some(object) = value.as_object() {
            for (name, value) in object {
                let kind = parameter_kind_from_json(value);
                parameters
                    .entry(format!("body:{name}"))
                    .or_insert_with(|| {
                        ParameterAccumulator::new(
                            name.clone(),
                            ParameterLocation::Body,
                            kind.clone(),
                        )
                    })
                    .observe(kind);
            }
        }
    }
}

fn path_parameter_kind(segment: &str) -> ParameterKind {
    if segment.starts_with(":id") {
        ParameterKind::NumericId
    } else if segment.starts_with(":uuid") {
        ParameterKind::Uuid
    } else if segment.starts_with(":param") {
        ParameterKind::DynamicSegment
    } else {
        ParameterKind::Unknown
    }
}

fn infer_value_kind(value: &str) -> ParameterKind {
    if value.is_empty() {
        return ParameterKind::Empty;
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return ParameterKind::Integer;
    }
    if is_uuid_like(value) {
        return ParameterKind::Uuid;
    }
    if is_hash_like(value) {
        return ParameterKind::Token;
    }
    if matches!(value, "true" | "false") {
        return ParameterKind::Boolean;
    }
    ParameterKind::String
}

fn parameter_kind_from_json(value: &serde_json::Value) -> ParameterKind {
    match value {
        serde_json::Value::Null => ParameterKind::Unknown,
        serde_json::Value::Bool(_) => ParameterKind::Boolean,
        serde_json::Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                ParameterKind::Integer
            } else {
                ParameterKind::Number
            }
        }
        serde_json::Value::String(value) => infer_value_kind(value),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => ParameterKind::DynamicSegment,
    }
}

fn choose_parameter_kind(current: &ParameterKind, candidate: &ParameterKind) -> ParameterKind {
    let rank = |kind: &ParameterKind| match kind {
        ParameterKind::Uuid => 7,
        ParameterKind::Token => 6,
        ParameterKind::NumericId => 5,
        ParameterKind::Integer => 4,
        ParameterKind::Boolean => 3,
        ParameterKind::Number => 2,
        ParameterKind::String => 1,
        ParameterKind::DynamicSegment | ParameterKind::Empty | ParameterKind::Unknown => 0,
    };

    if rank(candidate) >= rank(current) {
        candidate.clone()
    } else {
        current.clone()
    }
}

fn extract_schema_shape(body: &str) -> Option<SchemaShape> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    Some(infer_schema_shape(&value, 0))
}

fn infer_schema_shape(value: &serde_json::Value, depth: usize) -> SchemaShape {
    if depth > 3 {
        return SchemaShape {
            schema_type: SchemaType::Unknown,
            properties: BTreeMap::new(),
            items: None,
        };
    }

    match value {
        serde_json::Value::Null => SchemaShape {
            schema_type: SchemaType::Null,
            properties: BTreeMap::new(),
            items: None,
        },
        serde_json::Value::Bool(_) => SchemaShape {
            schema_type: SchemaType::Boolean,
            properties: BTreeMap::new(),
            items: None,
        },
        serde_json::Value::Number(number) => SchemaShape {
            schema_type: if number.is_i64() || number.is_u64() {
                SchemaType::Integer
            } else {
                SchemaType::Number
            },
            properties: BTreeMap::new(),
            items: None,
        },
        serde_json::Value::String(_) => SchemaShape {
            schema_type: SchemaType::String,
            properties: BTreeMap::new(),
            items: None,
        },
        serde_json::Value::Array(items) => SchemaShape {
            schema_type: SchemaType::Array,
            properties: BTreeMap::new(),
            items: items
                .first()
                .map(|item| Box::new(infer_schema_shape(item, depth + 1))),
        },
        serde_json::Value::Object(object) => {
            let mut properties = BTreeMap::new();
            for (key, value) in object {
                properties.insert(key.clone(), infer_schema_shape(value, depth + 1));
            }
            SchemaShape {
                schema_type: SchemaType::Object,
                properties,
                items: None,
            }
        }
    }
}

fn calculate_confidence(endpoint: &EndpointAccumulator) -> f64 {
    let observation_score = (endpoint.observation_count.min(5) as f64) / 5.0;
    let request_schema_score = if endpoint.request_schema.is_some() {
        0.2
    } else {
        0.0
    };
    let response_schema_score = if endpoint.response_schema.is_some() {
        0.2
    } else {
        0.0
    };
    let parameter_score = if endpoint.parameters.is_empty() {
        0.0
    } else {
        0.1
    };
    let status_score = if endpoint.status_codes.is_empty() {
        0.0
    } else {
        0.1
    };

    (observation_score * 0.4
        + request_schema_score
        + response_schema_score
        + parameter_score
        + status_score)
        .clamp(0.0, 1.0)
}

fn extract_response_candidates(response_body: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(response_body) {
        collect_json_string_candidates(None, &value, &mut candidates);
        candidates.sort();
        candidates.dedup();
        return candidates;
    }

    for pair in response_body.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let cleaned_value = value.trim();
            if cleaned_value.len() >= 3 && !cleaned_value.chars().any(char::is_whitespace) {
                candidates.push((key.trim().to_string(), cleaned_value.to_string()));
            }
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

fn collect_json_string_candidates(
    current_key: Option<&str>,
    value: &serde_json::Value,
    output: &mut Vec<(String, String)>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                collect_json_string_candidates(Some(key), value, output);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_string_candidates(current_key, item, output);
            }
        }
        serde_json::Value::String(text) => {
            if let Some(key) = current_key {
                let cleaned_text = text.trim();
                if cleaned_text.len() >= 3 && !cleaned_text.chars().any(char::is_whitespace) {
                    output.push((key.to_string(), cleaned_text.to_string()));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::TrafficRow;

    // ── Helpers ──────────────────────────────────────────────────────────

    fn minimal_traffic_row(method: &str, host: &str, path: &str) -> TrafficRow {
        TrafficRow {
            row_index: 0,
            url: format!("https://{host}{path}"),
            method: method.to_string(),
            status_code: Some(200),
            content_type: String::new(),
            request_headers: String::new(),
            response_headers: String::new(),
            request_body: String::new(),
            response_body: String::new(),
            host: host.to_string(),
            port: Some(443),
            path: path.to_string(),
            query: None,
            ip: String::new(),
            latency_ms: None,
            response_length: None,
            request_time: String::new(),
            title: String::new(),
            tags: String::new(),
        }
    }

    // ── is_all_digits ────────────────────────────────────────────────────

    #[test]
    fn is_all_digits_with_numeric_string() {
        assert!(is_all_digits("123"));
    }

    #[test]
    fn is_all_digits_with_alphanumeric_string() {
        assert!(!is_all_digits("12a3"));
    }

    #[test]
    fn is_all_digits_with_empty_string() {
        assert!(!is_all_digits(""));
    }

    #[test]
    fn is_all_digits_with_single_digit() {
        assert!(is_all_digits("0"));
    }

    #[test]
    fn is_all_digits_with_leading_zeros() {
        assert!(is_all_digits("00123"));
    }

    // ── is_uuid_like ────────────────────────────────────────────────────

    #[test]
    fn is_uuid_like_with_standard_format() {
        assert!(is_uuid_like("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn is_uuid_like_with_compact_format() {
        assert!(is_uuid_like("550e8400e29b41d4a716446655440000"));
    }

    #[test]
    fn is_uuid_like_rejects_non_uuid() {
        assert!(!is_uuid_like("not-a-uuid"));
    }

    #[test]
    fn is_uuid_like_rejects_short_hex() {
        assert!(!is_uuid_like("550e8400e29b"));
    }

    #[test]
    fn is_uuid_like_with_uppercase() {
        assert!(is_uuid_like("550E8400-E29B-41D4-A716-446655440000"));
    }

    // ── is_hash_like ────────────────────────────────────────────────────

    #[test]
    fn is_hash_like_with_16_hex_chars() {
        assert!(is_hash_like("abc123def4567890"));
    }

    #[test]
    fn is_hash_like_with_12_hex_chars() {
        assert!(is_hash_like("abc123def456"));
    }

    #[test]
    fn is_hash_like_rejects_short_value() {
        assert!(!is_hash_like("short"));
    }

    #[test]
    fn is_hash_like_with_exactly_8_hex_chars() {
        assert!(is_hash_like("abcdef01"));
    }

    #[test]
    fn is_hash_like_rejects_7_hex_chars() {
        assert!(!is_hash_like("abcdef0"));
    }

    #[test]
    fn is_hash_like_with_16_alphanumeric_mixed() {
        assert!(is_hash_like("abc123def_456-78"));
    }

    // ── split_extension ─────────────────────────────────────────────────

    #[test]
    fn split_extension_with_normal_file() {
        assert_eq!(split_extension("file.json"), Some(("file", "json")));
    }

    #[test]
    fn split_extension_rejects_hidden_file() {
        assert_eq!(split_extension(".hidden"), None);
    }

    #[test]
    fn split_extension_rejects_empty_extension() {
        assert_eq!(split_extension("noext."), None);
    }

    #[test]
    fn split_extension_rejects_no_dot() {
        assert_eq!(split_extension("noext"), None);
    }

    #[test]
    fn split_extension_with_multiple_dots() {
        assert_eq!(split_extension("archive.tar.gz"), Some(("archive.tar", "gz")));
    }

    #[test]
    fn split_extension_with_non_alphanumeric_extension() {
        assert_eq!(split_extension("file.ht ml"), None);
    }

    // ── normalize_path_segment ──────────────────────────────────────────

    #[test]
    fn normalize_segment_digits_become_id() {
        assert_eq!(normalize_path_segment("123"), ":id");
    }

    #[test]
    fn normalize_segment_uuid_stays_uuid() {
        assert_eq!(
            normalize_path_segment("550e8400-e29b-41d4-a716-446655440000"),
            ":uuid"
        );
    }

    #[test]
    fn normalize_segment_hash_becomes_param() {
        assert_eq!(normalize_path_segment("abc123def456ghij"), ":param");
    }

    #[test]
    fn normalize_segment_word_unchanged() {
        assert_eq!(normalize_path_segment("users"), "users");
    }

    #[test]
    fn normalize_segment_digits_with_extension() {
        assert_eq!(normalize_path_segment("123.json"), ":id.json");
    }

    #[test]
    fn normalize_segment_uuid_with_extension() {
        assert_eq!(
            normalize_path_segment("550e8400-e29b-41d4-a716-446655440000.pdf"),
            ":uuid.pdf"
        );
    }

    #[test]
    fn normalize_segment_hash_with_extension() {
        assert_eq!(
            normalize_path_segment("abc123def4567890.css"),
            ":param.css"
        );
    }

    // ── normalize_path_template ─────────────────────────────────────────

    #[test]
    fn normalize_path_numeric_id() {
        assert_eq!(normalize_path_template("/api/users/123"), "/api/users/:id");
    }

    #[test]
    fn normalize_path_uuid_segment() {
        assert_eq!(
            normalize_path_template("/api/users/550e8400-e29b-41d4-a716-446655440000"),
            "/api/users/:uuid"
        );
    }

    #[test]
    fn normalize_path_hash_like_segment() {
        assert_eq!(normalize_path_template("/api/users/abc123def456"), "/api/users/:param");
    }

    #[test]
    fn normalize_path_id_with_extension() {
        assert_eq!(
            normalize_path_template("/api/users/123.json"),
            "/api/users/:id.json"
        );
    }

    #[test]
    fn normalize_path_root_stays_root() {
        assert_eq!(normalize_path_template("/"), "/");
    }

    #[test]
    fn normalize_path_strips_query() {
        assert_eq!(normalize_path_template("/api/users?page=1"), "/api/users");
    }

    #[test]
    fn normalize_path_strips_empty_segments() {
        assert_eq!(normalize_path_template("//api///users//"), "/api/users");
    }

    #[test]
    fn normalize_path_plain_segments_unchanged() {
        assert_eq!(normalize_path_template("/api/users/profile"), "/api/users/profile");
    }

    // ── business_path_prefix ────────────────────────────────────────────

    #[test]
    fn business_prefix_stops_at_first_static_segment() {
        assert_eq!(business_path_prefix("/api/users/:id"), "/api");
    }

    #[test]
    fn business_prefix_multiple_static() {
        assert_eq!(business_path_prefix("/api/users"), "/api");
    }

    #[test]
    fn business_prefix_starts_with_dynamic_then_static() {
        assert_eq!(business_path_prefix("/:id/profile"), "/:id/profile");
    }

    #[test]
    fn business_prefix_root() {
        assert_eq!(business_path_prefix("/"), "/");
    }

    #[test]
    fn business_prefix_single_static() {
        assert_eq!(business_path_prefix("/api"), "/api");
    }

    #[test]
    fn business_prefix_all_dynamic() {
        assert_eq!(business_path_prefix("/:id/:uuid"), "/:id/:uuid");
    }

    // ── path_parameter_kind ─────────────────────────────────────────────

    #[test]
    fn path_param_kind_id() {
        assert_eq!(path_parameter_kind(":id"), ParameterKind::NumericId);
    }

    #[test]
    fn path_param_kind_id_with_extension() {
        assert_eq!(path_parameter_kind(":id.json"), ParameterKind::NumericId);
    }

    #[test]
    fn path_param_kind_uuid() {
        assert_eq!(path_parameter_kind(":uuid"), ParameterKind::Uuid);
    }

    #[test]
    fn path_param_kind_param() {
        assert_eq!(path_parameter_kind(":param"), ParameterKind::DynamicSegment);
    }

    #[test]
    fn path_param_kind_unknown() {
        assert_eq!(path_parameter_kind(":other"), ParameterKind::Unknown);
    }

    // ── infer_value_kind ────────────────────────────────────────────────

    #[test]
    fn infer_kind_empty() {
        assert_eq!(infer_value_kind(""), ParameterKind::Empty);
    }

    #[test]
    fn infer_kind_integer() {
        assert_eq!(infer_value_kind("123"), ParameterKind::Integer);
    }

    #[test]
    fn infer_kind_uuid() {
        assert_eq!(
            infer_value_kind("550e8400-e29b-41d4-a716-446655440000"),
            ParameterKind::Uuid
        );
    }

    #[test]
    fn infer_kind_boolean_true() {
        assert_eq!(infer_value_kind("true"), ParameterKind::Boolean);
    }

    #[test]
    fn infer_kind_boolean_false() {
        assert_eq!(infer_value_kind("false"), ParameterKind::Boolean);
    }

    #[test]
    fn infer_kind_string() {
        assert_eq!(infer_value_kind("hello"), ParameterKind::String);
    }

    #[test]
    fn infer_kind_token_for_long_hex() {
        assert_eq!(infer_value_kind("abc123def4567890"), ParameterKind::Token);
    }

    // ── choose_parameter_kind ───────────────────────────────────────────

    #[test]
    fn choose_kind_upgrades_to_higher_rank() {
        assert_eq!(
            choose_parameter_kind(&ParameterKind::String, &ParameterKind::Uuid),
            ParameterKind::Uuid
        );
    }

    #[test]
    fn choose_kind_keeps_current_when_higher() {
        assert_eq!(
            choose_parameter_kind(&ParameterKind::Uuid, &ParameterKind::Integer),
            ParameterKind::Uuid
        );
    }

    #[test]
    fn choose_kind_same_rank_keeps_candidate() {
        assert_eq!(
            choose_parameter_kind(&ParameterKind::Uuid, &ParameterKind::Uuid),
            ParameterKind::Uuid
        );
    }

    // ── extract_response_candidates ─────────────────────────────────────

    #[test]
    fn extract_candidates_from_json_with_strings() {
        let body = r#"{"name":"alice","token":"abc","short":"ab"}"#;
        let candidates = extract_response_candidates(body);
        assert_eq!(candidates, vec![
            ("name".into(), "alice".into()),
            ("token".into(), "abc".into()),
        ]);
    }

    #[test]
    fn extract_candidates_from_form_encoded() {
        let body = "key=value123&skip=ab&ok=hello";
        let candidates = extract_response_candidates(body);
        assert_eq!(candidates, vec![
            ("key".into(), "value123".into()),
            ("ok".into(), "hello".into()),
        ]);
    }

    #[test]
    fn extract_candidates_from_empty_body() {
        assert!(extract_response_candidates("").is_empty());
    }

    #[test]
    fn extract_candidates_from_json_with_nested_object() {
        let body = r#"{"data":{"token":"nested_value"}}"#;
        let candidates = extract_response_candidates(body);
        assert_eq!(candidates, vec![("token".into(), "nested_value".into())]);
    }

    #[test]
    fn extract_candidates_from_json_array() {
        let body = r#"[{"id":"abc123"},{"id":"def456"}]"#;
        let candidates = extract_response_candidates(body);
        assert_eq!(candidates, vec![
            ("id".into(), "abc123".into()),
            ("id".into(), "def456".into()),
        ]);
    }

    // ── parameter_kind_from_json ────────────────────────────────────────

    #[test]
    fn json_kind_null_is_unknown() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::Value::Null),
            ParameterKind::Unknown
        );
    }

    #[test]
    fn json_kind_bool() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::json!(true)),
            ParameterKind::Boolean
        );
    }

    #[test]
    fn json_kind_integer() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::json!(42)),
            ParameterKind::Integer
        );
    }

    #[test]
    fn json_kind_float_is_number() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::json!(2.5)),
            ParameterKind::Number
        );
    }

    #[test]
    fn json_kind_string_infers_inner() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::json!("123")),
            ParameterKind::Integer
        );
    }

    #[test]
    fn json_kind_array_is_dynamic_segment() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::json!([1, 2, 3])),
            ParameterKind::DynamicSegment
        );
    }

    #[test]
    fn json_kind_object_is_dynamic_segment() {
        assert_eq!(
            parameter_kind_from_json(&serde_json::json!({"a": 1})),
            ParameterKind::DynamicSegment
        );
    }

    // ── build_business_graph ────────────────────────────────────────────

    #[test]
    fn empty_rows_produce_empty_graph() {
        let graph = build_business_graph(&[]).unwrap();
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn single_row_produces_bf_and_endpoint_nodes_with_contains_edge() {
        let rows = vec![minimal_traffic_row("GET", "example.com", "/api/users")];
        let graph = build_business_graph(&rows).unwrap();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].label, "contains");

        let bf_node = graph
            .nodes
            .iter()
            .find(|n| n.kind == BusinessNodeKind::BusinessFunction)
            .expect("business function node");
        let ep_node = graph
            .nodes
            .iter()
            .find(|n| n.kind == BusinessNodeKind::Endpoint)
            .expect("endpoint node");

        assert_eq!(bf_node.stable_key, "bf:example.com:/api");
        assert_eq!(ep_node.stable_key, "ep:GET:example.com:/api/users");
        assert_eq!(graph.edges[0].source_node_id, bf_node.id);
        assert_eq!(graph.edges[0].target_node_id, ep_node.id);
    }

    #[test]
    fn two_different_endpoints_produce_calls_after_edge() {
        let rows = vec![
            minimal_traffic_row("GET", "example.com", "/api/users"),
            minimal_traffic_row("POST", "example.com", "/api/login"),
        ];
        let graph = build_business_graph(&rows).unwrap();

        let calls_after: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.label == "calls_after")
            .collect();
        assert_eq!(calls_after.len(), 1);
    }

    #[test]
    fn same_endpoint_twice_produces_no_calls_after() {
        let rows = vec![
            minimal_traffic_row("GET", "example.com", "/api/users"),
            minimal_traffic_row("GET", "example.com", "/api/users"),
        ];
        let graph = build_business_graph(&rows).unwrap();

        let calls_after: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.label == "calls_after")
            .collect();
        assert!(calls_after.is_empty());
    }

    #[test]
    fn build_business_graph_is_deterministic() {
        let make_rows = || {
            vec![
                minimal_traffic_row("GET", "example.com", "/api/users"),
                minimal_traffic_row("POST", "example.com", "/api/login"),
                minimal_traffic_row("GET", "example.com", "/api/users/123"),
            ]
        };

        let graph_a = build_business_graph(&make_rows()).unwrap();
        let graph_b = build_business_graph(&make_rows()).unwrap();

        assert_eq!(graph_a.nodes.len(), graph_b.nodes.len());
        assert_eq!(graph_a.edges.len(), graph_b.edges.len());

        for (a, b) in graph_a.nodes.iter().zip(graph_b.nodes.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.stable_key, b.stable_key);
        }
        for (a, b) in graph_a.edges.iter().zip(graph_b.edges.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.source_node_id, b.source_node_id);
            assert_eq!(a.target_node_id, b.target_node_id);
            assert_eq!(a.label, b.label);
        }
    }

    #[test]
    fn stable_key_formats_match_expected_patterns() {
        let rows = vec![minimal_traffic_row("GET", "example.com", "/api/users")];
        let graph = build_business_graph(&rows).unwrap();

        let bf_keys: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.kind == BusinessNodeKind::BusinessFunction)
            .map(|n| &n.stable_key)
            .collect();
        assert!(bf_keys.iter().all(|k| k.starts_with("bf:")));

        let ep_keys: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.kind == BusinessNodeKind::Endpoint)
            .map(|n| &n.stable_key)
            .collect();
        assert!(ep_keys.iter().all(|k| k.starts_with("ep:")));
    }

    #[test]
    fn multiple_hosts_create_separate_bf_nodes() {
        let rows = vec![
            minimal_traffic_row("GET", "a.example.com", "/api/users"),
            minimal_traffic_row("GET", "b.example.com", "/api/users"),
        ];
        let graph = build_business_graph(&rows).unwrap();

        let bf_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.kind == BusinessNodeKind::BusinessFunction)
            .collect();
        assert_eq!(bf_nodes.len(), 2);
    }

    #[test]
    fn data_dependency_edge_created_when_response_value_appears_in_later_request() {
        let mut row1 = minimal_traffic_row("GET", "example.com", "/api/token");
        row1.response_body = r#"{"token":"secret_token_value"}"#.to_string();

        let mut row2 = minimal_traffic_row("POST", "example.com", "/api/action");
        row2.request_body = r#"{"token":"secret_token_value"}"#.to_string();

        let graph = build_business_graph(&[row1, row2]).unwrap();

        let data_dep: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.label.starts_with("data_dependency:"))
            .collect();
        assert_eq!(data_dep.len(), 1);
        assert_eq!(data_dep[0].label, "data_dependency:token");
    }

    #[test]
    fn path_normalization_groups_different_ids_to_same_endpoint() {
        let rows = vec![
            minimal_traffic_row("GET", "example.com", "/api/users/123"),
            minimal_traffic_row("GET", "example.com", "/api/users/456"),
        ];
        let graph = build_business_graph(&rows).unwrap();

        let ep_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.kind == BusinessNodeKind::Endpoint)
            .collect();
        assert_eq!(ep_nodes.len(), 1);
        assert_eq!(
            ep_nodes[0].stable_key,
            "ep:GET:example.com:/api/users/:id"
        );
    }

    #[test]
    fn nodes_are_sorted_by_stable_key() {
        let rows = vec![
            minimal_traffic_row("POST", "example.com", "/api/login"),
            minimal_traffic_row("GET", "example.com", "/api/users"),
        ];
        let graph = build_business_graph(&rows).unwrap();

        let keys: Vec<_> = graph.nodes.iter().map(|n| &n.stable_key).collect();
        let mut sorted_keys = keys.clone();
        sorted_keys.sort();
        assert_eq!(keys, sorted_keys);
    }

    #[test]
    fn edges_are_sorted_by_label_then_source_then_target() {
        let rows = vec![
            minimal_traffic_row("GET", "example.com", "/api/users"),
            minimal_traffic_row("POST", "example.com", "/api/login"),
        ];
        let graph = build_business_graph(&rows).unwrap();

        let edge_tuples: Vec<_> = graph
            .edges
            .iter()
            .map(|e| (&e.label, &e.source_node_id, &e.target_node_id))
            .collect();
        let mut sorted = edge_tuples.clone();
        sorted.sort();
        assert_eq!(edge_tuples, sorted);
    }

    #[test]
    fn endpoint_observes_status_codes() {
        let mut row = minimal_traffic_row("GET", "example.com", "/api/users");
        row.status_code = Some(404);

        let graph = build_business_graph(&[row]).unwrap();
        let ep = graph
            .nodes
            .iter()
            .find(|n| n.kind == BusinessNodeKind::Endpoint)
            .unwrap();

        if let BusinessNodeProperties::Endpoint(props) = &ep.properties {
            assert!(props.status_codes.contains(&404));
            assert_eq!(props.status_profiles.client_error, 1);
        } else {
            panic!("expected endpoint properties");
        }
    }

    #[test]
    fn endpoint_confidence_increases_with_observations() {
        let one_row = vec![minimal_traffic_row("GET", "example.com", "/api/users")];
        let graph_one = build_business_graph(&one_row).unwrap();

        let five_rows: Vec<_> = (0..5)
            .map(|i| {
                let mut r = minimal_traffic_row("GET", "example.com", "/api/users");
                r.row_index = i;
                r
            })
            .collect();
        let graph_five = build_business_graph(&five_rows).unwrap();

        let confidence_one = match &graph_one.nodes[0].properties {
            BusinessNodeProperties::Endpoint(p) => p.confidence,
            _ => match &graph_one.nodes[1].properties {
                BusinessNodeProperties::Endpoint(p) => p.confidence,
                _ => panic!("no endpoint found"),
            },
        };
        let confidence_five = match &graph_five.nodes[0].properties {
            BusinessNodeProperties::Endpoint(p) => p.confidence,
            _ => match &graph_five.nodes[1].properties {
                BusinessNodeProperties::Endpoint(p) => p.confidence,
                _ => panic!("no endpoint found"),
            },
        };

        assert!(confidence_five > confidence_one);
    }
}

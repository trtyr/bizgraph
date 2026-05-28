use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::json;

use crate::parser::TrafficRow;
use crate::types::{
    deterministic_id, BusinessEdge, BusinessFunctionProperties, BusinessGraph, BusinessNode,
    BusinessNodeKind, BusinessNodeProperties, EndpointProperties, ParameterDescriptor,
    ParameterKind, ParameterLocation, SchemaShape, SchemaType, StatusProfiles,
};

pub fn build_business_graph(rows: &[TrafficRow]) -> Result<BusinessGraph, String> {
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
                    source_node_id: *node_ids.get(business_key).ok_or_else(|| {
                        format!("missing business function node for {business_key}")
                    })?,
                    target_node_id: *node_ids
                        .get(endpoint_key)
                        .ok_or_else(|| format!("missing endpoint node for {endpoint_key}"))?,
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
                    .ok_or_else(|| format!("missing sequence source node for {source_key}"))?,
                target_node_id: *node_ids
                    .get(target_key)
                    .ok_or_else(|| format!("missing sequence target node for {target_key}"))?,
                label: "calls_after".to_string(),
                properties: json!({}),
            });
        }
    }

    for (index, current_key) in sequence.iter().enumerate() {
        let current = endpoint_state
            .get(current_key)
            .ok_or_else(|| format!("missing endpoint accumulator for {current_key}"))?;
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
                        source_node_id: *node_ids.get(previous_key).ok_or_else(|| {
                            format!("missing dependency source node for {previous_key}")
                        })?,
                        target_node_id: *node_ids.get(current_key).ok_or_else(|| {
                            format!("missing dependency target node for {current_key}")
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
}

impl BusinessFunctionAccumulator {
    fn new(host: &str, path_prefix: &str) -> Self {
        Self {
            host: host.to_string(),
            path_prefix: path_prefix.to_string(),
            endpoint_keys: BTreeSet::new(),
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

fn normalize_path_template(path: &str) -> String {
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

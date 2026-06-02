use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::graph::is_static_resource;
use crate::parser::TrafficRow;
use crate::{Error, Result};

use super::chat;

/// A business function identified by the AI, with its associated endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessFunctionGroup {
    pub name: String,
    pub description: String,
    pub endpoints: Vec<EndpointMapping>,
}

/// An endpoint mapped to a business function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointMapping {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub host: String,
}

/// Response from the AI business function identification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessIdentification {
    pub business_functions: Vec<BusinessFunctionGroup>,
}

/// Time gap in seconds to split sessions. If two consecutive requests are
/// more than this apart, they belong to different user sessions.
const SESSION_GAP_SECS: i64 = 30;

/// A user session identified by time-based grouping of requests.
#[derive(Debug, Clone)]
pub struct Session {
    pub index: usize,
    pub endpoints: Vec<String>, // "METHOD /path" sequence
}

/// A sample request/response pair for an endpoint.
#[derive(Debug, Clone)]
pub struct EndpointSample {
    pub method: String,
    pub path: String,
    pub request_body: String,
    pub response_body: String,
    pub status: Option<u16>,
}

/// Group traffic rows into sessions by time gaps.
/// Returns sessions sorted by start time, each containing an ordered endpoint sequence.
pub fn group_sessions(rows: &[TrafficRow]) -> Vec<Session> {
    if rows.is_empty() {
        return Vec::new();
    }

    // Parse timestamps and sort by time
    let mut timed: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let dt = DateTime::parse_from_rfc3339(&row.request_time)
                .ok()?
                .with_timezone(&Utc);
            Some((dt, row))
        })
        .collect();
    timed.sort_by_key(|(dt, _)| *dt);

    let mut sessions: Vec<Session> = Vec::new();
    let mut current_endpoints: Vec<String> = Vec::new();
    let mut last_time: Option<DateTime<Utc>> = None;

    for (dt, row) in &timed {
        let endpoint = format!("{} {}", row.method, row.path);
        if let Some(prev) = last_time {
            if (*dt - prev).num_seconds() > SESSION_GAP_SECS {
                // New session
                if !current_endpoints.is_empty() {
                    sessions.push(Session {
                        index: sessions.len(),
                        endpoints: std::mem::take(&mut current_endpoints),
                    });
                }
            }
        }
        current_endpoints.push(endpoint);
        last_time = Some(*dt);
    }

    if !current_endpoints.is_empty() {
        sessions.push(Session {
            index: sessions.len(),
            endpoints: current_endpoints,
        });
    }

    sessions
}

/// Build a compact session flow summary for the AI prompt.
/// Shows each session as an ordered sequence of endpoints.
pub fn build_session_summary(sessions: &[Session]) -> String {
    if sessions.is_empty() {
        return "No sessions detected.".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!("Detected {} user sessions:\n\n", sessions.len()));

    for session in sessions {
        // Deduplicate consecutive identical endpoints
        let mut deduped: Vec<&str> = Vec::new();
        for ep in &session.endpoints {
            if deduped.last() != Some(&ep.as_str()) {
                deduped.push(ep);
            }
        }
        out.push_str(&format!(
            "Session {} ({} requests → {} unique):\n  {}\n\n",
            session.index + 1,
            session.endpoints.len(),
            deduped.len(),
            deduped.join(" → ")
        ));
    }

    out
}

/// Collect one sample per unique endpoint (method + path).
/// Picks the sample with the longest response body for richest context.
pub fn collect_endpoint_samples(rows: &[TrafficRow]) -> Vec<EndpointSample> {
    let mut samples: BTreeMap<String, EndpointSample> = BTreeMap::new();

    for row in rows {
        if is_static_resource(&row.path) {
            continue;
        }

        let key = format!("{} {}", row.method, row.path);
        let resp_len = row.response_body.len();
        let req_len = row.request_body.len();

        let should_replace = match samples.get(&key) {
            Some(existing) => resp_len > existing.response_body.len(),
            None => true,
        };

        if should_replace && (resp_len > 0 || req_len > 0) {
            samples.insert(
                key,
                EndpointSample {
                    method: row.method.clone(),
                    path: row.path.clone(),
                    request_body: row.request_body.clone(),
                    response_body: row.response_body.clone(),
                    status: row.status_code,
                },
            );
        }
    }

    samples.into_values().collect()
}

/// Build a compact sample summary for the AI prompt.
pub fn build_sample_summary(samples: &[EndpointSample]) -> String {
    if samples.is_empty() {
        return "No request/response samples available.".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Request/response samples ({} endpoints):\n\n",
        samples.len()
    ));

    for sample in samples {
        out.push_str(&format!(
            "=== {} {} (status: {}) ===\n",
            sample.method,
            sample.path,
            sample.status.map_or("-".to_string(), |s| s.to_string()),
        ));
        if !sample.request_body.is_empty() {
            out.push_str(&format!("Request: {}\n", sample.request_body));
        }
        if !sample.response_body.is_empty() {
            out.push_str(&format!("Response: {}\n", sample.response_body));
        }
        out.push('\n');
    }

    out
}

/// Build a compact endpoint list from traffic rows for the AI prompt.
/// Excludes static resources (JS, CSS, images, fonts, CDN) to keep the list focused.
pub fn build_endpoint_list(rows: &[TrafficRow]) -> String {
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        if is_static_resource(&row.path) {
            continue;
        }
        let key = format!("{} {}", row.method, row.path);
        seen.entry(key).or_default().insert(row.host.clone());
    }

    let mut lines: Vec<String> = Vec::new();
    for (endpoint, hosts) in &seen {
        let host_list: Vec<&str> = hosts.iter().map(|s| s.as_str()).collect();
        lines.push(format!("{endpoint} [{}]", host_list.join(", ")));
    }
    lines.join("\n")
}

/// Ask the AI to identify business functions from the endpoint list.
/// Enhanced with session flows and request/response samples for deeper analysis.
pub async fn identify_business_functions(
    rows: &[TrafficRow],
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<BusinessIdentification> {
    let endpoint_list = build_endpoint_list(rows);

    // Build session analysis
    let sessions = group_sessions(rows);
    let session_summary = build_session_summary(&sessions);

    // Build request/response samples
    let samples = collect_endpoint_samples(rows);
    let sample_summary = build_sample_summary(&samples);

    let prompt = format!(
        r#"Analyze the following HTTP traffic captured from a web application and identify the BUSINESS FUNCTIONS it serves.

## Endpoints
{endpoint_list}

## User Session Flows
{session_summary}

## Request/Response Samples
{sample_summary}

Based on the above, identify the business functions. Consider:
1. The endpoint paths and methods to understand what each API does
2. The session flows to understand how users navigate through the application
3. The request/response bodies to understand the actual data being exchanged
4. Group endpoints by business logic, NOT by URL path prefix

Return a JSON object with this exact structure:
{{
  "business_functions": [
    {{
      "name": "Business Function Name",
      "description": "Brief description of what this function does, based on actual data observed",
      "endpoints": [
        {{"method": "GET", "path": "/api/...", "host": "example.com"}}
      ]
    }}
  ]
}}

IMPORTANT:
- Use the ACTUAL business function names from the application, not generic names
- Each endpoint must appear in exactly one business function
- Include ALL endpoints, do not skip any
- Group by business logic, NOT by URL path prefix
- Use the request/response data to enrich your descriptions
- Return ONLY the JSON, no other text"#
    );

    let messages = vec![
        chat::ChatMessage::system(super::prompts::BUSINESS_ID_PROMPT),
        chat::ChatMessage::user(prompt),
    ];

    let response = chat::chat_fresh(messages, api_key, model, api_url).await?;

    // Try to parse the response as JSON
    let cleaned = clean_json_response(&response);
    let identification: BusinessIdentification =
        serde_json::from_str(&cleaned).map_err(|source| Error::JsonContext {
            context: "Failed to parse AI business identification response".to_string(),
            source,
        })?;

    Ok(identification)
}

/// Clean AI response to extract JSON — strip markdown code blocks, leading/trailing text.
fn clean_json_response(response: &str) -> String {
    let trimmed = response.trim();

    // Strip markdown code block
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            return trimmed[start..=end].to_string();
        }
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_json_strips_markdown_blocks() {
        let input = r#"```json
{"business_functions": []}
```"#;
        let cleaned = clean_json_response(input);
        assert_eq!(cleaned, r#"{"business_functions": []}"#);
    }

    #[test]
    fn clean_json_handles_plain_json() {
        let input = r#"{"business_functions": []}"#;
        let cleaned = clean_json_response(input);
        assert_eq!(cleaned, input);
    }

    #[test]
    fn build_endpoint_list_deduplicates() {
        let rows = vec![
            TrafficRow {
                row_index: 0,
                url: "https://a.com/api/users".to_string(),
                method: "GET".to_string(),
                status_code: Some(200),
                content_type: "application/json".to_string(),
                request_headers: String::new(),
                response_headers: String::new(),
                request_body: String::new(),
                response_body: String::new(),
                host: "a.com".to_string(),
                port: None,
                path: "/api/users".to_string(),
                query: None,
                ip: String::new(),
                latency_ms: None,
                response_length: None,
                request_time: String::new(),
                title: String::new(),
                tags: String::new(),
            },
            TrafficRow {
                row_index: 1,
                url: "https://a.com/api/users".to_string(),
                method: "GET".to_string(),
                status_code: Some(200),
                content_type: "application/json".to_string(),
                request_headers: String::new(),
                response_headers: String::new(),
                request_body: String::new(),
                response_body: String::new(),
                host: "a.com".to_string(),
                port: None,
                path: "/api/users".to_string(),
                query: None,
                ip: String::new(),
                latency_ms: None,
                response_length: None,
                request_time: String::new(),
                title: String::new(),
                tags: String::new(),
            },
        ];
        let list = build_endpoint_list(&rows);
        assert_eq!(list, "GET /api/users [a.com]");
    }

    #[test]
    fn is_static_resource_by_extension() {
        assert!(is_static_resource("/assets/js/app.js"));
        assert!(is_static_resource("/style.css"));
        assert!(is_static_resource("/img/logo.png"));
        assert!(is_static_resource("/font.woff2"));
        assert!(is_static_resource("/bundle.min.js"));
    }

    #[test]
    fn is_static_resource_keeps_important() {
        assert!(!is_static_resource("/api/source.map"));
        assert!(!is_static_resource("/api/data.json"));
        assert!(!is_static_resource("/wv/docdatahandler.ashx"));
        assert!(!is_static_resource("/api/users"));
    }

    #[test]
    fn is_static_resource_cdn() {
        assert!(is_static_resource("/cdn/vue/vue.min.js"));
        assert!(is_static_resource("/cdn/antd/locale-provider/zh_CN.js"));
    }

    #[test]
    fn build_endpoint_list_filters_static() {
        let rows = vec![
            TrafficRow {
                row_index: 0,
                url: "https://a.com/api/users".to_string(),
                method: "GET".to_string(),
                status_code: Some(200),
                content_type: "application/json".to_string(),
                request_headers: String::new(),
                response_headers: String::new(),
                request_body: String::new(),
                response_body: String::new(),
                host: "a.com".to_string(),
                port: None,
                path: "/api/users".to_string(),
                query: None,
                ip: String::new(),
                latency_ms: None,
                response_length: None,
                request_time: String::new(),
                title: String::new(),
                tags: String::new(),
            },
            TrafficRow {
                row_index: 1,
                url: "https://a.com/cdn/vue/vue.min.js".to_string(),
                method: "GET".to_string(),
                status_code: Some(200),
                content_type: "application/javascript".to_string(),
                request_headers: String::new(),
                response_headers: String::new(),
                request_body: String::new(),
                response_body: String::new(),
                host: "a.com".to_string(),
                port: None,
                path: "/cdn/vue/vue.min.js".to_string(),
                query: None,
                ip: String::new(),
                latency_ms: None,
                response_length: None,
                request_time: String::new(),
                title: String::new(),
                tags: String::new(),
            },
            TrafficRow {
                row_index: 2,
                url: "https://a.com/assets/logo.png".to_string(),
                method: "GET".to_string(),
                status_code: Some(200),
                content_type: "image/png".to_string(),
                request_headers: String::new(),
                response_headers: String::new(),
                request_body: String::new(),
                response_body: String::new(),
                host: "a.com".to_string(),
                port: None,
                path: "/assets/logo.png".to_string(),
                query: None,
                ip: String::new(),
                latency_ms: None,
                response_length: None,
                request_time: String::new(),
                title: String::new(),
                tags: String::new(),
            },
        ];
        let list = build_endpoint_list(&rows);
        assert_eq!(list, "GET /api/users [a.com]");
    }

    // ── Session analysis tests ─────────────────────────────────────────

    fn make_row(method: &str, path: &str, time: &str) -> TrafficRow {
        TrafficRow {
            row_index: 0,
            url: format!("https://a.com{path}"),
            method: method.to_string(),
            status_code: Some(200),
            content_type: "application/json".to_string(),
            request_headers: String::new(),
            response_headers: String::new(),
            request_body: String::new(),
            response_body: String::new(),
            host: "a.com".to_string(),
            port: None,
            path: path.to_string(),
            query: None,
            ip: String::new(),
            latency_ms: None,
            response_length: None,
            request_time: time.to_string(),
            title: String::new(),
            tags: String::new(),
        }
    }

    fn make_row_with_body(method: &str, path: &str, time: &str, req_body: &str, resp_body: &str) -> TrafficRow {
        TrafficRow {
            row_index: 0,
            url: format!("https://a.com{path}"),
            method: method.to_string(),
            status_code: Some(200),
            content_type: "application/json".to_string(),
            request_headers: String::new(),
            response_headers: String::new(),
            request_body: req_body.to_string(),
            response_body: resp_body.to_string(),
            host: "a.com".to_string(),
            port: None,
            path: path.to_string(),
            query: None,
            ip: String::new(),
            latency_ms: None,
            response_length: None,
            request_time: time.to_string(),
            title: String::new(),
            tags: String::new(),
        }
    }

    #[test]
    fn group_sessions_single_session() {
        let rows = vec![
            make_row("GET", "/api/a", "2025-01-01T00:00:00Z"),
            make_row("GET", "/api/b", "2025-01-01T00:00:10Z"),
            make_row("GET", "/api/c", "2025-01-01T00:00:20Z"),
        ];
        let sessions = group_sessions(&rows);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].endpoints.len(), 3);
    }

    #[test]
    fn group_sessions_multiple_sessions() {
        let rows = vec![
            make_row("GET", "/api/a", "2025-01-01T00:00:00Z"),
            make_row("GET", "/api/b", "2025-01-01T00:00:10Z"),
            // Gap > 30s → new session
            make_row("POST", "/api/login", "2025-01-01T00:01:00Z"),
            make_row("GET", "/api/c", "2025-01-01T00:01:10Z"),
        ];
        let sessions = group_sessions(&rows);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].endpoints, vec!["GET /api/a", "GET /api/b"]);
        assert_eq!(sessions[1].endpoints, vec!["POST /api/login", "GET /api/c"]);
    }

    #[test]
    fn group_sessions_empty() {
        let sessions = group_sessions(&[]);
        assert!(sessions.is_empty());
    }

    #[test]
    fn group_sessions_no_timestamps() {
        let rows = vec![
            make_row("GET", "/api/a", ""),
            make_row("GET", "/api/b", ""),
        ];
        let sessions = group_sessions(&rows);
        assert!(sessions.is_empty());
    }

    #[test]
    fn build_session_summary_deduplicates_consecutive() {
        let sessions = vec![Session {
            index: 0,
            endpoints: vec![
                "GET /api/a".to_string(),
                "GET /api/a".to_string(),
                "GET /api/b".to_string(),
            ],
        }];
        let summary = build_session_summary(&sessions);
        assert!(summary.contains("3 requests → 2 unique"));
        assert!(summary.contains("GET /api/a → GET /api/b"));
    }

    // ── Endpoint sample tests ──────────────────────────────────────────

    #[test]
    fn collect_endpoint_samples_picks_richest() {
        let rows = vec![
            make_row_with_body("GET", "/api/users", "2025-01-01T00:00:00Z", "", "{\"users\":[]}"),
            make_row_with_body("GET", "/api/users", "2025-01-01T00:00:10Z", "", "{\"users\":[{\"id\":1,\"name\":\"Alice\"}],\"total\":1}"),
        ];
        let samples = collect_endpoint_samples(&rows);
        assert_eq!(samples.len(), 1);
        assert!(samples[0].response_body.contains("Alice"));
    }

    #[test]
    fn collect_endpoint_samples_skips_empty() {
        let rows = vec![
            make_row("GET", "/api/ping", "2025-01-01T00:00:00Z"),
        ];
        let samples = collect_endpoint_samples(&rows);
        assert!(samples.is_empty());
    }

    #[test]
    fn collect_endpoint_samples_skips_static() {
        let rows = vec![
            make_row_with_body("GET", "/assets/app.js", "2025-01-01T00:00:00Z", "", "var x=1;"),
        ];
        let samples = collect_endpoint_samples(&rows);
        assert!(samples.is_empty());
    }

    #[test]
    fn build_sample_summary_shows_all() {
        let samples: Vec<EndpointSample> = (0..5)
            .map(|i| EndpointSample {
                method: "GET".to_string(),
                path: format!("/api/{}", i),
                request_body: String::new(),
                response_body: format!("{{\"id\":{i}}}"),
                status: Some(200),
            })
            .collect();
        let summary = build_sample_summary(&samples);
        assert!(summary.contains("5 endpoints"));
        assert!(summary.contains("/api/0"));
        assert!(summary.contains("/api/4"));
    }
}

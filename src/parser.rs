use serde::Deserialize;

use crate::{Error, Result};

/// Internal traffic row extracted from a HAR (HTTP Archive) file.
#[derive(Debug, Clone)]
pub struct TrafficRow {
    pub row_index: usize,
    pub url: String,
    pub method: String,
    pub status_code: Option<u16>,
    pub content_type: String,
    pub request_headers: String,
    pub response_headers: String,
    pub request_body: String,
    pub response_body: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
    pub query: Option<String>,
    pub ip: String,
    pub latency_ms: Option<u64>,
    pub response_length: Option<u64>,
    pub request_time: String,
    pub title: String,
    pub tags: String,
}

// ── HAR deserialization types ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HarFile {
    log: HarLog,
}

#[derive(Debug, Deserialize)]
struct HarLog {
    entries: Vec<HarEntry>,
}

#[derive(Debug, Deserialize)]
struct HarEntry {
    request: HarRequest,
    response: HarResponse,
    #[serde(default, rename = "startedDateTime")]
    started_date_time: Option<String>,
    #[serde(default, rename = "serverIpAddress")]
    server_ip_address: Option<String>,
    #[serde(default)]
    timings: Option<HarTimings>,
}

#[derive(Debug, Deserialize)]
struct HarRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: Option<Vec<HarHeader>>,
    #[serde(default, rename = "queryString")]
    #[allow(dead_code)]
    query_string: Option<Vec<HarQueryString>>,
    #[serde(default, rename = "postData")]
    post_data: Option<HarPostData>,
}

#[derive(Debug, Deserialize)]
struct HarResponse {
    status: u16,
    #[serde(default)]
    content: Option<HarContent>,
    #[serde(default)]
    headers: Option<Vec<HarHeader>>,
}

#[derive(Debug, Deserialize)]
struct HarHeader {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HarQueryString {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct HarPostData {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HarContent {
    #[serde(default)]
    size: Option<i64>,
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HarTimings {
    #[serde(default)]
    receive: Option<f64>,
    #[serde(default)]
    send: Option<f64>,
    #[serde(default)]
    wait: Option<f64>,
}

// ── Public API ────────────────────────────────────────────────────────────

/// Parse a HAR (HTTP Archive) file into traffic rows.
pub fn parse_har(har_path: &str, host_filter: Option<&str>) -> Result<Vec<TrafficRow>> {
    let content = std::fs::read_to_string(har_path).map_err(|source| {
        Error::io(
            format!("failed to read HAR file '{har_path}'"),
            source,
        )
    })?;

    // Validate HAR file format before parsing
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(Error::validation(format!("HAR file '{har_path}' is empty")));
    }
    if !trimmed.starts_with('{') {
        return Err(Error::validation(format!(
            "'{har_path}' does not look like a HAR file (expected JSON object starting with '{{', got '{}')",
            trimmed.chars().next().unwrap_or('?')
        )));
    }
    if !trimmed.contains("\"log\"") {
        return Err(Error::validation(format!(
            "'{har_path}' is not a valid HAR file (missing 'log' field). Make sure you exported from browser DevTools → Network → Export HAR."
        )));
    }

    let har: HarFile = serde_json::from_str(&content).map_err(|source| {
        Error::json(
            format!("failed to parse HAR file '{har_path}': {source}"),
            source,
        )
    })?;

    let mut rows = Vec::new();

    for (index, entry) in har.log.entries.iter().enumerate() {
        let url = entry.request.url.trim().to_string();
        let method = entry.request.method.trim().to_uppercase();

        if url.is_empty() || method.is_empty() {
            continue;
        }

        let parsed_url = match Url::parse(&url) {
            Ok(u) => u,
            Err(_) => continue,
        };

        let host = parsed_url.host_str().unwrap_or("").to_string();
        if host.is_empty() {
            continue;
        }

        if let Some(filter) = host_filter {
            if !host.starts_with(filter) {
                continue;
            }
        }

        let path = if parsed_url.path().is_empty() {
            "/".to_string()
        } else {
            parsed_url.path().to_string()
        };

        let query = parsed_url
            .query()
            .filter(|q| !q.is_empty())
            .map(|q| q.to_string());

        let port = extract_port(&parsed_url);

        let request_headers = format_headers(entry.request.headers.as_deref().unwrap_or(&[]));
        let response_headers = format_headers(entry.response.headers.as_deref().unwrap_or(&[]));

        let request_body = entry
            .request
            .post_data
            .as_ref()
            .and_then(|p| p.text.as_deref())
            .unwrap_or("")
            .to_string();

        let content = entry.response.content.as_ref();
        let response_body = content
            .and_then(|c| c.text.as_deref())
            .unwrap_or("")
            .to_string();

        let content_type = content
            .and_then(|c| c.mime_type.as_deref())
            .unwrap_or("")
            .to_string();

        let response_length = content.and_then(|c| c.size.filter(|&s| s >= 0).map(|s| s as u64));

        let latency_ms = entry.timings.as_ref().and_then(|t| {
            let wait = t.wait.unwrap_or(0.0);
            let receive = t.receive.unwrap_or(0.0);
            let send = t.send.unwrap_or(0.0);
            let total = wait + receive + send;
            if total > 0.0 {
                Some(total as u64)
            } else {
                None
            }
        });

        let request_time = entry
            .started_date_time
            .as_deref()
            .unwrap_or("")
            .to_string();

        let ip = entry
            .server_ip_address
            .as_deref()
            .unwrap_or("")
            .to_string();

        rows.push(TrafficRow {
            row_index: index,
            url,
            method,
            status_code: Some(entry.response.status),
            content_type,
            request_headers,
            response_headers,
            request_body,
            response_body,
            host,
            port,
            path,
            query,
            ip,
            latency_ms,
            response_length,
            request_time,
            title: String::new(),
            tags: String::new(),
        });
    }

    Ok(rows)
}

// ── Helpers ───────────────────────────────────────────────────────────────

use url::Url;

fn extract_port(url: &Url) -> Option<u16> {
    if let Some(port) = url.port() {
        return Some(port);
    }
    match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn format_headers(headers: &[HarHeader]) -> String {
    headers
        .iter()
        .map(|h| format!("{}: {}", h.name, h.value))
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_har() {
        let har = r#"{
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {
                            "method": "GET",
                            "url": "https://example.com/api/users?page=1",
                            "headers": [{"name": "Accept", "value": "application/json"}],
                            "queryString": [{"name": "page", "value": "1"}]
                        },
                        "response": {
                            "status": 200,
                            "content": {
                                "size": 42,
                                "mimeType": "application/json",
                                "text": "{\"users\": []}"
                            },
                            "headers": [{"name": "Content-Type", "value": "application/json"}]
                        },
                        "startedDateTime": "2025-01-01T00:00:00Z",
                        "serverIpAddress": "93.184.216.34",
                        "timings": {"wait": 100, "receive": 50, "send": 10}
                    }
                ]
            }
        }"#;

        let rows = parse_har_from_str(har, None).unwrap();
        assert_eq!(rows.len(), 1);

        let row = &rows[0];
        assert_eq!(row.method, "GET");
        assert_eq!(row.host, "example.com");
        assert_eq!(row.path, "/api/users");
        assert_eq!(row.query.as_deref(), Some("page=1"));
        assert_eq!(row.status_code, Some(200));
        assert_eq!(row.content_type, "application/json");
        assert_eq!(row.ip, "93.184.216.34");
        assert_eq!(row.port, Some(443));
        assert_eq!(row.latency_ms, Some(160));
        assert_eq!(row.response_length, Some(42));
    }

    #[test]
    fn filters_by_host() {
        let har = r#"{
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {"method": "GET", "url": "https://a.com/1"},
                        "response": {"status": 200}
                    },
                    {
                        "request": {"method": "GET", "url": "https://b.com/2"},
                        "response": {"status": 200}
                    }
                ]
            }
        }"#;

        let rows = parse_har_from_str(har, Some("a.com")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host, "a.com");
    }

    #[test]
    fn skips_entries_with_empty_url() {
        let har = r#"{
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {"method": "GET", "url": ""},
                        "response": {"status": 200}
                    },
                    {
                        "request": {"method": "GET", "url": "https://example.com/ok"},
                        "response": {"status": 200}
                    }
                ]
            }
        }"#;

        let rows = parse_har_from_str(har, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/ok");
    }

    #[test]
    fn handles_missing_optional_fields() {
        let har = r#"{
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {"method": "POST", "url": "https://example.com/api"},
                        "response": {"status": 201}
                    }
                ]
            }
        }"#;

        let rows = parse_har_from_str(har, None).unwrap();
        assert_eq!(rows.len(), 1);

        let row = &rows[0];
        assert_eq!(row.method, "POST");
        assert_eq!(row.status_code, Some(201));
        assert!(row.request_body.is_empty());
        assert!(row.response_body.is_empty());
        assert!(row.content_type.is_empty());
        assert!(row.ip.is_empty());
        assert!(row.latency_ms.is_none());
        assert!(row.response_length.is_none());
    }

    #[test]
    fn normalizes_root_path() {
        let har = r#"{
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {"method": "GET", "url": "https://example.com"},
                        "response": {"status": 200}
                    }
                ]
            }
        }"#;

        let rows = parse_har_from_str(har, None).unwrap();
        assert_eq!(rows[0].path, "/");
    }

    #[test]
    fn http_default_port() {
        let har = r#"{
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {"method": "GET", "url": "http://example.com/page"},
                        "response": {"status": 200}
                    }
                ]
            }
        }"#;

        let rows = parse_har_from_str(har, None).unwrap();
        assert_eq!(rows[0].port, Some(80));
    }

    /// Helper for tests — parse HAR from a string instead of a file.
    fn parse_har_from_str(json: &str, host_filter: Option<&str>) -> Result<Vec<TrafficRow>> {
        let har: HarFile = serde_json::from_str(json)
            .map_err(|source| Error::json("failed to parse HAR", source))?;

        let mut rows = Vec::new();

        for (index, entry) in har.log.entries.iter().enumerate() {
            let url = entry.request.url.trim().to_string();
            let method = entry.request.method.trim().to_uppercase();

            if url.is_empty() || method.is_empty() {
                continue;
            }

            let parsed_url = match Url::parse(&url) {
                Ok(u) => u,
                Err(_) => continue,
            };

            let host = parsed_url.host_str().unwrap_or("").to_string();
            if host.is_empty() {
                continue;
            }

            if let Some(filter) = host_filter {
                if !host.starts_with(filter) {
                    continue;
                }
            }

            let path = if parsed_url.path().is_empty() {
                "/".to_string()
            } else {
                parsed_url.path().to_string()
            };

            let query = if parsed_url.query().is_none_or(|q| q.is_empty()) {
                None
            } else {
                Some(parsed_url.query().unwrap().to_string())
            };

            let port = extract_port(&parsed_url);

            let request_headers = format_headers(entry.request.headers.as_deref().unwrap_or(&[]));
            let response_headers = format_headers(entry.response.headers.as_deref().unwrap_or(&[]));

            let request_body = entry
                .request
                .post_data
                .as_ref()
                .and_then(|p| p.text.as_deref())
                .unwrap_or("")
                .to_string();

            let content = entry.response.content.as_ref();
            let response_body = content
                .and_then(|c| c.text.as_deref())
                .unwrap_or("")
                .to_string();

            let content_type = content
                .and_then(|c| c.mime_type.as_deref())
                .unwrap_or("")
                .to_string();

            let response_length =
                content.and_then(|c| c.size.filter(|&s| s >= 0).map(|s| s as u64));

            let latency_ms = entry.timings.as_ref().and_then(|t| {
                let wait = t.wait.unwrap_or(0.0);
                let receive = t.receive.unwrap_or(0.0);
                let send = t.send.unwrap_or(0.0);
                let total = wait + receive + send;
                if total > 0.0 {
                    Some(total as u64)
                } else {
                    None
                }
            });

            let request_time = entry
                .started_date_time
                .as_deref()
                .unwrap_or("")
                .to_string();

            let ip = entry
                .server_ip_address
                .as_deref()
                .unwrap_or("")
                .to_string();

            rows.push(TrafficRow {
                row_index: index,
                url,
                method,
                status_code: Some(entry.response.status),
                content_type,
                request_headers,
                response_headers,
                request_body,
                response_body,
                host,
                port,
                path,
                query,
                ip,
                latency_ms,
                response_length,
                request_time,
                title: String::new(),
                tags: String::new(),
            });
        }

        Ok(rows)
    }

    #[test]
    fn rejects_empty_file() {
        let dir = std::env::temp_dir().join("bizgraph_test_validation");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.har");
        std::fs::write(&path, "").unwrap();
        let result = parse_har(path.to_str().unwrap(), None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_json_file() {
        let dir = std::env::temp_dir().join("bizgraph_test_validation2");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not_har.har");
        std::fs::write(&path, "<!DOCTYPE html><html>not a HAR</html>").unwrap();
        let result = parse_har(path.to_str().unwrap(), None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not look like a HAR file"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_json_without_log_field() {
        let dir = std::env::temp_dir().join("bizgraph_test_validation3");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("no_log.har");
        std::fs::write(&path, r#"{"version": "1.0", "entries": []}"#).unwrap();
        let result = parse_har(path.to_str().unwrap(), None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing 'log' field"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

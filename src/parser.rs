use calamine::{open_workbook_auto, Data, Reader};

const YAKIT_PACKET_SEPARATOR: &str = "_x000d_\n";
const YAKIT_PACKET_HEADER_BODY_SEPARATOR: &str = "_x000d_\n_x000d_\n";

/// Internal traffic row extracted from a Yakit Excel export.
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

pub fn parse_yakit_excel(
    yakit_excel_path: &str,
    host_filter: Option<&str>,
) -> Result<Vec<TrafficRow>, String> {
    let mut workbook = open_workbook_auto(yakit_excel_path)
        .map_err(|err| format!("failed to open Yakit Excel '{}': {err}", yakit_excel_path))?;

    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| "Yakit Excel has no worksheets".to_string())?;

    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|err| format!("failed to read worksheet '{sheet_name}': {err}"))?;

    let mut parsed_rows = Vec::new();
    for (row_index, row) in range.rows().enumerate() {
        if row_index == 0 && looks_like_header(row) {
            continue;
        }

        let url = cell_string(row, 3);
        let method = cell_string(row, 1).to_uppercase();
        let host = cell_string(row, 4);
        let raw_path = cell_string(row, 5);

        if url.is_empty() || method.is_empty() || host.is_empty() {
            continue;
        }

        if let Some(filter) = host_filter {
            if !host.starts_with(filter) {
                continue;
            }
        }

        let (path, query) = split_path_and_query(&raw_path, &url);
        let request_packet = parse_http_packet(&cell_string(row, 17));
        let response_packet = parse_http_packet(&cell_string(row, 18));

        let port = extract_port_from_url(&url);

        parsed_rows.push(TrafficRow {
            row_index,
            url,
            method,
            status_code: parse_optional_u16(&cell_string(row, 2)),
            content_type: cell_string(row, 12),
            request_headers: request_packet.headers,
            response_headers: response_packet.headers,
            request_body: request_packet.body,
            response_body: response_packet.body,
            host,
            port,
            path,
            query,
            ip: cell_string(row, 8),
            latency_ms: parse_optional_u64(&cell_string(row, 14)),
            response_length: parse_optional_u64(&cell_string(row, 9)),
            request_time: cell_string(row, 15),
            title: cell_string(row, 10),
            tags: cell_string(row, 7),
        });
    }

    Ok(parsed_rows)
}

fn looks_like_header(row: &[Data]) -> bool {
    let sequence = cell_string(row, 0);
    let method = cell_string(row, 1);

    sequence.contains("序号") && method.contains("方法")
}

fn cell_string(row: &[Data], index: usize) -> String {
    row.get(index)
        .map(data_to_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn data_to_string(data: &Data) -> String {
    match data {
        Data::Empty => String::new(),
        Data::String(value) => value.clone(),
        Data::Float(value) => {
            if value.fract() == 0.0 {
                format!("{value:.0}")
            } else {
                value.to_string()
            }
        }
        Data::Int(value) => value.to_string(),
        Data::Bool(value) => value.to_string(),
        Data::DateTime(value) => value.to_string(),
        Data::DateTimeIso(value) => value.clone(),
        Data::DurationIso(value) => value.clone(),
        Data::Error(_) => String::new(),
    }
}

fn parse_optional_u16(value: &str) -> Option<u16> {
    value.trim().parse::<u16>().ok()
}

fn parse_optional_u64(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

fn split_path_and_query(raw_path: &str, url: &str) -> (String, Option<String>) {
    let candidate = if raw_path.trim().is_empty() {
        url_path_from_url(url)
    } else {
        raw_path.trim()
    };

    let raw_path = if candidate.is_empty() { "/" } else { candidate };

    let (path, query) = match raw_path.split_once('?') {
        Some((path, query)) => (path, Some(query.to_string())),
        None => (raw_path, None),
    };

    let path = if path.is_empty() { "/" } else { path };
    (path.to_string(), query)
}

fn url_path_from_url(url: &str) -> &str {
    if let Some(scheme_pos) = url.find("://") {
        match url[scheme_pos + 3..].find('/') {
            Some(path_pos) => &url[scheme_pos + 3 + path_pos..],
            None => "/",
        }
    } else if url.starts_with('/') {
        url
    } else {
        "/"
    }
}

fn extract_port_from_url(url: &str) -> Option<u16> {
    let trimmed = url.trim();
    let (scheme, remainder) = trimmed.split_once("://")?;
    let authority = remainder.split('/').next().unwrap_or(remainder);
    if authority.is_empty() {
        return default_port_for_scheme(scheme);
    }

    if let Some(port_text) = authority.rsplit_once(':').map(|(_, port)| port) {
        if !port_text.is_empty() && port_text.chars().all(|character| character.is_ascii_digit()) {
            if let Ok(port) = port_text.parse::<u16>() {
                return Some(port);
            }
        }
    }

    default_port_for_scheme(scheme)
}

fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct ParsedHttpPacket {
    headers: String,
    body: String,
}

fn parse_http_packet(packet: &str) -> ParsedHttpPacket {
    if packet.trim().is_empty() {
        return ParsedHttpPacket::default();
    }

    let normalized = packet.replace("\r\n", YAKIT_PACKET_SEPARATOR);
    let (header_block, body) = match normalized.split_once(YAKIT_PACKET_HEADER_BODY_SEPARATOR) {
        Some((headers, body)) => (headers, body),
        None => (normalized.as_str(), ""),
    };

    let headers = header_block
        .split(YAKIT_PACKET_SEPARATOR)
        .skip(1)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\r\n");

    ParsedHttpPacket {
        headers,
        body: body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_real_yakit_header_row() {
        let row = vec![
            Data::String("序号".to_string()),
            Data::String("方法".to_string()),
        ];

        assert!(looks_like_header(&row));
    }

    #[test]
    fn parses_http_packet_headers_and_body() {
        let packet = concat!(
            "POST /api/login HTTP/1.1_x000d_\n",
            "Host: example.com_x000d_\n",
            "Content-Type: application/json_x000d_\n",
            "_x000d_\n",
            "{\"ok\":true}"
        );

        let parsed = parse_http_packet(packet);

        assert_eq!(parsed.headers, "Host: example.com\r\nContent-Type: application/json");
        assert_eq!(parsed.body, "{\"ok\":true}");
    }

    #[test]
    fn splits_query_from_real_path_column() {
        let (path, query) = split_path_and_query("/common.js?sv=20260228", "https://example.com/common.js?sv=20260228");

        assert_eq!(path, "/common.js");
        assert_eq!(query.as_deref(), Some("sv=20260228"));
    }

    #[test]
    fn falls_back_to_default_port_from_scheme() {
        assert_eq!(extract_port_from_url("https://career.cmbc.com.cn/common.js"), Some(443));
        assert_eq!(extract_port_from_url("http://career.cmbc.com.cn/common.js"), Some(80));
        assert_eq!(extract_port_from_url("https://career.cmbc.com.cn:8443/common.js"), Some(8443));
    }
}

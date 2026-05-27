use calamine::{open_workbook_auto, Data, Reader};

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

        let url = cell_string(row, 0);
        let method = cell_string(row, 1).to_uppercase();
        let host = cell_string(row, 10);

        if url.is_empty() || method.is_empty() || host.is_empty() {
            continue;
        }

        if let Some(filter) = host_filter {
            if !host.starts_with(filter) {
                continue;
            }
        }

        let (path, query) = split_url_path_and_query(&url);

        parsed_rows.push(TrafficRow {
            row_index,
            url,
            method,
            status_code: parse_optional_u16(&cell_string(row, 2)),
            content_type: cell_string(row, 14),
            request_headers: cell_string(row, 6),
            response_headers: cell_string(row, 7),
            request_body: cell_string(row, 8),
            response_body: cell_string(row, 9),
            host,
            port: parse_optional_u16(&cell_string(row, 11)),
            path,
            query,
        });
    }

    Ok(parsed_rows)
}

fn looks_like_header(row: &[Data]) -> bool {
    let url = cell_string(row, 0).to_ascii_lowercase();
    let method = cell_string(row, 1).to_ascii_lowercase();
    let host = cell_string(row, 10).to_ascii_lowercase();

    matches!(url.as_str(), "url" | "链接" | "地址")
        && matches!(method.as_str(), "method" | "方法")
        && host == "host"
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

fn split_url_path_and_query(url: &str) -> (String, Option<String>) {
    let raw_path = if let Some(scheme_pos) = url.find("://") {
        match url[scheme_pos + 3..].find('/') {
            Some(path_pos) => &url[scheme_pos + 3 + path_pos..],
            None => "/",
        }
    } else if url.starts_with('/') {
        url
    } else {
        "/"
    };

    let (path, query) = match raw_path.split_once('?') {
        Some((path, query)) => (path, Some(query.to_string())),
        None => (raw_path, None),
    };

    let path = if path.is_empty() { "/" } else { path };
    (path.to_string(), query)
}

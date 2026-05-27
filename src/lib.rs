pub mod graph;
pub mod parser;
pub mod types;

use graph::build_business_graph;
use parser::parse_yakit_excel;
use types::BusinessGraph;

/// Analyze a Yakit Excel traffic export into a deterministic business graph.
pub fn analyze(yakit_excel_path: &str, host_filter: Option<&str>) -> Result<BusinessGraph, String> {
    let rows = parse_yakit_excel(yakit_excel_path, host_filter)?;
    build_business_graph(&rows)
}

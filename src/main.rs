use std::collections::BTreeMap;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "bizgraph",
    about = "HAR traffic → Business graph mapper",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze HAR traffic and persist to a project.
    /// Parses the HAR file, identifies business functions via AI (if configured),
    /// builds a deterministic business graph, and merges it into the project.
    /// Supports incremental analysis: only new endpoints are sent to AI.
    Analyze {
        /// Path to HAR file (.har)
        har_path: String,

        /// Project name or ID (auto-creates if not exists)
        #[arg(long = "project", short = 'p')]
        project: String,

        /// Filter traffic by Host column (prefix match)
        #[arg(long = "host", short = 'H')]
        host: Option<String>,
    },
    /// Manage persisted graph projects
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Ask a question about a project traffic data (conversational, with context)
    Ask {
        /// The question to ask
        question: String,

        /// Project name or ID
        #[arg(long = "project", short = 'p')]
        project: String,

        /// Clear conversation history before asking
        #[arg(long, help = "Start a fresh conversation")]
        clear: bool,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Create a new empty project
    New { name: String },
    /// List all projects with their IDs and creation dates
    List,
    /// Show project overview: stats, business function tree, and AI report preview
    Show { name: String },
    /// Show analysis history with timestamps and change stats
    History { name: String },
    /// Export the full graph as JSON (nodes, edges, business functions, AI report)
    Export {
        name: String,
        #[arg(short = 'o', help = "Output file path (default: stdout)")]
        output: Option<String>,
    },
    /// Generate an interactive HTML visualization of the business graph
    Viz {
        name: String,
        #[arg(short = 'o', default_value = "graph.html", help = "Output HTML file")]
        output: String,
    },
    /// Compare the last two analyses and show added/removed nodes
    Diff { name: String },
    /// Delete a project and all its data (nodes, edges, analyses)
    Delete {
        name: String,
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
    },
    /// Show the full AI analysis report for a project
    Report { name: String },
}

#[tokio::main]
async fn main() {
    // Graceful shutdown on Ctrl+C
    tokio::spawn(async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\n⛔ Interrupted.");
        std::process::exit(130);
    });

    let cli = Cli::parse();
    match cli.command {
        Command::Analyze {
            har_path,
            host,
            project,
        } => {
            let resolved_ai_config = bizgraph::try_load_config();

            match bizgraph::analyze_with_project(
                &har_path,
                host.as_deref(),
                &project,
                resolved_ai_config
                    .as_ref()
                    .map(|(api_key, _, _)| api_key.as_str()),
                resolved_ai_config
                    .as_ref()
                    .map(|(_, model, _)| model.as_str()),
                resolved_ai_config
                    .as_ref()
                    .map(|(_, _, api_url)| api_url.as_str()),
                None,
            )
            .await
            {
                Ok(result) => {
                    print_project_import_summary(
                        &result.project.name,
                        &result.graph,
                        &result.stats,
                    );
                    print_business_tree(&result.graph);
                    print_ai_preview(result.ai_report.as_deref());
                }
                Err(error) => {
                    eprintln!("Error: {error}");
                    std::process::exit(1);
                }
            }
        }
        Command::Project { action } => {
            let db = match bizgraph::Database::open_default() {
                Ok(db) => db,
                Err(error) => {
                    eprintln!("Error: {error}");
                    std::process::exit(1);
                }
            };

            let outcome = match action {
                ProjectAction::New { name } => db.create_project(&name).map(|project| {
                    println!("Created project '{}' ({})", project.name, short_id(&project.id));
                }),
                ProjectAction::List => db.list_projects().map(|projects| {
                    if projects.is_empty() {
                        println!("No projects found.");
                        return;
                    }

                    println!("{:<10} {:<26} NAME", "ID", "CREATED");
                    for project in projects {
                        println!("{:<10} {:<26} {}", short_id(&project.id), project.created_at.to_rfc3339(), project.name);
                    }
                }),
                ProjectAction::Show { name } => resolve_project(&db, &name).and_then(|project| {
                    let graph = db.get_graph(project.id)?;
                    let history = db.get_analysis_history(project.id)?;
                    let counts = node_counts(&graph);
                    println!("Project: {}", project.name);
                    println!("ID: {}", short_id(&project.id));
                    println!("Created: {}", project.created_at.to_rfc3339());
                    println!("Nodes: {} total", graph.nodes.len());
                    for (kind, count) in counts {
                        println!("  {kind}: {count}");
                    }
                    println!("Edges: {} total", graph.edges.len());
                    println!("Analyses: {} total", history.len());
                    if let Some(last) = history.last() {
                        println!(
                            "Last analysis: {} (rows={}, +nodes={}, ~nodes={}, +edges={}, skipped_edges={})",
                            last.created_at.to_rfc3339(),
                            last.row_count,
                            last.new_nodes,
                            last.updated_nodes,
                            last.new_edges,
                            last.skipped_edges,
                        );
                    }
                    print_graph_metrics(&graph);
                    print_business_tree(&graph);
                    if let Some(latest) = db.get_latest_analysis(project.id)? {
                        print_ai_preview(latest.ai_report.as_deref());
                    }
                    Ok(())
                }),
                ProjectAction::History { name } => resolve_project(&db, &name).and_then(|project| {
                    let history = db.get_analysis_history(project.id)?;
                    if history.is_empty() {
                        println!("No analysis history for '{}' yet.", project.name);
                        return Ok(());
                    }

                    println!("History for {}:", project.name);
                    for record in history {
                        println!(
                            "{}\trows={}\t+nodes={}\t~nodes={}\t+edges={}\tskipped_edges={}\thost={}\tsource={}",
                            record.created_at.to_rfc3339(),
                            record.row_count,
                            record.new_nodes,
                            record.updated_nodes,
                            record.new_edges,
                            record.skipped_edges,
                            record.host_filter.as_deref().unwrap_or("-"),
                            record.source_path.as_deref().unwrap_or("-"),
                        );
                    }
                    Ok(())
                }),
                ProjectAction::Export { name, output } => resolve_project(&db, &name).and_then(|project| {
                    let graph = db.get_graph(project.id)?;
                    let ai_report = db.get_latest_analysis(project.id)?
                        .and_then(|a| a.ai_report);

                    // Build export with business summary
                    let mut export = serde_json::Map::new();
                    export.insert("project".to_string(), serde_json::json!({
                        "name": project.name,
                        "id": short_id(&project.id),
                        "created_at": project.created_at.to_rfc3339(),
                    }));

                    // Business functions summary
                    use bizgraph::types::{BusinessNodeKind, BusinessNodeProperties};
                    let mut business_functions = Vec::new();
                    for node in &graph.nodes {
                        if node.kind == BusinessNodeKind::BusinessFunction {
                            if let BusinessNodeProperties::BusinessFunction(props) = &node.properties {
                                business_functions.push(serde_json::json!({
                                    "name": node.label,
                                    "host": props.host,
                                    "path_prefix": props.path_prefix,
                                    "endpoint_count": props.endpoint_count,
                                    "description": props.description,
                                }));
                            }
                        }
                    }
                    export.insert("business_functions".to_string(), serde_json::Value::Array(business_functions));
                    export.insert("graph".to_string(), serde_json::to_value(&graph)
                        .map_err(|source| bizgraph::Error::json("failed to serialize graph", source))?);
                    if let Some(report) = ai_report {
                        export.insert("ai_report".to_string(), serde_json::Value::String(report));
                    }

                    let json = serde_json::to_string_pretty(&export)
                        .map_err(|source| bizgraph::Error::json("failed to serialize export", source))?;
                    if let Some(path) = output {
                        std::fs::write(&path, &json)
                            .map_err(|source| bizgraph::Error::io(format!("failed to write export '{path}'"), source))?;
                        eprintln!("Exported '{}' to {}", project.name, path);
                    } else {
                        println!("{json}");
                    }
                    Ok(())
                }),
                ProjectAction::Viz { name, output } => resolve_project(&db, &name).and_then(|project| {
                    let graph = db.get_graph(project.id)?;
                    let html = generate_viz_html(&graph, &project.name);
                    std::fs::write(&output, &html)
                        .map_err(|source| bizgraph::Error::io(format!("failed to write viz '{output}'"), source))?;
                    eprintln!("Visualization '{}' → {}", project.name, output);
                    Ok(())
                }),
                ProjectAction::Diff { name } => resolve_project(&db, &name).and_then(|project| {
                    let history = db.get_analysis_history(project.id)?;
                    if history.len() < 2 {
                        eprintln!("Need at least 2 analyses to diff. Current: {}", history.len());
                        return Ok(());
                    }

                    let prev = &history[history.len() - 2];
                    let curr = &history[history.len() - 1];

                    println!("Diff: {} (last two analyses)\n", project.name);
                    println!("  Previous: {}", prev.created_at.to_rfc3339());
                    println!("  Current:  {}\n", curr.created_at.to_rfc3339());

                    // Stats delta
                    println!("Stats change:");
                    println!("  Rows:    {} → {} ({:+})", prev.row_count, curr.row_count, curr.row_count as isize - prev.row_count as isize);
                    println!("  +Nodes:  {} → {}", prev.new_nodes, curr.new_nodes);
                    println!("  ~Nodes:  {} → {}", prev.updated_nodes, curr.updated_nodes);
                    println!("  +Edges:  {} → {}", prev.new_edges, curr.new_edges);

                    // Node diff from snapshots
                    let prev_keys: std::collections::HashSet<String> = prev
                        .node_snapshot
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                        .map(|v| v.into_iter().collect())
                        .unwrap_or_default();
                    let curr_keys: std::collections::HashSet<String> = curr
                        .node_snapshot
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                        .map(|v| v.into_iter().collect())
                        .unwrap_or_default();

                    if prev_keys.is_empty() && curr_keys.is_empty() {
                        eprintln!("\n(No node snapshots available for detailed diff)");
                        return Ok(());
                    }

                    let added: Vec<&String> = curr_keys.difference(&prev_keys).collect();
                    let removed: Vec<&String> = prev_keys.difference(&curr_keys).collect();

                    // Group by type and filter static resources
                    let mut added_bfs: Vec<&&String> = added.iter().filter(|k| k.starts_with("bf:")).collect();
                    let mut added_eps: Vec<&&String> = added.iter().filter(|k| k.starts_with("ep:")).collect();
                    let mut removed_bfs: Vec<&&String> = removed.iter().filter(|k| k.starts_with("bf:")).collect();
                    let mut removed_eps: Vec<&&String> = removed.iter().filter(|k| k.starts_with("ep:")).collect();

                    added_bfs.sort();
                    added_eps.sort();
                    removed_bfs.sort();
                    removed_eps.sort();

                    // Filter out static resource endpoints
                    let static_exts = ["js", "css", "png", "jpg", "jpeg", "gif", "svg", "ico", "woff", "woff2", "ttf", "eot", "otf"];
                    let is_static = |key: &str| -> bool {
                        if let Some(ep_part) = key.strip_prefix("ep:") {
                            if let Some(path_part) = ep_part.split(':').skip(2).collect::<Vec<_>>().join(":").split(':').next_back() {
                                return static_exts.iter().any(|ext| path_part.ends_with(&format!(".{}", ext)));
                            }
                        }
                        false
                    };

                    let added_eps_static: Vec<&&String> = added_eps.iter().filter(|k| is_static(k)).copied().collect();
                    let added_eps_dynamic: Vec<&&String> = added_eps.iter().filter(|k| !is_static(k)).copied().collect();
                    let removed_eps_static: Vec<&&String> = removed_eps.iter().filter(|k| is_static(k)).copied().collect();
                    let removed_eps_dynamic: Vec<&&String> = removed_eps.iter().filter(|k| !is_static(k)).copied().collect();

                    // Print BF changes
                    if !added_bfs.is_empty() {
                        println!("\n+ Added business functions ({}):", added_bfs.len());
                        for key in &added_bfs {
                            println!("    + {}", key.strip_prefix("bf:").unwrap_or(key));
                        }
                    }
                    if !removed_bfs.is_empty() {
                        println!("\n- Removed business functions ({}):", removed_bfs.len());
                        for key in &removed_bfs {
                            println!("    - {}", key.strip_prefix("bf:").unwrap_or(key));
                        }
                    }

                    // Print dynamic endpoint changes
                    if !added_eps_dynamic.is_empty() {
                        println!("\n+ Added endpoints ({}):", added_eps_dynamic.len());
                        for key in &added_eps_dynamic {
                            let display = key.strip_prefix("ep:").unwrap_or(key);
                            println!("    + {}", display);
                        }
                    }
                    if !removed_eps_dynamic.is_empty() {
                        println!("\n- Removed endpoints ({}):", removed_eps_dynamic.len());
                        for key in &removed_eps_dynamic {
                            let display = key.strip_prefix("ep:").unwrap_or(key);
                            println!("    - {}", display);
                        }
                    }

                    // Print static resource summary
                    if !added_eps_static.is_empty() || !removed_eps_static.is_empty() {
                        println!("\nStatic resources: +{} -{}", added_eps_static.len(), removed_eps_static.len());
                    }

                    if added_bfs.is_empty() && removed_bfs.is_empty()
                        && added_eps_dynamic.is_empty() && removed_eps_dynamic.is_empty()
                        && added_eps_static.is_empty() && removed_eps_static.is_empty() {
                        println!("\nNo node changes (same set of {} nodes)", curr_keys.len());
                    }

                    // Report comparison — show AI report diff if both have reports
                    if let (Some(prev_report), Some(curr_report)) = (&prev.ai_report, &curr.ai_report) {
                        let prev_sections = extract_report_sections(prev_report);
                        let curr_sections = extract_report_sections(curr_report);
                        let prev_titles: std::collections::HashSet<&str> = prev_sections.iter().copied().collect();
                        let curr_titles: std::collections::HashSet<&str> = curr_sections.iter().copied().collect();

                        let added_sections: Vec<&&str> = curr_titles.difference(&prev_titles).collect();
                        let removed_sections: Vec<&&str> = prev_titles.difference(&curr_titles).collect();

                        if !added_sections.is_empty() || !removed_sections.is_empty() {
                            println!("\nReport sections changed:");
                            for title in &added_sections {
                                println!("    + {}", title);
                            }
                            for title in &removed_sections {
                                println!("    - {}", title);
                            }
                        }
                    }

                    Ok(())
                }),
                ProjectAction::Delete { name, force } => resolve_project(&db, &name).and_then(|project| {
                    if !force {
                        eprintln!("This will permanently delete project '{}' and all its data.", project.name);
                        eprintln!("Use --force to skip this confirmation.");
                        std::process::exit(1);
                    }
                    db.delete_project(project.id)?;
                    eprintln!("Deleted project '{}'", project.name);
                    Ok(())
                }),
                ProjectAction::Report { name } => resolve_project(&db, &name).and_then(|project| {
                    let history = db.get_analysis_history(project.id)?;
                    let latest_with_report = history.iter().rev().find(|r| r.ai_report.is_some());
                    match latest_with_report {
                        Some(record) => {
                            println!("AI Report for '{}' ({}):\n", project.name, record.created_at.to_rfc3339());
                            println!("{}", record.ai_report.as_deref().unwrap_or("(empty)"));
                        }
                        None => {
                            eprintln!("No AI report found for project '{}'", project.name);
                        }
                    }
                    Ok(())
                }),
            };

            if let Err(error) = outcome {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Command::Ask { question, project, clear } => {
            if clear {
                if let Err(e) = bizgraph::clear_conversation(&project) {
                    eprintln!("Warning: could not clear conversation: {e}");
                } else {
                    println!("Conversation history cleared.");
                }
            }
            match bizgraph::ask(&project, &question).await {
                Ok(answer) => println!("{}", answer),
                Err(error) => {
                    eprintln!("Error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}

#[allow(clippy::result_large_err)]
fn resolve_project(
    db: &bizgraph::Database,
    name_or_id: &str,
) -> bizgraph::Result<bizgraph::types::Project> {
    db.resolve_project(name_or_id)?
        .ok_or_else(|| bizgraph::Error::ProjectNotFound {
            reference: name_or_id.to_string(),
        })
}

fn print_project_import_summary(
    project_name: &str,
    graph: &bizgraph::types::BusinessGraph,
    stats: &bizgraph::types::AnalysisStats,
) {
    println!("Project: {project_name}");
    println!("Imported rows: {}", stats.row_count);
    println!("New nodes: {}", stats.new_nodes);
    println!("Updated nodes: {}", stats.updated_nodes);
    println!("New edges: {}", stats.new_edges);
    println!("Skipped edges: {}", stats.skipped_edges);
    print_graph_summary(graph);
}

fn print_graph_summary(graph: &bizgraph::types::BusinessGraph) {
    let counts = node_counts(graph);
    println!("Nodes: {} total", graph.nodes.len());
    for (kind, count) in counts {
        println!("  {kind}: {count}");
    }
    println!("Edges: {} total", graph.edges.len());
}

fn short_id(id: &uuid::Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn node_counts(graph: &bizgraph::types::BusinessGraph) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for node in &graph.nodes {
        let key = match node.kind {
            bizgraph::types::BusinessNodeKind::Host => "host",
            bizgraph::types::BusinessNodeKind::BusinessFunction => "business_function",
            bizgraph::types::BusinessNodeKind::Endpoint => "endpoint",
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

fn print_ai_preview(report: Option<&str>) {
    if let Some(report) = report {
        let preview: String = report.chars().take(500).collect();
        println!("\nAI Report Preview:\n{preview}\n...");
    }
}

/// Extract section titles (## headings) from an AI report for comparison.
fn extract_report_sections(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter(|line| line.starts_with("## "))
        .map(|line| line.trim_start_matches("## ").trim())
        .collect()
}

fn print_graph_metrics(graph: &bizgraph::types::BusinessGraph) {
    use std::collections::HashMap;

    if graph.nodes.is_empty() {
        return;
    }

    // Count fan-in (incoming edges) and fan-out (outgoing edges) per node UUID
    let mut fan_in: HashMap<uuid::Uuid, usize> = HashMap::new();
    let mut fan_out: HashMap<uuid::Uuid, usize> = HashMap::new();
    for edge in &graph.edges {
        *fan_out.entry(edge.source_node_id).or_insert(0) += 1;
        *fan_in.entry(edge.target_node_id).or_insert(0) += 1;
    }

    // Orphan nodes (no edges at all)
    let orphan_count = graph
        .nodes
        .iter()
        .filter(|n| !fan_in.contains_key(&n.id) && !fan_out.contains_key(&n.id))
        .count();

    // Max fan-in (most depended-on)
    let max_fan_in = fan_in.iter().max_by_key(|(_, v)| *v);
    // Max fan-out (most dependencies)
    let max_fan_out = fan_out.iter().max_by_key(|(_, v)| *v);

    // Cross-host edges — extract host from endpoint stable_key (format: ep:<method>:<host>:<path>)
    let node_hosts: HashMap<uuid::Uuid, &str> = graph
        .nodes
        .iter()
        .filter_map(|n| {
            if let bizgraph::types::BusinessNodeKind::Endpoint = n.kind {
                let key = n.stable_key.as_str();
                if let Some(rest) = key.strip_prefix("ep:") {
                    let parts: Vec<&str> = rest.splitn(3, ':').collect();
                    if parts.len() >= 2 {
                        return Some((n.id, parts[1]));
                    }
                }
            }
            None
        })
        .collect();

    let cross_host = graph
        .edges
        .iter()
        .filter(|e| {
            e.label.starts_with("data_dependency")
                && match (node_hosts.get(&e.source_node_id), node_hosts.get(&e.target_node_id)) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                }
        })
        .count();

    println!("Graph Metrics:");
    if let Some((uuid, count)) = max_fan_in {
        let label = graph
            .nodes
            .iter()
            .find(|n| n.id == *uuid)
            .map(|n| n.label.as_str())
            .unwrap_or("unknown");
        println!("  Most depended-on: {} (fan-in: {})", label, count);
    }
    if let Some((uuid, count)) = max_fan_out {
        let label = graph
            .nodes
            .iter()
            .find(|n| n.id == *uuid)
            .map(|n| n.label.as_str())
            .unwrap_or("unknown");
        println!("  Most dependencies: {} (fan-out: {})", label, count);
    }
    if orphan_count > 0 {
        println!("  Orphan nodes: {} (no edges)", orphan_count);
    }
    if cross_host > 0 {
        println!("  Cross-host calls: {} (between different domains)", cross_host);
    }
}

fn print_business_tree(graph: &bizgraph::types::BusinessGraph) {
    use bizgraph::types::{BusinessNodeKind, BusinessNodeProperties};

    // Collect contains edges: parent_id → child_ids
    let mut contains_children: BTreeMap<uuid::Uuid, Vec<uuid::Uuid>> = BTreeMap::new();
    for edge in &graph.edges {
        if edge.label == "contains" {
            contains_children
                .entry(edge.source_node_id)
                .or_default()
                .push(edge.target_node_id);
        }
    }

    // Index nodes by id
    let node_by_id: BTreeMap<uuid::Uuid, &bizgraph::types::BusinessNode> =
        graph.nodes.iter().map(|n| (n.id, n)).collect();

    // Group business functions by host
    let mut host_bfs: BTreeMap<String, Vec<&bizgraph::types::BusinessNode>> = BTreeMap::new();
    for node in &graph.nodes {
        if node.kind == BusinessNodeKind::BusinessFunction {
            if let BusinessNodeProperties::BusinessFunction(props) = &node.properties {
                host_bfs
                    .entry(props.host.clone())
                    .or_default()
                    .push(node);
            }
        }
    }

    if host_bfs.is_empty() {
        return;
    }

    println!("\nBusiness Structure:");
    for (host, bfs) in &host_bfs {
        if host.is_empty() {
            // AI-identified business functions (no host grouping)
            let mut sorted_bfs = bfs.clone();
            sorted_bfs.sort_by_key(|n| &n.label);
            for bf in &sorted_bfs {
                print_bf_node(bf, &contains_children, &node_by_id);
            }
        } else {
            println!("  [host] {host}");
            let mut sorted_bfs = bfs.clone();
            sorted_bfs.sort_by_key(|n| &n.label);
            for bf in &sorted_bfs {
                print_bf_node(bf, &contains_children, &node_by_id);
            }
        }
    }
}

fn print_bf_node(
    bf: &bizgraph::types::BusinessNode,
    contains_children: &BTreeMap<uuid::Uuid, Vec<uuid::Uuid>>,
    node_by_id: &BTreeMap<uuid::Uuid, &bizgraph::types::BusinessNode>,
) {
    use bizgraph::types::BusinessNodeProperties;
    if let BusinessNodeProperties::BusinessFunction(props) = &bf.properties {
        let ep_count = props.endpoint_count;
        println!("    [bf] {}  ({ep_count} endpoints)", bf.label);
        if let Some(desc) = &props.description {
            if !desc.is_empty() {
                println!("        {desc}");
            }
        }

        let ep_ids = contains_children.get(&bf.id).map(Vec::as_slice).unwrap_or(&[]);
        let mut eps: Vec<&&bizgraph::types::BusinessNode> = ep_ids
            .iter()
            .filter_map(|id| node_by_id.get(id))
            .filter(|n| n.kind == bizgraph::types::BusinessNodeKind::Endpoint)
            .collect();
        eps.sort_by_key(|n| &n.label);

        for ep in &eps {
            if let BusinessNodeProperties::Endpoint(props) = &ep.properties {
                let methods = props.methods.join(",");
                let params = if props.parameters.is_empty() {
                    String::new()
                } else {
                    let names: Vec<&str> = props.parameters.iter().map(|p| p.name.as_str()).collect();
                    format!("  params: [{}]", names.join(", "))
                };
                let conf = format!("{:.0}%", props.confidence * 100.0);
                println!("      {methods:<6} {}  {conf}{params}", props.path_template);
            }
        }
    }
}

fn generate_viz_html(graph: &bizgraph::types::BusinessGraph, project_name: &str) -> String {
    use bizgraph::types::{BusinessNodeKind, BusinessNodeProperties};

    let mut nodes_json = Vec::new();
    let mut edges_json = Vec::new();

    // Group business functions by host
    let mut host_bfs: BTreeMap<String, Vec<&bizgraph::types::BusinessNode>> = BTreeMap::new();
    for node in &graph.nodes {
        if node.kind == BusinessNodeKind::BusinessFunction {
            if let BusinessNodeProperties::BusinessFunction(props) = &node.properties {
                host_bfs
                    .entry(props.host.clone())
                    .or_default()
                    .push(node);
            }
        }
    }

    // Create host nodes
    for host in host_bfs.keys() {
        nodes_json.push(serde_json::json!({
            "id": format!("host:{host}"),
            "label": host,
            "kind": "host",
            "description": serde_json::Value::Null,
            "details": serde_json::Value::Null,
        }));
    }

    // Create business function nodes
    for (host, bfs) in &host_bfs {
        let mut sorted_bfs = bfs.clone();
        sorted_bfs.sort_by_key(|n| &n.label);
        for bf in &sorted_bfs {
            if let BusinessNodeProperties::BusinessFunction(props) = &bf.properties {
                nodes_json.push(serde_json::json!({
                    "id": bf.id.to_string(),
                    "label": bf.label,
                    "kind": "business_function",
                    "description": props.description,
                    "endpoint_count": props.endpoint_count,
                    "host": host,
                    "details": serde_json::json!({
                        "host": host,
                        "path_prefix": props.path_prefix,
                        "endpoint_count": props.endpoint_count,
                    }),
                }));
                edges_json.push(serde_json::json!({
                    "from": format!("host:{host}"),
                    "to": bf.id.to_string(),
                    "label": "contains",
                    "edge_type": "hierarchy",
                }));
            }
        }
    }

    // Index nodes by id
    let node_by_id: BTreeMap<uuid::Uuid, &bizgraph::types::BusinessNode> =
        graph.nodes.iter().map(|n| (n.id, n)).collect();

    // Collect contains edges: parent → children
    let mut contains_children: BTreeMap<uuid::Uuid, Vec<uuid::Uuid>> = BTreeMap::new();
    for edge in &graph.edges {
        if edge.label == "contains" {
            contains_children
                .entry(edge.source_node_id)
                .or_default()
                .push(edge.target_node_id);
        }
    }

    // Create endpoint nodes with full details
    for node in &graph.nodes {
        if node.kind == BusinessNodeKind::BusinessFunction {
            let ep_ids = contains_children.get(&node.id).map(Vec::as_slice).unwrap_or(&[]);
            for ep_id in ep_ids {
                if let Some(ep) = node_by_id.get(ep_id) {
                    if let BusinessNodeProperties::Endpoint(props) = &ep.properties {
                        let methods = props.methods.join(",");
                        let label = format!("{methods} {}", props.path_template);
                        let param_names: Vec<&str> = props.parameters.iter().map(|p| p.name.as_str()).collect();
                        let desc_parts: Vec<String> = vec![
                            format!("Methods: {methods}"),
                            format!("Path: {}", props.path_template),
                            format!("Confidence: {:.0}%", props.confidence * 100.0),
                            if param_names.is_empty() { String::new() } else { format!("Parameters: {}", param_names.join(", ")) },
                        ].into_iter().filter(|s| !s.is_empty()).collect();
                        nodes_json.push(serde_json::json!({
                            "id": ep.id.to_string(),
                            "label": label,
                            "kind": "endpoint",
                            "description": desc_parts.join("\n"),
                            "confidence": props.confidence,
                            "methods": props.methods,
                            "path": props.path_template,
                            "parameters": props.parameters.iter().map(|p| serde_json::json!({
                                "name": p.name, "location": p.location
                            })).collect::<Vec<_>>(),
                            "details": serde_json::json!({
                                "methods": props.methods,
                                "path": props.path_template,
                                "confidence": props.confidence,
                                "parameters": props.parameters,
                            }),
                        }));
                        edges_json.push(serde_json::json!({
                            "from": node.id.to_string(),
                            "to": ep.id.to_string(),
                            "label": "contains",
                            "edge_type": "hierarchy",
                        }));
                    }
                }
            }
        }
    }

    // Add calls_after edges (sequential flow between endpoints)
    for edge in &graph.edges {
        if edge.label == "calls_after" {
            if node_by_id.contains_key(&edge.source_node_id) && node_by_id.contains_key(&edge.target_node_id) {
                edges_json.push(serde_json::json!({
                    "from": edge.source_node_id.to_string(),
                    "to": edge.target_node_id.to_string(),
                    "label": "calls_after",
                    "edge_type": "flow",
                }));
            }
        }
    }

    // Add data_dependency edges
    for edge in &graph.edges {
        if edge.label.starts_with("data_dependency") {
            if node_by_id.contains_key(&edge.source_node_id) && node_by_id.contains_key(&edge.target_node_id) {
                edges_json.push(serde_json::json!({
                    "from": edge.source_node_id.to_string(),
                    "to": edge.target_node_id.to_string(),
                    "label": edge.label.clone(),
                    "edge_type": "dependency",
                }));
            }
        }
    }

    let data_json = serde_json::json!({
        "nodes": nodes_json,
        "edges": edges_json,
    });

    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str(&format!("<title>{project_name} — Business Graph</title>\n"));
    html.push_str("<script src=\"https://unpkg.com/vis-network/standalone/umd/vis-network.min.js\"></script>\n");
    html.push_str(r#"<style>
body { margin: 0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #1a1a2e; color: #e0e0e0; }
#graph { width: 100vw; height: 100vh; }
#controls { position: fixed; top: 16px; left: 16px; z-index: 10; background: rgba(30,30,60,0.9); padding: 12px 16px; border-radius: 8px; box-shadow: 0 2px 12px rgba(0,0,0,0.3); }
#controls h3 { margin: 0 0 8px 0; color: #7BC67E; }
#controls label { display: block; margin: 4px 0; cursor: pointer; }
#controls input[type="checkbox"] { margin-right: 6px; }
#stats { position: fixed; bottom: 16px; left: 16px; z-index: 10; background: rgba(30,30,60,0.9); padding: 8px 12px; border-radius: 6px; font-size: 13px; }
.legend { display: flex; gap: 16px; margin-top: 8px; }
.legend-item { display: flex; align-items: center; gap: 4px; }
.legend-color { width: 12px; height: 12px; border-radius: 2px; }
</style>
</head>
<body>
"#);
    html.push_str(r#"<div id="controls">
  <h3>"#);
    html.push_str(project_name);
    html.push_str(r#"</h3>
  <label><input type="checkbox" id="showEndpoints" onchange="toggleEndpoints()"> Endpoints</label>
  <label><input type="checkbox" id="showDescriptions" checked onchange="toggleDescriptions()"> 业务描述</label>
  <label><input type="checkbox" id="showFlow" checked onchange="toggleFlow()"> 调用顺序</label>
  <label><input type="checkbox" id="showDeps" checked onchange="toggleDeps()"> 数据依赖</label>
  <div class="legend">
    <div class="legend-item"><div class="legend-color" style="background:#4A90D9"></div> Host</div>
    <div class="legend-item"><div class="legend-color" style="background:#7BC67E"></div> 业务功能</div>
    <div class="legend-item"><div class="legend-color" style="background:#F5A623"></div> Endpoint</div>
    <div class="legend-item"><div class="legend-color" style="background:#999"></div> contains</div>
    <div class="legend-item"><div class="legend-color" style="background:#4A90D9"></div> calls_after</div>
    <div class="legend-item"><div class="legend-color" style="background:#E74C3C"></div> data_dependency</div>
  </div>
</div>
<div id="detail-panel" style="display:none; position:fixed; top:16px; right:16px; z-index:20; background:rgba(30,30,60,0.95); padding:16px 20px; border-radius:8px; box-shadow:0 2px 12px rgba(0,0,0,0.4); max-width:400px; max-height:80vh; overflow-y:auto; font-size:13px; line-height:1.6;">
  <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom:8px;">
    <h3 id="detail-title" style="margin:0; color:#7BC67E;"></h3>
    <span onclick="closeDetail()" style="cursor:pointer; color:#999; font-size:18px;">✕</span>
  </div>
  <div id="detail-content"></div>
</div>
"#);
    html.push_str("<div id=\"stats\"></div>\n<div id=\"graph\"></div>\n<script>\n");
    html.push_str(&format!("const DATA = {data_json};\n\n"));
    html.push_str(r#"const colors = { host: '#4A90D9', business_function: '#7BC67E', endpoint: '#F5A623' };
const shapes = { host: 'box', business_function: 'box', endpoint: 'ellipse' };
const fontSizes = { host: 16, business_function: 13, endpoint: 9 };

const visNodes = DATA.nodes.map(n => {
  let label = n.label;
  if (n.kind === 'business_function' && n.description) {
    label = n.label + '\n' + n.description;
  }
  if (n.kind === 'endpoint' && n.label.length > 50) {
    label = n.label.substring(0, 47) + '...';
  }
  return {
    id: n.id,
    label: label,
    color: { background: colors[n.kind] || '#CCC', border: colors[n.kind] || '#CCC' },
    font: { color: n.kind === 'host' ? '#fff' : '#333', size: fontSizes[n.kind] || 10 },
    shape: shapes[n.kind] || 'dot',
    title: n.label + (n.description ? '\n' + n.description : ''),
    hidden: n.kind === 'endpoint',
  };
});

const edgeStyles = {
  hierarchy: { color: { color: '#999', opacity: 0.5 }, dashes: false, arrows: 'to', width: 1, smooth: { type: 'cubicBezier' } },
  flow: { color: { color: '#4A90D9', opacity: 0.8 }, dashes: false, arrows: 'to', width: 2, smooth: { type: 'cubicBezier' }, font: { color: '#4A90D9', size: 9, strokeWidth: 0 } },
  dependency: { color: { color: '#E74C3C', opacity: 0.7 }, dashes: [5, 5], arrows: 'to', width: 1.5, smooth: { type: 'cubicBezier' }, font: { color: '#E74C3C', size: 9, strokeWidth: 0 } },
};
const visEdges = DATA.edges.map(e => {
  const style = edgeStyles[e.edge_type] || edgeStyles.hierarchy;
  return { id: e.from + '->' + e.to + ':' + e.edge_type, from: e.from, to: e.to, label: e.edge_type !== 'hierarchy' ? e.label : undefined, edge_type: e.edge_type, ...style };
});

const container = document.getElementById('graph');
const nodeDS = new vis.DataSet(visNodes);
const edgeDS = new vis.DataSet(visEdges);

const network = new vis.Network(container, { nodes: nodeDS, edges: edgeDS }, {
  layout: { improvedLayout: true },
  physics: {
    enabled: true,
    solver: 'forceAtlas2Based',
    forceAtlas2Based: { gravitationalConstant: -80, centralGravity: 0.01, springLength: 120 },
    stabilization: { iterations: 200 },
  },
  interaction: { hover: true, tooltipDelay: 100 },
});

const hostCount = DATA.nodes.filter(n => n.kind === 'host').length;
const bfCount = DATA.nodes.filter(n => n.kind === 'business_function').length;
const epCount = DATA.nodes.filter(n => n.kind === 'endpoint').length;
document.getElementById('stats').innerHTML =
  hostCount + ' hosts · ' + bfCount + ' 业务功能 · ' + epCount + ' endpoints';

window.toggleEndpoints = function() {
  const show = document.getElementById('showEndpoints').checked;
  const updates = DATA.nodes.filter(n => n.kind === 'endpoint').map(n => ({ id: n.id, hidden: !show }));
  nodeDS.update(updates);
};
window.toggleDescriptions = function() {
  const show = document.getElementById('showDescriptions').checked;
  const updates = DATA.nodes.filter(n => n.kind === 'business_function').map(n => ({
    id: n.id,
    label: show && n.description ? n.label + '\n' + n.description : n.label,
  }));
  nodeDS.update(updates);
};
window.toggleFlow = function() {
  const show = document.getElementById('showFlow').checked;
  const allEdges = edgeDS.get();
  const updates = allEdges.filter(e => e.edge_type === 'flow').map(e => ({ id: e.id, hidden: !show }));
  edgeDS.update(updates);
};
window.toggleDeps = function() {
  const show = document.getElementById('showDeps').checked;
  const allEdges = edgeDS.get();
  const updates = allEdges.filter(e => e.edge_type === 'dependency').map(e => ({ id: e.id, hidden: !show }));
  edgeDS.update(updates);
};
window.closeDetail = function() {
  document.getElementById('detail-panel').style.display = 'none';
};
network.on('click', function(params) {
  if (params.nodes.length > 0) {
    const nodeId = params.nodes[0];
    const node = DATA.nodes.find(n => n.id === nodeId);
    if (node) showNodeDetail(node);
  } else {
    closeDetail();
  }
});
function showNodeDetail(node) {
  const panel = document.getElementById('detail-panel');
  const title = document.getElementById('detail-title');
  const content = document.getElementById('detail-content');
  title.textContent = node.label;
  let h = '<div style="color:#aaa;margin-bottom:8px;">Type: ' + node.kind + '</div>';
  if (node.description) h += '<div style="margin-bottom:8px;">' + node.description.replace(/\n/g, '<br>') + '</div>';
  if (node.details) {
    const d = node.details;
    h += '<div style="border-top:1px solid #333;padding-top:8px;margin-top:4px;">';
    for (const [k, v] of Object.entries(d)) {
      if (v && v !== '' && v !== 0) h += '<div><span style="color:#888;">' + k + ':</span> ' + (typeof v === 'object' ? JSON.stringify(v) : v) + '</div>';
    }
    h += '</div>';
  }
  const outE = DATA.edges.filter(e => e.from === node.id);
  const inE = DATA.edges.filter(e => e.to === node.id);
  if (outE.length + inE.length > 0) {
    h += '<div style="border-top:1px solid #333;padding-top:8px;margin-top:8px;">';
    h += '<div style="color:#888;margin-bottom:4px;">Connections:</div>';
    outE.forEach(e => { const t = DATA.nodes.find(n => n.id === e.to); h += '<div style="color:#4A90D9;">\u2192 ' + e.edge_type + ': ' + (t ? t.label : e.to) + '</div>'; });
    inE.forEach(e => { const s = DATA.nodes.find(n => n.id === e.from); h += '<div style="color:#E74C3C;">\u2190 ' + e.edge_type + ': ' + (s ? s.label : e.from) + '</div>'; });
    h += '</div>';
  }
  content.innerHTML = h;
  panel.style.display = 'block';
}
</script>
</body>
</html>"#);
    html
}

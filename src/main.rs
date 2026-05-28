use std::collections::BTreeMap;

use bizgraph;
use clap::{Parser, Subcommand};
use serde_json;

#[derive(Parser)]
#[command(
    name = "bizgraph",
    about = "Yakit traffic → Business graph mapper",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze Yakit Excel traffic and produce a business graph
    Analyze {
        /// Path to Yakit Excel file (.xlsx)
        yakit_excel: String,

        /// Project name — if set, persist to DB and show summary
        #[arg(long = "project", short = 'p')]
        project: Option<String>,

        /// Filter traffic by Host column (prefix match)
        #[arg(long = "host", short = 'H')]
        host: Option<String>,

        /// Enable AI business analysis (saved to project DB)
        #[arg(long)]
        ai: bool,
    },
    /// Manage persisted graph projects
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Create a new project
    New { name: String },
    /// List projects
    List,
    /// Show graph stats for a project
    Show { name: String },
    /// Show analysis history for a project
    History { name: String },
    /// Export the current graph state for a project
    Export {
        name: String,
        #[arg(short = 'o')]
        output: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze {
            yakit_excel,
            host,
            project,
            ai,
        } => {
            if let Some(project) = project {
                let resolved_ai_config = if ai {
                    match bizgraph::load_config() {
                        Ok(config) => Some(config),
                        Err(error) => {
                            eprintln!("Error: {error}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    None
                };

                match bizgraph::analyze_with_project(
                    &yakit_excel,
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
                        print_ai_preview(result.ai_report.as_deref());
                    }
                    Err(error) => {
                        eprintln!("Error: {error}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            let graph_result = if ai {
                match bizgraph::load_config() {
                    Ok((resolved_api_key, resolved_model, resolved_api_url)) => {
                        bizgraph::analyze_with_ai_report(
                            &yakit_excel,
                            host.as_deref(),
                            &resolved_api_key,
                            &resolved_model,
                            &resolved_api_url,
                            true,
                        )
                        .await
                        .map(|(graph, report)| (graph, Some(report)))
                    }
                    Err(e) => Err(e),
                }
            } else {
                bizgraph::analyze(&yakit_excel, host.as_deref()).map(|graph| (graph, None))
            };

            match graph_result {
                Ok(graph) => {
                    let (graph, ai_report) = graph;
                    println!("{}", serde_json::to_string(&graph).unwrap());
                    print_ai_report(ai_report.as_deref());
                }
                Err(e) => {
                    eprintln!("Error: {e}");
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

                    println!("{:<10} {:<26} {}", "ID", "CREATED", "NAME");
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
                            "{}\trows={}\t+nodes={}\t~nodes={}\t+edges={}\tskipped_edges={}\thost={}\texcel={}",
                            record.created_at.to_rfc3339(),
                            record.row_count,
                            record.new_nodes,
                            record.updated_nodes,
                            record.new_edges,
                            record.skipped_edges,
                            record.host_filter.as_deref().unwrap_or("-"),
                            record.excel_path.as_deref().unwrap_or("-"),
                        );
                    }
                    Ok(())
                }),
                ProjectAction::Export { name, output } => resolve_project(&db, &name).and_then(|project| {
                    let graph = db.get_graph(project.id)?;
                    let json = serde_json::to_string_pretty(&graph)
                        .map_err(|e| format!("failed to serialize project graph: {e}"))?;
                    if let Some(path) = output {
                        std::fs::write(&path, json)
                            .map_err(|e| format!("failed to write export '{}': {e}", path))?;
                        eprintln!("Exported '{}' to {}", project.name, path);
                    } else {
                        println!("{json}");
                    }
                    Ok(())
                }),
            };

            if let Err(error) = outcome {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
    }
}

fn resolve_project(
    db: &bizgraph::Database,
    name_or_id: &str,
) -> Result<bizgraph::types::Project, String> {
    db.resolve_project(name_or_id)?
        .ok_or_else(|| format!("project '{}' not found", name_or_id))
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

fn print_ai_report(report: Option<&str>) {
    if let Some(report) = report {
        println!("\n---\n\n{report}");
    }
}

fn print_ai_preview(report: Option<&str>) {
    if let Some(report) = report {
        let preview: String = report.chars().take(500).collect();
        println!("\nAI Report Preview:\n{preview}\n...");
    }
}

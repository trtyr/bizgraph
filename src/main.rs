use clap::Parser;
use bizgraph;
use serde_json;

#[derive(Parser)]
#[command(name = "bizgraph", about = "Yakit traffic → Business graph mapper", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Analyze Yakit Excel traffic and produce a business graph
    Analyze {
        /// Path to Yakit Excel file (.xlsx)
        #[arg(long = "yakit-excel", short = 'f')]
        yakit_excel: String,

        /// Filter traffic by Host column value (prefix match)
        #[arg(long = "host", short = 'H')]
        host: Option<String>,

        /// Output path for JSON graph (stdout if not set)
        #[arg(long = "output", short = 'o')]
        output: Option<String>,

        /// Preview only — print summary, don't output full graph
        #[arg(long)]
        summary: bool,

        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,

        /// Enable AI analysis using DeepSeek API
        #[arg(long)]
        ai: bool,

        /// DeepSeek API key (or set in ~/.config/bizgraph/config.toml)
        #[arg(long = "api-key")]
        api_key: Option<String>,

        /// Save AI report to file
        #[arg(long = "ai-output")]
        ai_output: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze { yakit_excel, host, output, summary, pretty, ai, api_key, ai_output } => {
            let graph_result = if ai {
                match bizgraph::load_api_key(api_key.as_deref()) {
                    Ok(resolved_api_key) => bizgraph::analyze_with_ai_report(
                        &yakit_excel,
                        host.as_deref(),
                        &resolved_api_key,
                    )
                    .await
                    .map(|(graph, report)| (graph, Some(report))),
                    Err(e) => Err(e),
                }
            } else {
                bizgraph::analyze(&yakit_excel, host.as_deref()).map(|graph| (graph, None))
            };

            match graph_result {
                Ok(graph) => {
                    let (graph, ai_report) = graph;
                    if summary {
                        let counts: std::collections::HashMap<_, _> = graph.nodes.iter()
                            .fold(std::collections::HashMap::new(), |mut acc, n| {
                                *acc.entry(&n.kind).or_insert(0) += 1;
                                acc
                            });
                        println!("Nodes: {} total", graph.nodes.len());
                        for (kind, count) in &counts {
                            println!("  {:?}: {}", kind, count);
                        }
                        println!("Edges: {} total", graph.edges.len());
                        if let Some(report) = ai_report {
                            if let Some(path) = ai_output {
                                std::fs::write(&path, report).expect("Failed to write AI output");
                                eprintln!("AI report written to {path}");
                            } else {
                                println!("\n---\n\n{report}");
                            }
                        }
                        return;
                    }
                    let json = if pretty {
                        serde_json::to_string_pretty(&graph).unwrap()
                    } else {
                        serde_json::to_string(&graph).unwrap()
                    };
                    if let Some(path) = output {
                        std::fs::write(&path, json).expect("Failed to write output");
                        eprintln!("Written to {path}");
                    } else {
                        println!("{json}");
                    }

                    if let Some(report) = ai_report {
                        if let Some(path) = ai_output {
                            std::fs::write(&path, report).expect("Failed to write AI output");
                            eprintln!("AI report written to {path}");
                        } else {
                            println!("\n---\n\n{report}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

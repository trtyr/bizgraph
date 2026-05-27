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
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze { yakit_excel, host, output, summary, pretty } => {
            match bizgraph::analyze(&yakit_excel, host.as_deref()) {
                Ok(graph) => {
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
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

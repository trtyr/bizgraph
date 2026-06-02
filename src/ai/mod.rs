mod agent;
mod business;
mod chat;
mod prompts;
mod summarization;

pub use business::{identify_business_functions, BusinessFunctionGroup, BusinessIdentification};
use crate::types::BusinessGraph;
use crate::Result;

pub use prompts::*;

/// Send the business graph to the configured chat completion API for AI analysis.
/// Returns a Markdown report.
pub async fn analyze_with_ai(
    graph: &BusinessGraph,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String> {
    let summary = summarization::build_graph_summary(graph);

    let messages = vec![
        chat::ChatMessage::system(prompts::SYSTEM_PROMPT),
        chat::ChatMessage::user(format!(
            "Analyze the following structured summary of a business graph extracted from network traffic.\n\
             The summary contains BusinessFunction nodes (grouped by host+path prefix) and \
             Endpoint nodes (HTTP endpoints with methods, status codes, and schemas).\n\
             \n\
             Describe:\n\
             1. What business functions does this application serve?\n\
             2. What's the user flow / navigation pattern?\n\
             3. How are data and operations organized across business functions?\n\
             4. What is the purpose of each endpoint?\n\
             \n\
             Respond in Markdown.\n\
             \n{summary}"
        )),
    ];

    chat::chat_fresh(messages, api_key, model, api_url).await
}

/// Deep multi-phase AI analysis. If `business_context` is provided, the agent
/// uses it to pre-populate domain knowledge instead of re-discovering business functions.
pub async fn analyze_with_ai_deep(
    graph: &BusinessGraph,
    api_key: &str,
    model: &str,
    api_url: &str,
    business_context: Option<&str>,
) -> Result<String> {
    agent::run_agent(graph, api_key, model, api_url, business_context).await
}

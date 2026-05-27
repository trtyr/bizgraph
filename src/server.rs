use std::sync::Arc;

use axum::{
    Json, Router,
    response::Html,
    routing::get,
};
use tower_http::{cors::CorsLayer, services::ServeDir};

use crate::{analyze, types::BusinessGraph};

struct AppState {
    graph: BusinessGraph,
}

pub async fn serve_with_graph(
    yakit_excel_path: &str,
    host_filter: Option<&str>,
    port: u16,
) -> Result<(), String> {
    let graph = analyze(yakit_excel_path, host_filter)?;
    let state = Arc::new(AppState { graph });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/graph", get(get_graph))
        .nest_service(
            "/static",
            ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static")),
        )
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    eprintln!("BizGraph server: http://127.0.0.1:{port}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| e.to_string())?;
    axum::serve(listener, app)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>BizGraph</title>
</head>
<body>
  <h1>BizGraph server is running</h1>
  <p>Graph data is available at <code>/api/graph</code>.</p>
  <p>Static frontend assets are not present in this repository yet.</p>
</body>
</html>"#,
    )
}

async fn get_graph(state: axum::extract::State<Arc<AppState>>) -> Json<BusinessGraph> {
    Json(state.graph.clone())
}

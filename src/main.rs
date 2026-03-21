mod db;
mod state;
mod scheduler;
mod tools;
mod daemon;
mod executor;
mod watchers;

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::signal;
use tracing_subscriber;

use db::Database;
use daemon::SimpleHandler;

#[derive(Clone)]
struct AppState {
    handler: Arc<SimpleHandler>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::INFO.into()),
        )
        .init();

    // Create data directory
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let data_dir = std::path::PathBuf::from(format!("{}/.task-trigger", home));
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(data_dir.join("logs"))?;

    // Initialize database
    let db_path = data_dir.join("tasks.db");
    let db = Arc::new(Database::new(db_path)?);

    tracing::info!("task-trigger-mcp v{} starting", env!("CARGO_PKG_VERSION"));

    // Get port from environment or use default
    let port = std::env::var("TASK_TRIGGER_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(7755);

    // Create handler
    let handler = Arc::new(SimpleHandler::new(db, port));

    let state = AppState {
        handler: handler.clone(),
    };

    // Build router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/mcp/tools", get(list_tools))
        .route("/mcp/call", post(call_tool))
        .with_state(state);

    // Create listener
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    let local_addr = listener.local_addr()?;

    tracing::info!("task-trigger-mcp daemon listening on http://{}", local_addr);

    // Run server with graceful shutdown
    let server = axum::serve(listener, app);

    tokio::select! {
        _ = server => {},
        _ = signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
        }
    }

    Ok(())
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn list_tools() -> Json<Value> {
    Json(json!({
        "tools": [
            {
                "name": "task_add",
                "description": "Register a new scheduled task"
            },
            {
                "name": "task_watch",
                "description": "Watch a file or directory for changes"
            },
            {
                "name": "task_list",
                "description": "List all registered tasks"
            },
            {
                "name": "task_watchers",
                "description": "List all active watchers"
            },
            {
                "name": "task_remove",
                "description": "Remove a registered task"
            },
            {
                "name": "task_unwatch",
                "description": "Stop watching a file or directory"
            },
            {
                "name": "task_enable",
                "description": "Enable a disabled task"
            },
            {
                "name": "task_disable",
                "description": "Disable an enabled task"
            },
            {
                "name": "task_status",
                "description": "Get status of all tasks and watchers"
            },
            {
                "name": "task_logs",
                "description": "Get logs for a task"
            }
        ]
    }))
}

#[derive(serde::Deserialize)]
struct CallToolRequest {
    tool: String,
    params: Value,
}

async fn call_tool(
    State(state): State<AppState>,
    Json(payload): Json<CallToolRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match state.handler.call_tool(&payload.tool, payload.params).await {
        Ok(result) => Ok(Json(result)),
        Err(e) => {
            tracing::error!("Tool call failed: {}", e);
            Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "error": e
                })),
            ))
        }
    }
}

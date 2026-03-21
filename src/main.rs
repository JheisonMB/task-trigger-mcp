mod db;
mod state;
mod scheduler;
mod tools;
mod daemon;
mod executor;
mod watchers;

use anyhow::Result;
use rmcp::handler::server::tool::ToolRouter;
use std::sync::Arc;
use tokio::signal;
use tracing_subscriber;

use db::Database;
use daemon::McpHandler;

#[tokio::main]
async fn main() -> Result<()> {
    // Inicializar logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
        )
        .init();

    // Crear directorio de datos
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let data_dir = std::path::PathBuf::from(format!("{}/.task-trigger", home));
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(data_dir.join("logs"))?;

    // Inicializar base de datos
    let db_path = data_dir.join("tasks.db");
    let db = Arc::new(Database::new(db_path)?);

    tracing::info!("task-trigger-mcp v{} starting", env!("CARGO_PKG_VERSION"));

    // Crear handler MCP
    let handler = McpHandler::new(db, 7755);

    // TODO: Implementar servidor HTTP/SSE con rmcp

    tracing::info!("task-trigger-mcp daemon listening on port 7755");
    
    // Esperar a Ctrl+C
    signal::ctrl_c().await?;
    tracing::info!("Shutdown signal received");

    Ok(())
}

//! Error handling for the task-trigger-mcp daemon.
//!
//! Provides a structured error type for future library extraction.
//! Currently the binary uses `anyhow::Result` directly; this module
//! is available for when the handler logic is extracted into a library crate.

use thiserror::Error;

/// Structured error type for handler operations.
///
/// Not currently used by the binary (which uses `anyhow`), but designed
/// for future extraction as a library crate where typed errors matter.
#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum TaskError {
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid cron expression: {0}")]
    InvalidSchedule(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[allow(dead_code)]
impl TaskError {
    pub fn not_found(entity: &str, id: &str) -> Self {
        Self::NotFound(format!("{entity} '{id}' not found"))
    }
}

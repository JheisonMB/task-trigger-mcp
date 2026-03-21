use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tarea programada en el scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    pub schedule_expr: String,
    pub cli: Cli,
    pub model: Option<String>,
    pub working_dir: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_ok: Option<bool>,
    pub log_path: String,
}

/// Watcher de archivos/directorios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watcher {
    pub id: String,
    pub path: String,
    pub events: Vec<WatchEvent>,
    pub prompt: String,
    pub cli: Cli,
    pub model: Option<String>,
    pub debounce_seconds: u64,
    pub recursive: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_triggered_at: Option<DateTime<Utc>>,
    pub trigger_count: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WatchEvent {
    Create,
    Modify,
    Delete,
    Move,
}

impl std::fmt::Display for WatchEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchEvent::Create => write!(f, "create"),
            WatchEvent::Modify => write!(f, "modify"),
            WatchEvent::Delete => write!(f, "delete"),
            WatchEvent::Move => write!(f, "move"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Cli {
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "kiro")]
    Kiro,
}

impl std::fmt::Display for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cli::OpenCode => write!(f, "opencode"),
            Cli::Kiro => write!(f, "kiro"),
        }
    }
}

/// Registro de ejecución de una tarea
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLog {
    pub task_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub trigger_type: TriggerType,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    Scheduled,
    Manual,
    Watch,
}

impl std::fmt::Display for TriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriggerType::Scheduled => write!(f, "scheduled"),
            TriggerType::Manual => write!(f, "manual"),
            TriggerType::Watch => write!(f, "watch"),
        }
    }
}

/// Estado general del daemon
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub uptime_seconds: u64,
    pub port: u16,
    pub active_tasks: usize,
    pub active_watchers: usize,
    pub scheduler_available: bool,
    pub log_directory: String,
}

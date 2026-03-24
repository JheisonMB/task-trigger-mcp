//! Core domain models for the task-trigger-mcp daemon.
//!
//! Defines tasks, watchers, execution logs, and all supporting types.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A scheduled task that runs on a cron schedule.
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

impl Task {
    /// Check if this task has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Utc::now() > exp)
    }
}

/// A file system watcher that triggers tasks on file changes.
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

/// File system event types that watchers can respond to.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WatchEvent {
    Create,
    Modify,
    Delete,
    Move,
}

impl WatchEvent {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "create" => Some(Self::Create),
            "modify" => Some(Self::Modify),
            "delete" => Some(Self::Delete),
            "move" => Some(Self::Move),
            _ => None,
        }
    }
}

impl std::fmt::Display for WatchEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create => write!(f, "create"),
            Self::Modify => write!(f, "modify"),
            Self::Delete => write!(f, "delete"),
            Self::Move => write!(f, "move"),
        }
    }
}

/// Supported CLI tools for task execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Cli {
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "kiro")]
    Kiro,
}

impl Cli {
    /// Parse from string (defaults to `OpenCode` for unknown values).
    pub fn from_str(s: &str) -> Self {
        match s {
            "kiro" => Self::Kiro,
            _ => Self::OpenCode,
        }
    }

    /// Return the string representation used for DB storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Kiro => "kiro",
        }
    }

    /// Return the CLI command name.
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Kiro => "kiro-cli",
        }
    }
}

impl std::fmt::Display for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Record of a single task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLog {
    pub task_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub trigger_type: TriggerType,
}

/// How a task was triggered.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    Scheduled,
    Manual,
    Watch,
}

impl TriggerType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "manual" => Self::Manual,
            "watch" => Self::Watch,
            _ => Self::Scheduled,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
            Self::Watch => "watch",
        }
    }
}

impl std::fmt::Display for TriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}


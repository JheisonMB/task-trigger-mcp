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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // ── Task::is_expired ──────────────────────────────────────────

    #[test]
    fn test_task_not_expired_no_expiry() {
        let task = Task {
            id: "t1".to_string(),
            prompt: "test".to_string(),
            schedule_expr: "* * * * *".to_string(),
            cli: Cli::OpenCode,
            model: None,
            working_dir: None,
            enabled: true,
            created_at: Utc::now(),
            expires_at: None,
            last_run_at: None,
            last_run_ok: None,
            log_path: "/tmp/t.log".to_string(),
        };
        assert!(!task.is_expired());
    }

    #[test]
    fn test_task_not_expired_future() {
        let task = Task {
            id: "t2".to_string(),
            prompt: "test".to_string(),
            schedule_expr: "* * * * *".to_string(),
            cli: Cli::OpenCode,
            model: None,
            working_dir: None,
            enabled: true,
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::hours(1)),
            last_run_at: None,
            last_run_ok: None,
            log_path: "/tmp/t.log".to_string(),
        };
        assert!(!task.is_expired());
    }

    #[test]
    fn test_task_expired_past() {
        let task = Task {
            id: "t3".to_string(),
            prompt: "test".to_string(),
            schedule_expr: "* * * * *".to_string(),
            cli: Cli::OpenCode,
            model: None,
            working_dir: None,
            enabled: true,
            created_at: Utc::now() - Duration::hours(2),
            expires_at: Some(Utc::now() - Duration::hours(1)),
            last_run_at: None,
            last_run_ok: None,
            log_path: "/tmp/t.log".to_string(),
        };
        assert!(task.is_expired());
    }

    // ── WatchEvent ────────────────────────────────────────────────

    #[test]
    fn test_watch_event_from_str() {
        assert_eq!(WatchEvent::from_str("create"), Some(WatchEvent::Create));
        assert_eq!(WatchEvent::from_str("modify"), Some(WatchEvent::Modify));
        assert_eq!(WatchEvent::from_str("delete"), Some(WatchEvent::Delete));
        assert_eq!(WatchEvent::from_str("move"), Some(WatchEvent::Move));
        assert_eq!(WatchEvent::from_str("invalid"), None);
        assert_eq!(WatchEvent::from_str(""), None);
    }

    #[test]
    fn test_watch_event_display() {
        assert_eq!(WatchEvent::Create.to_string(), "create");
        assert_eq!(WatchEvent::Modify.to_string(), "modify");
        assert_eq!(WatchEvent::Delete.to_string(), "delete");
        assert_eq!(WatchEvent::Move.to_string(), "move");
    }

    // ── Cli ───────────────────────────────────────────────────────

    #[test]
    fn test_cli_from_str() {
        assert!(matches!(Cli::from_str("opencode"), Cli::OpenCode));
        assert!(matches!(Cli::from_str("kiro"), Cli::Kiro));
        // Unknown defaults to OpenCode
        assert!(matches!(Cli::from_str("unknown"), Cli::OpenCode));
        assert!(matches!(Cli::from_str(""), Cli::OpenCode));
    }

    #[test]
    fn test_cli_as_str() {
        assert_eq!(Cli::OpenCode.as_str(), "opencode");
        assert_eq!(Cli::Kiro.as_str(), "kiro");
    }

    #[test]
    fn test_cli_command_name() {
        assert_eq!(Cli::OpenCode.command_name(), "opencode");
        assert_eq!(Cli::Kiro.command_name(), "kiro-cli");
    }

    #[test]
    fn test_cli_display() {
        assert_eq!(format!("{}", Cli::OpenCode), "opencode");
        assert_eq!(format!("{}", Cli::Kiro), "kiro");
    }

    // ── TriggerType ───────────────────────────────────────────────

    #[test]
    fn test_trigger_type_from_str() {
        assert!(matches!(
            TriggerType::from_str("scheduled"),
            TriggerType::Scheduled
        ));
        assert!(matches!(
            TriggerType::from_str("manual"),
            TriggerType::Manual
        ));
        assert!(matches!(TriggerType::from_str("watch"), TriggerType::Watch));
        // Unknown defaults to Scheduled
        assert!(matches!(
            TriggerType::from_str("unknown"),
            TriggerType::Scheduled
        ));
    }

    #[test]
    fn test_trigger_type_roundtrip() {
        for tt in [
            TriggerType::Scheduled,
            TriggerType::Manual,
            TriggerType::Watch,
        ] {
            assert!(
                matches!(TriggerType::from_str(tt.as_str()), t if std::mem::discriminant(&t) == std::mem::discriminant(&tt))
            );
        }
    }
}

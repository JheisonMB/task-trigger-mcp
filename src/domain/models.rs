//! Core domain models for the canopy daemon.
//!
//! Defines background_agents, watchers, execution logs, and all supporting types.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A scheduled background_agent that runs on a cron schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundAgent {
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
    /// Timeout in minutes for execution locking (default: 15).
    pub timeout_minutes: u32,
}

impl BackgroundAgent {
    /// Check if this background_agent has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Utc::now() > exp)
    }
}

/// A file system watcher that triggers background_agents on file changes.
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
    /// Timeout in minutes for execution locking (default: 15).
    pub timeout_minutes: u32,
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

    /// Parse a list of event strings into `WatchEvent` values.
    ///
    /// Returns an error if any string is invalid or if the list is empty.
    pub fn parse_list(event_strs: &[String]) -> Result<Vec<WatchEvent>, String> {
        // "all" expands to every event type
        if event_strs.len() == 1 && event_strs[0].eq_ignore_ascii_case("all") {
            return Ok(vec![Self::Create, Self::Modify, Self::Delete, Self::Move]);
        }

        let mut events = Vec::with_capacity(event_strs.len());
        for s in event_strs {
            match WatchEvent::from_str(s) {
                Some(e) => events.push(e),
                None => {
                    return Err(format!(
                        "Invalid event type '{}'. Must be: create, modify, delete, move, or all",
                        s
                    ));
                }
            }
        }
        if events.is_empty() {
            return Err("At least one event type must be specified".to_string());
        }
        Ok(events)
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

/// A CLI platform identifier, backed by the canopy registry.
///
/// Stored as a plain string (e.g. `"opencode"`, `"kiro"`). Adding support for a new CLI
/// only requires updating the `canopy-registry/platforms.json` — no Rust code changes needed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct Cli(pub String);

impl Cli {
    /// Construct from any platform name.
    pub fn new(name: impl Into<String>) -> Self {
        Cli(name.into())
    }

    /// Parse from a DB/JSON string. Accepts any non-empty value; empty strings default to `"opencode"`.
    pub fn from_str(s: &str) -> Self {
        if s.is_empty() {
            Cli("opencode".to_string())
        } else {
            Cli(s.to_string())
        }
    }

    /// Return the platform name used for DB storage and display.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the binary name for this CLI, looked up from the saved registry config.
    /// Falls back to the platform name if no registry entry is found.
    pub fn command_name(&self) -> String {
        let registry = Self::load_registry();
        registry
            .and_then(|r| r.get(self.as_str()).map(|c| c.binary.clone()))
            .unwrap_or_else(|| self.0.clone())
    }

    /// Detect which CLIs are currently available, using the saved registry config.
    /// Returns names of CLIs whose binary was found in PATH during `canopy setup`.
    /// Falls back to an empty list if the config file is absent.
    pub fn detect_available() -> Vec<Cli> {
        let Some(registry) = Self::load_registry() else {
            return Vec::new();
        };
        registry
            .available_clis
            .iter()
            .map(|c| Cli::new(&c.name))
            .collect()
    }

    /// Auto-detect a default CLI. Returns the single available CLI,
    /// or `None` if zero or multiple CLIs are found.
    pub fn detect_default() -> Option<Cli> {
        let available = Self::detect_available();
        if available.len() == 1 {
            Some(available.into_iter().next().unwrap())
        } else {
            None
        }
    }

    /// Resolve CLI from an optional user-provided parameter.
    ///
    /// - `Some(name)` → returns `Cli(name)` for any non-empty string.
    /// - `None` → auto-detects from the saved registry. Fails if zero or multiple CLIs found.
    pub fn resolve(param: Option<&str>) -> Result<Cli, String> {
        match param {
            Some(name) if !name.is_empty() => Ok(Cli::new(name)),
            Some(_) => Err("CLI name must not be empty.".to_string()),
            None => match Cli::detect_default() {
                Some(cli) => {
                    tracing::info!("Auto-detected CLI: {}", cli);
                    Ok(cli)
                }
                None => {
                    let available = Cli::detect_available();
                    if available.is_empty() {
                        Err("No CLI found in the registry. Run 'canopy setup' to detect available CLIs.".to_string())
                    } else {
                        Err(format!(
                            "Multiple CLIs found ({}). Please specify the 'cli' parameter explicitly.",
                            available.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
                        ))
                    }
                }
            },
        }
    }

    /// Get the execution strategy for this CLI.
    ///
    /// Loads the strategy from the saved registry config.
    /// Panics with a clear error if configuration is not found.
    pub fn strategy(&self) -> Box<super::cli_strategy::CliStrategy> {
        let home = dirs::home_dir().expect("Could not determine home directory");
        let config_path = home.join(".canopy/cli_config.json");
        let registry = super::cli_config::CliRegistry::load(&config_path).unwrap_or_else(|| {
            panic!(
                "CLI configuration not found at {}\n\
                     Run 'canopy setup' to configure and generate the CLI config file.",
                config_path.display()
            )
        });

        let cli_config = registry.get(self.as_str()).unwrap_or_else(|| {
            panic!(
                "CLI '{}' not found in configuration at {}\n\
                 Available CLIs: {}\n\
                 Run 'canopy setup' to update the configuration.",
                self.as_str(),
                config_path.display(),
                registry.names().join(", ")
            )
        });

        Box::new(super::cli_strategy::CliStrategy {
            binary: cli_config.binary.clone(),
            headless_mode: cli_config.headless_mode.clone(),
            model_flag: cli_config.model_flag.clone(),
            supports_working_dir: cli_config.supports_working_dir,
            working_dir_flag: cli_config.working_dir_flag.clone(),
            env_vars: cli_config.env_vars.clone(),
        })
    }

    fn load_registry() -> Option<super::cli_config::CliRegistry> {
        let home = dirs::home_dir()?;
        super::cli_config::CliRegistry::load(&home.join(".canopy/cli_config.json"))
    }
}

impl std::fmt::Display for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of an execution run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Pending,
    InProgress,
    Success,
    Error,
    Timeout,
    Missed,
}

impl RunStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "success" => Self::Success,
            "error" => Self::Error,
            "timeout" => Self::Timeout,
            "missed" => Self::Missed,
            _ => Self::Pending,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Success => "success",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::Missed => "missed",
        }
    }

    /// Whether this status represents an active (locked) run.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Record of a single background_agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLog {
    pub id: String,
    pub background_agent_id: String,
    pub status: RunStatus,
    pub trigger_type: TriggerType,
    pub summary: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub timeout_at: Option<DateTime<Utc>>,
}

/// How a background_agent was triggered.
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
#[path = "models_tests.rs"]
mod tests;

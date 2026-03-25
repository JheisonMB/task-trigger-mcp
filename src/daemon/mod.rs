//! MCP Server handler implementing all task-trigger tools.
//!
//! Uses the `rmcp` SDK's `#[tool(tool_box)]` and `#[tool(param)]` / `#[tool(aggr)]`
//! macros for proper MCP protocol compliance.

use std::sync::Arc;

use chrono::Utc;
use rmcp::model::*;
use rmcp::schemars;
use rmcp::tool;
use rmcp::Error as McpError;
use serde::Deserialize;

use crate::db::Database;
use crate::executor::Executor;
use crate::state::{Task, WatchEvent, Watcher};
use crate::watchers::WatcherEngine;

// -- Constants ----------------------------------------------------------------

/// Maximum length for task/watcher IDs.
const MAX_ID_LENGTH: usize = 64;
/// Maximum length for prompt strings.
const MAX_PROMPT_LENGTH: usize = 50_000;
/// Maximum length for path strings.
const MAX_PATH_LENGTH: usize = 4096;

// ── Aggregate parameter types ────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskAddParams {
    /// Unique identifier. Lowercase, hyphens, underscores.
    pub id: String,
    /// The instruction the CLI will execute headlessly.
    pub prompt: String,
    /// Standard 5-field cron expression: minute hour day month weekday. Example: "0 9 * * *" for daily at 9am.
    pub schedule: String,
    /// CLI to use: "opencode" or "kiro".
    pub cli: String,
    /// Optional provider/model string.
    pub model: Option<String>,
    /// Auto-expire after N minutes from registration.
    pub duration_minutes: Option<i64>,
    /// Working directory for the CLI.
    pub working_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskWatchParams {
    /// Unique identifier for the watcher.
    pub id: String,
    /// Absolute path to file or directory to watch.
    pub path: String,
    /// Events to watch: "create", "modify", "delete", "move".
    pub events: Vec<String>,
    /// Instruction for the CLI on trigger.
    pub prompt: String,
    /// CLI to use: "opencode" or "kiro".
    pub cli: String,
    /// Optional provider/model string.
    pub model: Option<String>,
    /// Debounce window in seconds (default: 2).
    pub debounce_seconds: Option<u64>,
    /// Watch subdirectories (default: false).
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskLogsParams {
    /// Task or watcher ID.
    pub id: String,
    /// Last N lines to return (default: 50).
    pub lines: Option<usize>,
    /// ISO 8601 timestamp filter — only return logs after this time.
    pub since: Option<String>,
}

// ── MCP Handler ──────────────────────────────────────────────────────

/// The main MCP server handler for task-trigger-mcp.
#[derive(Clone)]
pub struct TaskTriggerHandler {
    pub db: Arc<Database>,
    pub executor: Arc<Executor>,
    pub watcher_engine: Arc<WatcherEngine>,
    pub start_time: std::time::Instant,
    pub port: u16,
}

#[tool(tool_box)]
impl TaskTriggerHandler {
    pub fn new(
        db: Arc<Database>,
        executor: Arc<Executor>,
        watcher_engine: Arc<WatcherEngine>,
        port: u16,
    ) -> Self {
        Self {
            db,
            executor,
            watcher_engine,
            start_time: std::time::Instant::now(),
            port,
        }
    }

    /// Register a new scheduled task. The daemon's internal scheduler handles execution.
    #[tool(
        name = "task_add",
        description = "Register a new scheduled task. The schedule field must be a standard 5-field cron expression. Common patterns: '*/5 * * * *' (every 5 min), '0 9 * * *' (daily 9am), '0 9 * * 1-5' (weekdays 9am), '0 */2 * * *' (every 2 hours), '30 14 1,15 * *' (1st and 15th at 2:30pm). Fields: minute(0-59) hour(0-23) day(1-31) month(1-12) weekday(0-6, 0=Sun). Use duration_minutes for temporary tasks that auto-expire."
    )]
    async fn task_add(
        &self,
        #[tool(aggr)] params: TaskAddParams,
    ) -> Result<CallToolResult, McpError> {
        use crate::scheduler::validate_cron;
        use crate::state::Cli;

        // Validate inputs
        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_prompt(&params.prompt) {
            return Ok(error_result(&e));
        }

        // Parse CLI type
        let cli = match params.cli.as_str() {
            "opencode" => Cli::OpenCode,
            "kiro" => Cli::Kiro,
            _ => return Ok(error_result("CLI must be 'opencode' or 'kiro'")),
        };

        // Validate cron expression
        let schedule_expr = params.schedule.trim().to_string();
        if !validate_cron(&schedule_expr) {
            return Ok(error_result(&format!(
                "Invalid cron expression '{}'. Must be a 5-field cron expression. Examples: '*/5 * * * *' (every 5 min), '0 9 * * *' (daily 9am), '0 9 * * 1-5' (weekdays 9am).",
                params.schedule
            )));
        }

        // Setup log directory and path
        let log_dir = data_dir()?.join("logs");
        std::fs::create_dir_all(&log_dir)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let log_path = log_dir.join(&params.id).with_extension("log");

        // Calculate expiration
        let expires_at = params
            .duration_minutes
            .map(|mins| Utc::now() + chrono::Duration::minutes(mins));

        let task = Task {
            id: params.id.clone(),
            prompt: params.prompt,
            schedule_expr: schedule_expr.clone(),
            cli,
            model: params.model,
            working_dir: params.working_dir,
            enabled: true,
            created_at: Utc::now(),
            expires_at,
            last_run_at: None,
            last_run_ok: None,
            log_path: log_path.to_string_lossy().to_string(),
        };

        self.db
            .insert_or_update_task(&task)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(success_result(&format!(
            "Task '{}' registered with schedule '{}'{}\nThe daemon's internal scheduler will execute this task automatically.",
            task.id,
            schedule_expr,
            expires_at
                .map(|e| format!(" (expires: {})", e.to_rfc3339()))
                .unwrap_or_default()
        )))
    }

    /// Register a file or directory watcher.
    #[tool(
        name = "task_watch",
        description = "Watch a file or directory for changes and execute a prompt when events occur"
    )]
    async fn task_watch(
        &self,
        #[tool(aggr)] params: TaskWatchParams,
    ) -> Result<CallToolResult, McpError> {
        use crate::state::Cli;

        // Validate inputs
        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_prompt(&params.prompt) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_watch_path(&params.path) {
            return Ok(error_result(&e));
        }

        // Parse CLI
        let cli = match params.cli.as_str() {
            "opencode" => Cli::OpenCode,
            "kiro" => Cli::Kiro,
            _ => return Ok(error_result("CLI must be 'opencode' or 'kiro'")),
        };

        // Parse events
        let mut events = Vec::new();
        for event_str in &params.events {
            match WatchEvent::from_str(event_str) {
                Some(e) => events.push(e),
                None => {
                    return Ok(error_result(&format!(
                        "Invalid event type '{}'. Must be: create, modify, delete, move",
                        event_str
                    )))
                }
            }
        }

        if events.is_empty() {
            return Ok(error_result("At least one event type must be specified"));
        }

        let watcher = Watcher {
            id: params.id.clone(),
            path: params.path.clone(),
            events,
            prompt: params.prompt,
            cli,
            model: params.model,
            debounce_seconds: params.debounce_seconds.unwrap_or(2),
            recursive: params.recursive.unwrap_or(false),
            enabled: true,
            created_at: Utc::now(),
            last_triggered_at: None,
            trigger_count: 0,
        };

        self.db
            .insert_or_update_watcher(&watcher)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Start the actual filesystem watcher
        if let Err(e) = self.watcher_engine.start_watcher(watcher.clone()).await {
            tracing::warn!("Watcher '{}' saved but failed to start: {}", watcher.id, e);
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Watcher '{}' registered but could not start watching '{}': {}. It will be retried on daemon restart.",
                watcher.id, params.path, e
            ))]));
        }

        Ok(success_result(&format!(
            "Watcher '{}' active on '{}' for events: {:?}",
            watcher.id, params.path, params.events
        )))
    }

    /// List all registered scheduled tasks with status.
    #[tool(
        name = "task_list",
        description = "List all registered scheduled tasks with their current status"
    )]
    async fn task_list(&self) -> Result<CallToolResult, McpError> {
        let tasks = self
            .db
            .list_tasks()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if tasks.is_empty() {
            return Ok(success_result("No tasks registered."));
        }

        let mut lines = vec![format!("Found {} task(s):\n", tasks.len())];

        for t in &tasks {
            let prompt_preview = if t.prompt.len() > 80 {
                format!("{}...", &t.prompt[..80])
            } else {
                t.prompt.clone()
            };

            let status = if !t.enabled {
                "disabled"
            } else if t.is_expired() {
                "expired"
            } else {
                "active"
            };

            let mut info = format!(
                "- **{}** [{}]\n  Schedule: `{}`\n  CLI: {}\n  Prompt: {}\n",
                t.id, status, t.schedule_expr, t.cli, prompt_preview
            );

            if let Some(last) = t.last_run_at {
                let ok_str = t
                    .last_run_ok
                    .map(|ok| if ok { "success" } else { "failed" })
                    .unwrap_or("unknown");
                info.push_str(&format!(
                    "  Last run: {} ({})\n",
                    last.to_rfc3339(),
                    ok_str
                ));
            }

            if let Some(exp) = t.expires_at {
                let remaining = exp.signed_duration_since(Utc::now());
                if remaining.num_seconds() > 0 {
                    info.push_str(&format!("  Expires in: {}m\n", remaining.num_minutes()));
                } else {
                    info.push_str("  Status: EXPIRED\n");
                }
            }

            lines.push(info);
        }

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    /// List all active file watchers with status.
    #[tool(
        name = "task_watchers",
        description = "List all registered file watchers with their current status"
    )]
    async fn task_watchers(&self) -> Result<CallToolResult, McpError> {
        let watchers = self
            .db
            .list_watchers()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if watchers.is_empty() {
            return Ok(success_result("No watchers registered."));
        }

        let mut lines = vec![format!("Found {} watcher(s):\n", watchers.len())];

        for w in &watchers {
            let events: Vec<String> = w.events.iter().map(|e| e.to_string()).collect();
            let runtime_active = self.watcher_engine.is_active(&w.id).await;

            let status = if !w.enabled {
                "paused"
            } else if runtime_active {
                "active"
            } else {
                "registered (not running)"
            };

            let mut info = format!(
                "- **{}** [{}]\n  Path: {}\n  Events: {}\n  CLI: {}\n  Debounce: {}s | Recursive: {}\n",
                w.id, status, w.path, events.join(", "), w.cli, w.debounce_seconds, w.recursive
            );

            if let Some(last) = w.last_triggered_at {
                info.push_str(&format!(
                    "  Last triggered: {} (total: {})\n",
                    last.to_rfc3339(),
                    w.trigger_count
                ));
            }

            lines.push(info);
        }

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    /// Remove a task or watcher completely.
    #[tool(
        name = "task_remove",
        description = "Remove a task or watcher completely — deletes from database and stops any active watcher"
    )]
    async fn task_remove(
        &self,
        #[tool(param)]
        #[schemars(description = "Task or watcher ID to remove")]
        id: String,
    ) -> Result<CallToolResult, McpError> {
        // Stop watcher if it exists
        let _ = self.watcher_engine.stop_watcher(&id).await;

        // Delete from database
        self.db
            .delete_task(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Also try to delete as watcher
        let _ = self.db.delete_watcher(&id);

        Ok(success_result(&format!("'{}' removed", id)))
    }

    /// Pause a file watcher without deleting it.
    #[tool(
        name = "task_unwatch",
        description = "Pause a file watcher without deleting its definition — can be resumed later"
    )]
    async fn task_unwatch(
        &self,
        #[tool(param)]
        #[schemars(description = "Watcher ID to pause")]
        id: String,
    ) -> Result<CallToolResult, McpError> {
        // Stop the runtime watcher
        let _ = self.watcher_engine.stop_watcher(&id).await;

        // Disable in DB
        self.db
            .update_watcher_enabled(&id, false)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(success_result(&format!("Watcher '{}' paused", id)))
    }

    /// Enable a disabled task or watcher.
    #[tool(
        name = "task_enable",
        description = "Enable a disabled scheduled task or watcher"
    )]
    async fn task_enable(
        &self,
        #[tool(param)]
        #[schemars(description = "Task or watcher ID to enable")]
        id: String,
    ) -> Result<CallToolResult, McpError> {
        // Enable task if it exists
        let _ = self.db.update_task_enabled(&id, true);

        // Re-enable and restart watcher if it exists
        if let Ok(Some(watcher)) = self.db.get_watcher(&id) {
            self.db
                .update_watcher_enabled(&id, true)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let _ = self.watcher_engine.start_watcher(watcher).await;
        }

        Ok(success_result(&format!("'{}' enabled", id)))
    }

    /// Disable a task without removing it.
    #[tool(
        name = "task_disable",
        description = "Disable a scheduled task or watcher without removing it"
    )]
    async fn task_disable(
        &self,
        #[tool(param)]
        #[schemars(description = "Task or watcher ID to disable")]
        id: String,
    ) -> Result<CallToolResult, McpError> {
        // Disable task if it exists
        let _ = self.db.update_task_enabled(&id, false);

        // Stop watcher if it exists
        if self.db.get_watcher(&id).ok().flatten().is_some() {
            self.db
                .update_watcher_enabled(&id, false)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let _ = self.watcher_engine.stop_watcher(&id).await;
        }

        Ok(success_result(&format!("'{}' disabled", id)))
    }

    /// Execute a task immediately, outside its schedule.
    #[tool(
        name = "task_run",
        description = "Execute a task immediately outside its schedule — useful for testing"
    )]
    async fn task_run(
        &self,
        #[tool(param)]
        #[schemars(description = "Task ID to execute")]
        id: String,
    ) -> Result<CallToolResult, McpError> {
        let task = self
            .db
            .get_task(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .ok_or_else(|| McpError::internal_error(format!("Task '{}' not found", id), None))?;

        match self
            .executor
            .execute_task(&task, crate::state::TriggerType::Manual)
            .await
        {
            Ok(exit_code) => {
                if exit_code == 0 {
                    Ok(success_result(&format!(
                        "Task '{}' executed successfully (exit code: 0)",
                        id
                    )))
                } else {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Task '{}' executed with exit code: {}. Check logs with task_logs.",
                        id, exit_code
                    ))]))
                }
            }
            Err(e) => Ok(error_result(&format!(
                "Failed to execute task '{}': {}",
                id, e
            ))),
        }
    }

    /// Get daemon status and statistics.
    #[tool(
        name = "task_status",
        description = "Get overall daemon health, scheduler state, and statistics"
    )]
    async fn task_status(&self) -> Result<CallToolResult, McpError> {
        let tasks = self
            .db
            .list_tasks()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let watchers = self
            .db
            .list_watchers()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let active_tasks = tasks.iter().filter(|t| t.enabled && !t.is_expired()).count();
        let active_watchers = self.watcher_engine.active_count().await;

        let uptime = self.start_time.elapsed();
        let uptime_str = if uptime.as_secs() > 3600 {
            format!(
                "{}h {}m",
                uptime.as_secs() / 3600,
                (uptime.as_secs() % 3600) / 60
            )
        } else if uptime.as_secs() > 60 {
            format!("{}m {}s", uptime.as_secs() / 60, uptime.as_secs() % 60)
        } else {
            format!("{}s", uptime.as_secs())
        };

        let log_dir = data_dir()
            .map(|d| d.join("logs").to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // Temporal tasks with time remaining
        let temporal: Vec<String> = tasks
            .iter()
            .filter(|t| t.expires_at.is_some() && t.enabled)
            .map(|t| {
                let remaining = t.expires_at.unwrap().signed_duration_since(Utc::now());
                if remaining.num_seconds() > 0 {
                    format!("  - {}: {}m remaining", t.id, remaining.num_minutes())
                } else {
                    format!("  - {}: EXPIRED", t.id)
                }
            })
            .collect();

        let transport = if self.port > 0 { "SSE/HTTP" } else { "stdio" };

        let mut status = format!(
            "task-trigger-mcp v{}\n\
             Uptime: {}\n\
             Transport: {}\n\
             Port: {}\n\
             Scheduler: internal (tokio)\n\
             Active tasks: {} / {}\n\
             Active watchers: {} / {}\n\
             Log directory: {}",
            env!("CARGO_PKG_VERSION"),
            uptime_str,
            transport,
            if self.port > 0 { self.port.to_string() } else { "N/A".to_string() },
            active_tasks,
            tasks.len(),
            active_watchers,
            watchers.len(),
            log_dir,
        );

        if !temporal.is_empty() {
            status.push_str("\n\nTemporal tasks:\n");
            status.push_str(&temporal.join("\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(status)]))
    }

    /// Get log output for a task or watcher.
    #[tool(
        name = "task_logs",
        description = "Get the log output for a task or watcher with optional line and time filters"
    )]
    async fn task_logs(
        &self,
        #[tool(aggr)] params: TaskLogsParams,
    ) -> Result<CallToolResult, McpError> {
        let max_lines = params.lines.unwrap_or(50);

        // Try to find as task first, then as watcher
        let log_path = if let Ok(Some(task)) = self.db.get_task(&params.id) {
            task.log_path
        } else {
            // Watcher logs are at ~/.task-trigger/logs/<id>.log
            let dir = data_dir().map_err(|e| McpError::internal_error(e.to_string(), None))?;
            dir.join("logs")
                .join(&params.id)
                .with_extension("log")
                .to_string_lossy()
                .to_string()
        };

        let path = std::path::Path::new(&log_path);
        if !path.exists() {
            return Ok(success_result(&format!(
                "No logs found for '{}'. The task has not been executed yet.",
                params.id
            )));
        }

        // Read the log file
        let content = std::fs::read_to_string(path)
            .map_err(|e| McpError::internal_error(format!("Failed to read log: {}", e), None))?;

        let mut lines: Vec<&str> = content.lines().collect();

        // Filter by 'since' timestamp if provided
        if let Some(ref since) = params.since {
            if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
                lines.retain(|line| {
                    if line.starts_with("--- [") {
                        if let Some(at_pos) = line.find(" at ") {
                            let rest = &line[at_pos + 4..];
                            if let Some(end) = rest.find(" ---") {
                                if let Ok(dt) =
                                    chrono::DateTime::parse_from_rfc3339(&rest[..end])
                                {
                                    return dt >= since_dt;
                                }
                            }
                        }
                    }
                    true // Keep non-timestamp lines
                });
            }
        }

        // Take last N lines
        let total = lines.len();
        if lines.len() > max_lines {
            lines = lines[lines.len() - max_lines..].to_vec();
        }

        let output = if lines.is_empty() {
            format!(
                "No log entries for '{}' matching the filter.",
                params.id
            )
        } else {
            format!(
                "Logs for '{}' (showing {} of {} lines):\n\n{}",
                params.id,
                lines.len(),
                total,
                lines.join("\n")
            )
        };

        // Also include recent runs from DB
        if let Ok(runs) = self.db.list_runs(&params.id, 5) {
            if !runs.is_empty() {
                let mut run_info = String::from("\n\nRecent executions:\n");
                for r in &runs {
                    run_info.push_str(&format!(
                        "  - {} | {} | exit: {} | {}\n",
                        r.started_at.to_rfc3339(),
                        r.trigger_type,
                        r.exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "running".to_string()),
                        r.finished_at
                            .map(|f| {
                                let dur = f.signed_duration_since(r.started_at);
                                format!("{}s", dur.num_seconds())
                            })
                            .unwrap_or_else(|| "in progress".to_string())
                    ));
                }
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "{}{}",
                    output, run_info
                ))]));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

#[tool(tool_box)]
impl rmcp::ServerHandler for TaskTriggerHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(
                "MCP server for registering, managing, and executing scheduled and event-driven tasks. \
                 Use task_add to create scheduled tasks, task_watch for file watchers, \
                 task_run to test immediately, and task_status for daemon health."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn data_dir() -> Result<std::path::PathBuf, McpError> {
    let home =
        dirs::home_dir().ok_or_else(|| McpError::internal_error("Home directory not found", None))?;
    Ok(home.join(".task-trigger"))
}

fn success_result(message: &str) -> CallToolResult {
    CallToolResult::success(vec![Content::text(message.to_string())])
}

fn error_result(message: &str) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message.to_string())])
}

/// Validate an ID: non-empty, max length, alphanumeric + hyphens + underscores.
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("ID cannot be empty".to_string());
    }
    if id.len() > MAX_ID_LENGTH {
        return Err(format!(
            "ID exceeds maximum length of {MAX_ID_LENGTH} characters"
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "ID must contain only alphanumeric characters, hyphens, and underscores".to_string(),
        );
    }
    Ok(())
}

/// Validate a prompt string: non-empty, max length.
fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Err("Prompt cannot be empty".to_string());
    }
    if prompt.len() > MAX_PROMPT_LENGTH {
        return Err(format!(
            "Prompt exceeds maximum length of {MAX_PROMPT_LENGTH} characters"
        ));
    }
    Ok(())
}

/// Validate a path string: non-empty, max length, absolute.
fn validate_watch_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Path cannot be empty".to_string());
    }
    if path.len() > MAX_PATH_LENGTH {
        return Err(format!(
            "Path exceeds maximum length of {MAX_PATH_LENGTH} characters"
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err("Path must be absolute".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_id ───────────────────────────────────────────────

    #[test]
    fn test_validate_id_valid() {
        assert!(validate_id("my-task").is_ok());
        assert!(validate_id("task_123").is_ok());
        assert!(validate_id("a").is_ok());
        assert!(validate_id("ABC-def_456").is_ok());
    }

    #[test]
    fn test_validate_id_empty() {
        assert!(validate_id("").is_err());
    }

    #[test]
    fn test_validate_id_too_long() {
        let long_id = "a".repeat(MAX_ID_LENGTH + 1);
        assert!(validate_id(&long_id).is_err());
        // Exactly at limit should pass
        let exact_id = "a".repeat(MAX_ID_LENGTH);
        assert!(validate_id(&exact_id).is_ok());
    }

    #[test]
    fn test_validate_id_invalid_chars() {
        assert!(validate_id("has space").is_err());
        assert!(validate_id("has.dot").is_err());
        assert!(validate_id("has/slash").is_err());
        assert!(validate_id("has@at").is_err());
        assert!(validate_id("has\nnewline").is_err());
    }

    // ── validate_prompt ───────────────────────────────────────────

    #[test]
    fn test_validate_prompt_valid() {
        assert!(validate_prompt("Run the tests").is_ok());
        assert!(validate_prompt("a").is_ok());
    }

    #[test]
    fn test_validate_prompt_empty() {
        assert!(validate_prompt("").is_err());
        assert!(validate_prompt("   ").is_err());
        assert!(validate_prompt("\t\n").is_err());
    }

    #[test]
    fn test_validate_prompt_too_long() {
        let long = "x".repeat(MAX_PROMPT_LENGTH + 1);
        assert!(validate_prompt(&long).is_err());
        let exact = "x".repeat(MAX_PROMPT_LENGTH);
        assert!(validate_prompt(&exact).is_ok());
    }

    // ── validate_watch_path ───────────────────────────────────────

    #[test]
    fn test_validate_watch_path_valid() {
        assert!(validate_watch_path("/tmp/project").is_ok());
        assert!(validate_watch_path("/home/user/src").is_ok());
    }

    #[test]
    fn test_validate_watch_path_empty() {
        assert!(validate_watch_path("").is_err());
        assert!(validate_watch_path("   ").is_err());
    }

    #[test]
    fn test_validate_watch_path_relative() {
        assert!(validate_watch_path("relative/path").is_err());
        assert!(validate_watch_path("./here").is_err());
    }

    #[test]
    fn test_validate_watch_path_too_long() {
        let long = format!("/{}", "a".repeat(MAX_PATH_LENGTH));
        assert!(validate_watch_path(&long).is_err());
    }
}

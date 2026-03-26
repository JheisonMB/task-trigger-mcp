//! MCP Server handler implementing all task-trigger tools.
//!
//! Uses the `rmcp` SDK's `#[tool_router]` and `#[tool_handler]` macros
//! with `Parameters<T>` for proper MCP protocol compliance.

use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::schemars;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use serde::Deserialize;

use crate::application::ports::{RunRepository, TaskRepository, WatcherRepository};
use crate::db::Database;
use crate::domain::models::{Task, WatchEvent, Watcher};
use crate::domain::validation::{validate_id, validate_prompt, validate_watch_path};
use crate::executor::Executor;
use crate::watchers::WatcherEngine;

// ── Aggregate parameter types ────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskAddParams {
    /// Unique identifier. Lowercase, hyphens, underscores.
    pub id: String,
    /// The instruction the CLI will execute headlessly.
    pub prompt: String,
    /// Standard 5-field cron expression: minute hour day month weekday. Example: "0 9 * * *" for daily at 9am.
    pub schedule: String,
    /// CLI to use: "opencode" or "kiro". If omitted, auto-detects from PATH.
    pub cli: Option<String>,
    /// Optional provider/model string. If omitted, the CLI uses its own configured default model.
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
    /// CLI to use: "opencode" or "kiro". If omitted, auto-detects from PATH.
    pub cli: Option<String>,
    /// Optional provider/model string. If omitted, the CLI uses its own configured default model.
    pub model: Option<String>,
    /// Debounce window in seconds (default: 2).
    pub debounce_seconds: Option<u64>,
    /// Watch subdirectories (default: false).
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskUpdateParams {
    /// ID of the task or watcher to update.
    pub id: String,
    /// New prompt/instruction (applies to both tasks and watchers).
    pub prompt: Option<String>,
    /// New CLI: "opencode" or "kiro" (applies to both).
    pub cli: Option<String>,
    /// New provider/model string, or null to clear (applies to both).
    pub model: Option<Option<String>>,
    // ── Task-only fields ──
    /// New 5-field cron expression (task only).
    pub schedule: Option<String>,
    /// New working directory, or null to clear (task only).
    pub working_dir: Option<Option<String>>,
    /// New duration in minutes from now, or null to clear expiration (task only).
    pub duration_minutes: Option<Option<i64>>,
    // ── Watcher-only fields ──
    /// New absolute path to watch (watcher only).
    pub path: Option<String>,
    /// New event list: "create", "modify", "delete", "move" (watcher only).
    pub events: Option<Vec<String>>,
    /// New debounce window in seconds (watcher only).
    pub debounce_seconds: Option<u64>,
    /// Watch subdirectories (watcher only).
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdParam {
    /// Task or watcher ID.
    pub id: String,
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
    tool_router: ToolRouter<Self>,
}

#[tool_router]
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
            tool_router: Self::tool_router(),
        }
    }

    /// Register a new scheduled task. The daemon's internal scheduler handles execution.
    #[tool(
        name = "task_add",
        description = "Register a new scheduled task. The schedule field must be a standard 5-field cron expression. Common patterns: '*/5 * * * *' (every 5 min), '0 9 * * *' (daily 9am), '0 9 * * 1-5' (weekdays 9am), '0 */2 * * *' (every 2 hours), '30 14 1,15 * *' (1st and 15th at 2:30pm). Fields: minute(0-59) hour(0-23) day(1-31) month(1-12) weekday(0-6, 0=Sun). Use duration_minutes for temporary tasks that auto-expire. The cli parameter is optional -- if omitted, it auto-detects the available CLI from PATH. The model parameter is optional -- if omitted, the CLI uses its own configured default model."
    )]
    async fn task_add(
        &self,
        Parameters(params): Parameters<TaskAddParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::scheduler::validate_cron;
        use crate::domain::models::Cli;

        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_prompt(&params.prompt) {
            return Ok(error_result(&e));
        }

        let cli = match params.cli.as_deref() {
            Some("opencode") => Cli::OpenCode,
            Some("kiro") => Cli::Kiro,
            Some(other) => return Ok(error_result(&format!(
                "Unknown CLI '{}'. Must be 'opencode' or 'kiro'", other
            ))),
            None => {
                match Cli::detect_default() {
                    Some(cli) => {
                        tracing::info!("Auto-detected CLI: {}", cli);
                        cli
                    }
                    None => {
                        let available = Cli::detect_available();
                        if available.is_empty() {
                            return Ok(error_result(
                                "No supported CLI found in PATH. Install 'opencode' or 'kiro-cli'."
                            ));
                        }
                        return Ok(error_result(&format!(
                            "Multiple CLIs found in PATH ({}). Please specify the 'cli' parameter explicitly.",
                            available.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
                        )));
                    }
                }
            }
        };

        let schedule_expr = params.schedule.trim().to_string();
        if !validate_cron(&schedule_expr) {
            return Ok(error_result(&format!(
                "Invalid cron expression '{}'. Must be a 5-field cron expression. Examples: '*/5 * * * *' (every 5 min), '0 9 * * *' (daily 9am), '0 9 * * 1-5' (weekdays 9am).",
                params.schedule
            )));
        }

        let log_dir = data_dir()?.join("logs");
        std::fs::create_dir_all(&log_dir)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let log_path = log_dir.join(&params.id).with_extension("log");

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
        description = "Watch a file or directory for changes and execute a prompt when events occur. The cli parameter is optional -- if omitted, it auto-detects the available CLI from PATH. The model parameter is optional -- if omitted, the CLI uses its own configured default model."
    )]
    async fn task_watch(
        &self,
        Parameters(params): Parameters<TaskWatchParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::domain::models::Cli;

        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_prompt(&params.prompt) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_watch_path(&params.path) {
            return Ok(error_result(&e));
        }

        let cli = match params.cli.as_deref() {
            Some("opencode") => Cli::OpenCode,
            Some("kiro") => Cli::Kiro,
            Some(other) => return Ok(error_result(&format!(
                "Unknown CLI '{}'. Must be 'opencode' or 'kiro'", other
            ))),
            None => {
                match Cli::detect_default() {
                    Some(cli) => cli,
                    None => {
                        let available = Cli::detect_available();
                        if available.is_empty() {
                            return Ok(error_result(
                                "No supported CLI found in PATH. Install 'opencode' or 'kiro-cli'."
                            ));
                        }
                        return Ok(error_result(&format!(
                            "Multiple CLIs found in PATH ({}). Please specify the 'cli' parameter explicitly.",
                            available.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
                        )));
                    }
                }
            }
        };

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
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let _ = self.watcher_engine.stop_watcher(&id).await;

        self.db
            .delete_task(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
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
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let _ = self.watcher_engine.stop_watcher(&id).await;

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
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let _ = self.db.update_task_enabled(&id, true);

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
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let _ = self.db.update_task_enabled(&id, false);

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
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let task = self
            .db
            .get_task(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .ok_or_else(|| McpError::internal_error(format!("Task '{}' not found", id), None))?;

        match self
            .executor
            .execute_task(&task, crate::domain::models::TriggerType::Manual)
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

        let transport = if self.port > 0 { "Streamable HTTP" } else { "stdio" };

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
        Parameters(params): Parameters<TaskLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_lines = params.lines.unwrap_or(50);

        let log_path = if let Ok(Some(task)) = self.db.get_task(&params.id) {
            task.log_path
        } else {
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

        let content = std::fs::read_to_string(path)
            .map_err(|e| McpError::internal_error(format!("Failed to read log: {}", e), None))?;

        let mut lines: Vec<&str> = content.lines().collect();

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
                    true
                });
            }
        }

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

    /// Update fields of an existing task or watcher without recreating it.
    #[tool(
        name = "task_update",
        description = "Modify an existing scheduled task or file watcher. Only the provided fields are updated — omitted fields remain unchanged. Auto-detects whether the ID belongs to a task or watcher. For tasks: schedule, prompt, cli, model, working_dir, duration_minutes. For watchers: path, events, prompt, cli, model, debounce_seconds, recursive."
    )]
    async fn task_update(
        &self,
        Parameters(params): Parameters<TaskUpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::scheduler::validate_cron;

        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }

        let is_task = self
            .db
            .get_task(&params.id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .is_some();
        let is_watcher = self
            .db
            .get_watcher(&params.id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .is_some();

        if !is_task && !is_watcher {
            return Ok(error_result(&format!(
                "No task or watcher found with ID '{}'",
                params.id
            )));
        }

        // ── Shared validation ────────────────────────────────────
        if let Some(ref prompt) = params.prompt {            if let Err(e) = validate_prompt(prompt) {
                return Ok(error_result(&e));
            }
        }

        let cli_str = if let Some(ref cli) = params.cli {
            match cli.as_str() {
                "opencode" | "kiro" => Some(cli.as_str()),
                _ => return Ok(error_result("CLI must be 'opencode' or 'kiro'")),
            }
        } else {
            None
        };

        // ── Task update path ─────────────────────────────────────
        if is_task {
            let ignored: Vec<&str> = [
                params.path.as_ref().map(|_| "path"),
                params.events.as_ref().map(|_| "events"),
                params.debounce_seconds.map(|_| "debounce_seconds"),
                params.recursive.map(|_| "recursive"),
            ]
            .into_iter()
            .flatten()
            .collect();

            if let Some(ref schedule) = params.schedule {
                let trimmed = schedule.trim();
                if !validate_cron(trimmed) {
                    return Ok(error_result(&format!(
                        "Invalid cron expression '{}'. Must be a 5-field cron expression.",
                        schedule
                    )));
                }
            }

            let expires_at: Option<Option<&str>> = match &params.duration_minutes {
                Some(Some(mins)) => {
                    if *mins <= 0 {
                        return Ok(error_result("duration_minutes must be positive"));
                    }
                    None
                }
                Some(None) => Some(None),
                None => None,
            };

            let expires_at_string: Option<String> = match &params.duration_minutes {
                Some(Some(mins)) => {
                    let ts = Utc::now() + chrono::Duration::minutes(*mins);
                    Some(ts.to_rfc3339())
                }
                _ => None,
            };

            let expires_at_param: Option<Option<&str>> = if expires_at_string.is_some() {
                Some(Some(expires_at_string.as_deref().unwrap()))
            } else {
                expires_at
            };

            let schedule_trimmed = params.schedule.as_deref().map(|s| s.trim());

            let task_fields = crate::application::ports::TaskFieldsUpdate {
                prompt: params.prompt.as_deref(),
                schedule_expr: schedule_trimmed,
                cli: cli_str,
                model: params.model.as_ref().map(|m| m.as_deref()),
                working_dir: params.working_dir.as_ref().map(|w| w.as_deref()),
                expires_at: expires_at_param,
            };

            let updated = self
                .db
                .update_task_fields(&params.id, &task_fields)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            if !updated {
                return Ok(error_result("No fields to update were provided"));
            }

            let mut msg = format!("Task '{}' updated successfully.", params.id);
            if params.schedule.is_some() {
                msg.push_str(" Schedule change will take effect within 30 seconds.");
            }
            if !ignored.is_empty() {
                msg.push_str(&format!(
                    " Note: watcher-only fields ignored: {}",
                    ignored.join(", ")
                ));
            }
            return Ok(success_result(&msg));
        }

        // ── Watcher update path ──────────────────────────────────
        let ignored: Vec<&str> = [
            params.schedule.as_ref().map(|_| "schedule"),
            params.working_dir.as_ref().map(|_| "working_dir"),
            params.duration_minutes.as_ref().map(|_| "duration_minutes"),
        ]
        .into_iter()
        .flatten()
        .collect();

        if let Some(ref path) = params.path {
            if let Err(e) = validate_watch_path(path) {
                return Ok(error_result(&e));
            }
        }

        let events_json: Option<String> = if let Some(ref event_strs) = params.events {
            let mut events = Vec::new();
            for s in event_strs {
                match WatchEvent::from_str(s) {
                    Some(e) => events.push(e),
                    None => {
                        return Ok(error_result(&format!(
                            "Invalid event type '{}'. Must be: create, modify, delete, move",
                            s
                        )));
                    }
                }
            }
            if events.is_empty() {
                return Ok(error_result("At least one event type must be specified"));
            }
            Some(serde_json::to_string(&events).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize events: {}", e), None)
            })?)
        } else {
            None
        };

        let watcher_fields = crate::application::ports::WatcherFieldsUpdate {
            prompt: params.prompt.as_deref(),
            path: params.path.as_deref(),
            events: events_json.as_deref(),
            cli: cli_str,
            model: params.model.as_ref().map(|m| m.as_deref()),
            debounce_seconds: params.debounce_seconds,
            recursive: params.recursive,
        };

        let updated = self
            .db
            .update_watcher_fields(&params.id, &watcher_fields)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if !updated {
            return Ok(error_result("No fields to update were provided"));
        }

        // Restart watcher if structural fields changed
        let needs_restart = params.path.is_some()            || params.events.is_some()
            || params.debounce_seconds.is_some()
            || params.recursive.is_some()
            || params.cli.is_some()
            || params.prompt.is_some()
            || params.model.is_some();

        if needs_restart {
            let _ = self.watcher_engine.stop_watcher(&params.id).await;
            if let Ok(Some(watcher)) = self.db.get_watcher(&params.id) {
                if watcher.enabled {
                    if let Err(e) = self.watcher_engine.start_watcher(watcher).await {
                        return Ok(CallToolResult::success(vec![Content::text(format!(
                            "Watcher '{}' updated but failed to restart: {}. It will be retried on daemon restart.",
                            params.id, e
                        ))]));
                    }
                }
            }
        }

        let mut msg = format!("Watcher '{}' updated successfully.", params.id);
        if needs_restart {
            msg.push_str(" Watcher restarted with new configuration.");
        }
        if !ignored.is_empty() {
            msg.push_str(&format!(
                " Note: task-only fields ignored: {}",
                ignored.join(", ")
            ));
        }
        Ok(success_result(&msg))
    }
}

#[tool_handler]
impl ServerHandler for TaskTriggerHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "MCP server for registering, managing, and executing scheduled and event-driven tasks. \
                 Use task_add to create scheduled tasks, task_watch for file watchers, \
                 task_run to test immediately, and task_status for daemon health."
                    .to_string(),
            )
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

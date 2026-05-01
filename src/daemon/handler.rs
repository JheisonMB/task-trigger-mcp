//! MCP Server handler implementing all canopy tools.
//!
//! Uses the `rmcp` SDK's `#[tool_router]` and `#[tool_handler]` macros
//! with `Parameters<T>` for proper MCP protocol compliance.

use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use tokio::sync::Notify;

use crate::application::notification_service::NotificationService;
use crate::application::ports::{AgentRepository, RunRepository};
use crate::daemon::helpers::{
    data_dir, error_result, filter_log_line, notify_run_result, success_result,
};
use crate::daemon::params::*;
use crate::db::Database;
use crate::domain::models::{Agent, Cli, Trigger, WatchEvent};
use crate::domain::validation::{validate_id, validate_prompt, validate_watch_path};
use crate::executor::Executor;
use crate::watchers::WatcherEngine;

#[derive(Clone)]
pub struct TaskTriggerHandler {
    pub db: Arc<Database>,
    pub executor: Arc<Executor>,
    pub watcher_engine: Arc<WatcherEngine>,
    pub scheduler_notify: Arc<Notify>,
    pub notification_service: Arc<dyn NotificationService>,
    pub start_time: std::time::Instant,
    pub port: u16,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl TaskTriggerHandler {
    pub fn new(
        db: Arc<Database>,
        executor: Arc<Executor>,
        watcher_engine: Arc<WatcherEngine>,
        scheduler_notify: Arc<Notify>,
        notification_service: Arc<dyn NotificationService>,
        port: u16,
    ) -> Self {
        Self {
            db,
            executor,
            watcher_engine,
            scheduler_notify,
            notification_service,
            start_time: std::time::Instant::now(),
            port,
            tool_router: Self::tool_router(),
        }
    }

    /// Create or update an agent. Supports cron triggers (schedule), watch triggers
    /// (path + events), or manual-only agents (no trigger). When updating an existing
    /// agent, only the fields you provide are changed.
    #[tool(
        name = "agent_add",
        description = "Create or update a scheduled background_agent. \
         Use agent_models to see available model options. \
         The schedule field must be a standard 5-field cron expression. \
         Common patterns: '*/5 * * * *' (every 5 min), '0 9 * * *' (daily 9am), \
         '0 9 * * 1-5' (weekdays 9am), '0 */2 * * *' (every 2 hours), \
         '30 14 1,15 * *' (1st and 15th at 2:30pm). \
         Fields: minute(0-59) hour(0-23) day(1-31) month(1-12) weekday(0-6, 0=Sun). \
         Use duration_minutes for temporary agents that auto-expire. \
         The cli parameter is optional — if omitted, auto-detects from registry. \
         The model parameter is optional — if omitted, the CLI uses its configured default."
    )]
    async fn task_add(
        &self,
        Parameters(params): Parameters<TaskAddParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::scheduler::validate_cron;
        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_prompt(&params.prompt) {
            return Ok(error_result(&e));
        }

        let cli = match Cli::resolve(params.cli.as_deref()) {
            Ok(c) => c,
            Err(e) => return Ok(error_result(&e)),
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

        let agent = Agent {
            id: params.id.clone(),
            prompt: params.prompt,
            trigger: Some(Trigger::Cron {
                schedule_expr: schedule_expr.clone(),
            }),
            cli,
            model: params.model,
            working_dir: params.working_dir,
            enabled: true,
            created_at: Utc::now(),
            log_path: log_path.to_string_lossy().to_string(),
            timeout_minutes: params.timeout_minutes.unwrap_or(15),
            expires_at,
            last_run_at: None,
            last_run_ok: None,
            last_triggered_at: None,
            trigger_count: 0,
        };

        self.db
            .upsert_agent(&agent)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        self.scheduler_notify.notify_one();

        Ok(success_result(&format!(
            "Agent '{}' registered with schedule '{}'{}\nThe daemon's internal scheduler will execute this agent automatically.",
            agent.id,
            schedule_expr,
            expires_at
                .map(|e| format!(" (expires: {})", e.to_rfc3339()))
                .unwrap_or_default()
        )))
    }

    /// Register a file or directory watcher.
    #[tool(
        name = "agent_watch",
        description = "Watch a file or directory for changes and execute a prompt when events occur. \
         The cli parameter is optional — if omitted, auto-detects from registry. \
         The model parameter is optional — if omitted, the CLI uses its configured default model."
    )]
    async fn task_watch(
        &self,
        Parameters(params): Parameters<TaskWatchParams>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_prompt(&params.prompt) {
            return Ok(error_result(&e));
        }
        if let Err(e) = validate_watch_path(&params.path) {
            return Ok(error_result(&e));
        }

        let cli = match Cli::resolve(params.cli.as_deref()) {
            Ok(c) => c,
            Err(e) => return Ok(error_result(&e)),
        };

        let events = match WatchEvent::parse_list(&params.events) {
            Ok(e) => e,
            Err(e) => return Ok(error_result(&e)),
        };

        let log_dir = data_dir()?.join("logs");
        std::fs::create_dir_all(&log_dir)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let log_path = log_dir.join(&params.id).with_extension("log");

        let agent = Agent {
            id: params.id.clone(),
            prompt: params.prompt,
            trigger: Some(Trigger::Watch {
                path: params.path.clone(),
                events: events.clone(),
                debounce_seconds: params.debounce_seconds.unwrap_or(2),
                recursive: params.recursive.unwrap_or(false),
            }),
            cli,
            model: params.model,
            working_dir: None,
            enabled: true,
            created_at: Utc::now(),
            log_path: log_path.to_string_lossy().to_string(),
            timeout_minutes: params.timeout_minutes.unwrap_or(15),
            expires_at: None,
            last_run_at: None,
            last_run_ok: None,
            last_triggered_at: None,
            trigger_count: 0,
        };

        self.db
            .upsert_agent(&agent)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if let Err(e) = self.watcher_engine.start_watcher(&agent).await {
            tracing::warn!("Watcher '{}' saved but failed to start: {}", agent.id, e);
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Watcher '{}' registered but could not start watching '{}': {}. It will be retried on daemon restart.",
                agent.id, params.path, e
            ))]));
        }

        Ok(success_result(&format!(
            "Watcher '{}' active on '{}' for events: {:?}",
            agent.id, params.path, params.events
        )))
    }

    /// List all registered agents with status.
    #[tool(
        name = "agent_list",
        description = "List all registered scheduled agents with their current status"
    )]
    async fn task_list(&self) -> Result<CallToolResult, McpError> {
        let agents = self
            .db
            .list_agents()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if agents.is_empty() {
            return Ok(success_result("No agents registered."));
        }

        let mut lines = vec![format!("Found {} agent(s):\n", agents.len())];

        for a in &agents {
            let prompt_preview = if a.prompt.len() > 80 {
                format!("{}...", &a.prompt[..80])
            } else {
                a.prompt.clone()
            };

            let status = if !a.enabled {
                "disabled"
            } else if a.is_expired() {
                "expired"
            } else {
                "active"
            };

            let trigger_label = a.trigger_type_label();
            let trigger_detail = match &a.trigger {
                Some(Trigger::Cron { schedule_expr }) => schedule_expr.clone(),
                Some(Trigger::Watch { path, .. }) => path.clone(),
                None => "manual".to_string(),
            };

            let mut info = format!(
                "- **{}** [{}] ({})\n  Trigger: {} `{}`\n  CLI: {}\n  Prompt: {}\n",
                a.id, status, trigger_label, trigger_label, trigger_detail, a.cli, prompt_preview
            );

            if let Some(last) = a.last_run_at {
                let ok_str = a
                    .last_run_ok
                    .map(|ok| if ok { "success" } else { "failed" })
                    .unwrap_or("unknown");
                info.push_str(&format!("  Last run: {} ({})\n", last.to_rfc3339(), ok_str));
            }

            if let Some(last) = a.last_triggered_at {
                info.push_str(&format!(
                    "  Last triggered: {} (count: {})\n",
                    last.to_rfc3339(),
                    a.trigger_count
                ));
            }

            if let Some(exp) = a.expires_at {
                let remaining = exp.signed_duration_since(Utc::now());
                if remaining.num_seconds() > 0 {
                    info.push_str(&format!("  Expires in: {}m\n", remaining.num_minutes()));
                } else {
                    info.push_str("  Status: EXPIRED\n");
                }
            }

            if a.is_watch() {
                let runtime_active = self.watcher_engine.is_active(&a.id).await;
                let watch_status = if !a.enabled {
                    "paused"
                } else if runtime_active {
                    "active"
                } else {
                    "registered (not running)"
                };
                info.push_str(&format!("  Watch status: {}\n", watch_status));
            }

            lines.push(info);
        }

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    /// Remove an agent completely.
    #[tool(
        name = "agent_remove",
        description = "Remove an agent completely — deletes from database and stops any active watcher"
    )]
    async fn task_remove(
        &self,
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let existing = self
            .db
            .get_agent(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if existing.is_none() {
            return Ok(error_result(&format!("No agent found with ID '{}'", id)));
        }

        let _ = self.watcher_engine.stop_watcher(&id).await;

        self.db
            .delete_agent(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        self.scheduler_notify.notify_one();
        Ok(success_result(&format!("Agent '{}' removed", id)))
    }

    /// Enable a disabled agent.
    #[tool(
        name = "agent_enable",
        description = "Enable a disabled agent — resumes scheduling or file watching"
    )]
    async fn task_enable(
        &self,
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let Some(existing) = self
            .db
            .get_agent(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
        else {
            return Ok(error_result(&format!("No agent found with ID '{}'", id)));
        };

        if existing.is_expired() {
            let mut updated = existing.clone();
            updated.expires_at = None;
            self.db
                .upsert_agent(&updated)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        self.db
            .update_agent_enabled(&id, true)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        self.scheduler_notify.notify_one();

        if existing.is_watch() {
            let _ = self.watcher_engine.start_watcher(&existing).await;
        }

        Ok(success_result(&format!("Agent '{}' enabled", id)))
    }

    /// Disable an agent without removing it.
    #[tool(
        name = "agent_disable",
        description = "Disable an agent without removing it — pauses scheduling or file watching"
    )]
    async fn task_disable(
        &self,
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let Some(existing) = self
            .db
            .get_agent(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
        else {
            return Ok(error_result(&format!("No agent found with ID '{}'", id)));
        };

        self.db
            .update_agent_enabled(&id, false)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if existing.is_watch() {
            let _ = self.watcher_engine.stop_watcher(&id).await;
        }

        self.scheduler_notify.notify_one();
        Ok(success_result(&format!("Agent '{}' disabled", id)))
    }

    /// Execute an agent immediately, outside its schedule.
    #[tool(
        name = "agent_run",
        description = "Execute an agent immediately outside its schedule — useful for testing"
    )]
    async fn agent_run(
        &self,
        Parameters(IdParam { id }): Parameters<IdParam>,
    ) -> Result<CallToolResult, McpError> {
        let existing = self
            .db
            .get_agent(&id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let Some(agent) = existing else {
            return Ok(error_result(&format!("No agent found with ID '{}'", id)));
        };

        let executor = Arc::clone(&self.executor);
        let notification_service = self.notification_service.clone();
        let agent_id = id.clone();

        tokio::spawn(async move {
            let result = executor.execute_agent(&agent, true).await;
            notify_run_result(
                &notification_service,
                &agent_id,
                result,
                "Manual run failed",
            );
        });

        Ok(success_result(&format!(
            "Agent '{}' launched in background. Use agent_logs to check progress.",
            id
        )))
    }

    /// Get daemon status and statistics.
    #[tool(
        name = "agent_status",
        description = "Get overall daemon health, scheduler state, and statistics"
    )]
    async fn task_status(&self) -> Result<CallToolResult, McpError> {
        let agents = self
            .db
            .list_agents()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let active_agents = agents
            .iter()
            .filter(|a| a.enabled && !a.is_expired())
            .count();
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

        let cron_count = agents.iter().filter(|a| a.is_cron()).count();
        let watch_count = agents.iter().filter(|a| a.is_watch()).count();
        let manual_count = agents.len() - cron_count - watch_count;

        let temporal: Vec<String> = agents
            .iter()
            .filter(|a| a.expires_at.is_some() && a.enabled)
            .map(|a| {
                let remaining = a.expires_at.unwrap().signed_duration_since(Utc::now());
                if remaining.num_seconds() > 0 {
                    format!("  - {}: {}m remaining", a.id, remaining.num_minutes())
                } else {
                    format!("  - {}: EXPIRED", a.id)
                }
            })
            .collect();

        let transport = if self.port > 0 {
            "Streamable HTTP"
        } else {
            "stdio"
        };

        let mut status = format!(
            "canopy v{}\n\
             Uptime: {}\n\
             Transport: {}\n\
             Port: {}\n\
             Scheduler: internal (tokio)\n\
             Active agents: {} / {} (cron: {}, watch: {}, manual: {})\n\
             Active watchers: {}\n\
             Log directory: {}",
            env!("CARGO_PKG_VERSION"),
            uptime_str,
            transport,
            if self.port > 0 {
                self.port.to_string()
            } else {
                "N/A".to_string()
            },
            active_agents,
            agents.len(),
            cron_count,
            watch_count,
            manual_count,
            active_watchers,
            log_dir,
        );

        if !temporal.is_empty() {
            status.push_str("\n\nTemporal agents:\n");
            status.push_str(&temporal.join("\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(status)]))
    }

    /// List available AI models.
    #[tool(
        name = "agent_models",
        description = "List common AI models available for use with agents. Returns provider/model strings that can be passed to the model field of agent_add or agent_watch."
    )]
    async fn task_models(&self) -> Result<CallToolResult, McpError> {
        let models = [
            ("OpenAI", "gpt-4.1"),
            ("OpenAI", "gpt-4o"),
            ("OpenAI", "gpt-4o-mini"),
            ("OpenAI", "o1"),
            ("OpenAI", "o3"),
            ("OpenAI", "o4-mini"),
            ("Anthropic", "claude-sonnet-4-20250514"),
            ("Anthropic", "claude-opus-4-20250514"),
            ("Anthropic", "claude-3-5-sonnet-20241022"),
            ("Anthropic", "claude-3-7-sonnet-20250219"),
            ("Google", "gemini-2.5-pro"),
            ("Google", "gemini-2.5-flash"),
            ("Google", "gemini-2.0-flash"),
            ("Amazon", "nova-pro"),
            ("Amazon", "nova-lite"),
            ("Mistral", "mistral-large-2411"),
            ("Meta", "llama-4-maverick"),
            ("Meta", "llama-4-scout"),
        ];

        let output = models
            .iter()
            .map(|(provider, model)| format!("  {}  ({})", model, provider))
            .collect::<Vec<_>>()
            .join("\n");

        let result = format!(
            "Available models (use the second column value as the model field):\n\
             {}\n\n\
             Note: Model availability depends on the CLI's configured API keys.\n\
             If model is omitted, the CLI uses its own default.",
            output
        );

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    /// Get log output for an agent.
    #[tool(
        name = "agent_logs",
        description = "Get the log output for an agent with optional line and time filters"
    )]
    async fn task_logs(
        &self,
        Parameters(params): Parameters<TaskLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_lines = params.lines.unwrap_or(50);

        let log_path = match self
            .db
            .get_agent(&params.id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
        {
            Some(agent) => agent.log_path,
            None => {
                let dir = data_dir().map_err(|e| McpError::internal_error(e.to_string(), None))?;
                dir.join("logs")
                    .join(&params.id)
                    .with_extension("log")
                    .to_string_lossy()
                    .to_string()
            }
        };

        let path = std::path::Path::new(&log_path);
        if !path.exists() {
            return Ok(success_result(&format!(
                "No logs found for '{}'. The agent has not been executed yet.",
                params.id
            )));
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| McpError::internal_error(format!("Failed to read log: {}", e), None))?;

        let mut lines: Vec<&str> = content.lines().collect();

        if let Some(ref since) = params.since {
            if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
                lines.retain(|line| filter_log_line(line, &since_dt));
            }
        }

        let total = lines.len();
        if lines.len() > max_lines {
            lines = lines[lines.len() - max_lines..].to_vec();
        }

        let output = if lines.is_empty() {
            format!("No log entries for '{}' matching the filter.", params.id)
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
                    let status_str = r.status.as_str();
                    let duration = r
                        .finished_at
                        .map(|f| {
                            let dur = f.signed_duration_since(r.started_at);
                            format!("{}s", dur.num_seconds())
                        })
                        .unwrap_or_else(|| "in progress".to_string());
                    let summary_str = r
                        .summary
                        .as_deref()
                        .map(|s| format!(" — {}", s))
                        .unwrap_or_default();
                    run_info.push_str(&format!(
                        "  - {} | {} | {} | {}{}\n",
                        r.started_at.to_rfc3339(),
                        r.trigger_type,
                        status_str,
                        duration,
                        summary_str,
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

    /// Update fields of an existing agent without recreating it.
    #[tool(
        name = "agent_update",
        description = "Modify an existing agent. Only the provided fields are updated — omitted fields remain unchanged. Auto-detects whether the agent is a cron or watch agent and applies the appropriate fields."
    )]
    async fn task_update(
        &self,
        Parameters(params): Parameters<TaskUpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::scheduler::validate_cron;

        if let Err(e) = validate_id(&params.id) {
            return Ok(error_result(&e));
        }

        let Some(mut agent) = self
            .db
            .get_agent(&params.id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
        else {
            return Ok(error_result(&format!(
                "No agent found with ID '{}'",
                params.id
            )));
        };

        if let Some(ref prompt) = params.prompt {
            if let Err(e) = validate_prompt(prompt) {
                return Ok(error_result(&e));
            }
            agent.prompt = prompt.clone();
        }

        if let Some(ref cli) = params.cli {
            agent.cli = Cli::from_str(cli);
        }

        if let Some(ref model_update) = params.model {
            agent.model = model_update.clone();
        }

        if params.working_dir.is_some() {
            agent.working_dir = params.working_dir.clone().flatten();
        }

        if params.enabled.is_some() {
            agent.enabled = params.enabled.unwrap();
        }

        match &mut agent.trigger {
            Some(Trigger::Cron { schedule_expr }) => {
                if let Some(ref schedule) = params.schedule {
                    let trimmed = schedule.trim();
                    if !validate_cron(trimmed) {
                        return Ok(error_result(&format!(
                            "Invalid cron expression '{}'.",
                            schedule
                        )));
                    }
                    *schedule_expr = trimmed.to_string();
                }

                if let Some(duration) = params.duration_minutes {
                    if duration.is_some() {
                        let mins = duration.unwrap();
                        if mins <= 0 {
                            return Ok(error_result("duration_minutes must be positive"));
                        }
                        agent.expires_at = Some(Utc::now() + chrono::Duration::minutes(mins));
                    } else {
                        agent.expires_at = None;
                    }
                }
            }
            Some(Trigger::Watch {
                path,
                events,
                debounce_seconds,
                recursive,
            }) => {
                if let Some(ref new_path) = params.path {
                    if let Err(e) = validate_watch_path(new_path) {
                        return Ok(error_result(&e));
                    }
                    *path = new_path.clone();
                }

                if let Some(ref event_strs) = params.events {
                    match WatchEvent::parse_list(event_strs) {
                        Ok(parsed) => *events = parsed,
                        Err(e) => return Ok(error_result(&e)),
                    }
                }

                if let Some(ds) = params.debounce_seconds {
                    *debounce_seconds = ds;
                }

                if let Some(r) = params.recursive {
                    *recursive = r;
                }
            }
            None => {
                // Manual-only agent — can upgrade to cron or watch if schedule/path provided
                if params.schedule.is_some() {
                    let schedule = params.schedule.as_ref().unwrap();
                    let trimmed = schedule.trim();
                    if !validate_cron(trimmed) {
                        return Ok(error_result(&format!(
                            "Invalid cron expression '{}'.",
                            schedule
                        )));
                    }
                    agent.trigger = Some(Trigger::Cron {
                        schedule_expr: trimmed.to_string(),
                    });
                } else if params.path.is_some() {
                    let new_path = params.path.as_ref().unwrap();
                    if let Err(e) = validate_watch_path(new_path) {
                        return Ok(error_result(&e));
                    }
                    let new_events = match &params.events {
                        Some(event_strs) => WatchEvent::parse_list(event_strs)
                            .map_err(|e| McpError::internal_error(e, None))?,
                        None => vec![WatchEvent::Create, WatchEvent::Modify],
                    };
                    agent.trigger = Some(Trigger::Watch {
                        path: new_path.clone(),
                        events: new_events,
                        debounce_seconds: params.debounce_seconds.unwrap_or(2),
                        recursive: params.recursive.unwrap_or(false),
                    });
                }
            }
        }

        self.db
            .upsert_agent(&agent)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if agent.is_cron() {
            self.scheduler_notify.notify_one();
        }

        if agent.is_watch() {
            let needs_restart = params.path.is_some()
                || params.events.is_some()
                || params.debounce_seconds.is_some()
                || params.recursive.is_some()
                || params.cli.is_some()
                || params.prompt.is_some()
                || params.model.is_some();

            let mut restarted = false;
            if needs_restart {
                let _ = self.watcher_engine.stop_watcher(&params.id).await;
                if agent.enabled {
                    if let Err(e) = self.watcher_engine.start_watcher(&agent).await {
                        return Ok(CallToolResult::success(vec![Content::text(format!(
                            "Agent '{}' updated but watcher failed to restart: {}. It will be retried on daemon restart.",
                            params.id, e
                        ))]));
                    }
                    restarted = true;
                }
            }

            if restarted {
                return Ok(success_result(&format!(
                    "Agent '{}' updated successfully. Watcher restarted with new configuration.",
                    params.id
                )));
            }
        }

        Ok(success_result(&format!(
            "Agent '{}' updated successfully.",
            params.id
        )))
    }

    /// Report execution status from within a running agent.
    #[tool(
        name = "agent_report",
        description = "Report execution status for a running agent. The run_id is provided in the agent execution prompt. Call with status='in_progress' immediately when starting, then status='success' or status='error' with a summary when finished."
    )]
    async fn task_report(
        &self,
        Parameters(params): Parameters<TaskReportParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::application::ports::RunRepository;
        use crate::domain::models::RunStatus;

        let status = match params.status.as_str() {
            "in_progress" => RunStatus::InProgress,
            "success" => RunStatus::Success,
            "error" => RunStatus::Error,
            _ => {
                return Ok(error_result(
                    "Invalid status. Must be 'in_progress', 'success', or 'error'.",
                ));
            }
        };

        if matches!(status, RunStatus::Success | RunStatus::Error) && params.summary.is_none() {
            return Ok(error_result(
                "A summary is required when reporting 'success' or 'error'.",
            ));
        }

        let run = self
            .db
            .get_run(&params.run_id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::internal_error(format!("Run '{}' not found.", params.run_id), None)
            })?;

        if !run.status.is_active() {
            // not active — skip timeout check
        } else if let Some(timeout_at) = run.timeout_at {
            if chrono::Utc::now() > timeout_at {
                let _ = self.db.update_run_status(
                    &params.run_id,
                    RunStatus::Timeout,
                    Some("Execution timed out"),
                );
                return Ok(error_result(&format!(
                    "Run '{}' has timed out and can no longer be updated.",
                    params.run_id
                )));
            }
        }

        let valid = matches!(
            (&run.status, &status),
            (RunStatus::Pending, RunStatus::InProgress)
                | (RunStatus::InProgress, RunStatus::Success | RunStatus::Error)
                | (RunStatus::Pending, RunStatus::Success | RunStatus::Error)
        );
        if !valid {
            return Ok(error_result(&format!(
                "Invalid transition: {} -> {}",
                run.status, status
            )));
        }

        let updated = self
            .db
            .update_run_status(&params.run_id, status, params.summary.as_deref())
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if !updated {
            return Ok(error_result(&format!(
                "Failed to update run '{}'.",
                params.run_id
            )));
        }

        if matches!(status, RunStatus::Success | RunStatus::Error) {
            let success = status == RunStatus::Success;
            let _ = self
                .db
                .update_agent_last_run(&run.background_agent_id, success);
        }

        Ok(success_result(&format!(
            "Run '{}' status updated to '{}'.",
            params.run_id, status
        )))
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
                "MCP server for registering, managing, and executing scheduled and event-driven agents. \
                 Use agent_add to create scheduled agents, agent_watch for file watchers, \
                 agent_run to test immediately, and agent_status for daemon health."
                    .to_string(),
            )
    }
}

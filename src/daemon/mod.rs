use rmcp::{tool, tool_router, handler::server::tool::ToolRouter, handler::server::wrapper::Parameters, Json};
use serde::Deserialize;
use schemars::JsonSchema;
use std::sync::Arc;
use chrono::Utc;

use crate::db::Database;
use crate::tools::*;
use crate::scheduler::natural_to_cron;

#[derive(Clone)]
pub struct McpHandler {
    pub db: Arc<Database>,
    pub start_time: std::time::Instant,
    pub port: u16,
    pub tool_router: ToolRouter<Self>,
}

// Parámetros vacíos para tools sin params
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EmptyParams {}

#[tool_router]
impl McpHandler {
    pub fn new(db: Arc<Database>, port: u16) -> Self {
        Self {
            db,
            start_time: std::time::Instant::now(),
            port,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(name = "task_add", description = "Register a new scheduled task")]
    async fn task_add(&self, params: Parameters<TaskAddParams>) -> Result<Json<TaskAddResult>, String> {
        let params = params.0;

        if !params.id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err("ID must contain only alphanumeric, hyphens, or underscores".to_string());
        }

        let schedule_expr = if let Some(cron) = natural_to_cron(&params.schedule) {
            cron
        } else {
            params.schedule.clone()
        };

        let log_dir = std::path::PathBuf::from(format!("{}/.task-trigger/logs", dirs_home()));
        std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

        let log_path = log_dir.join(&params.id).with_extension("log");

        let expires_at = params.duration_minutes.map(|mins| {
            Utc::now() + chrono::Duration::minutes(mins)
        });

        let task = crate::state::Task {
            id: params.id.clone(),
            prompt: params.prompt,
            schedule_expr,
            cli: params.cli,
            model: params.model,
            working_dir: params.working_dir,
            enabled: true,
            created_at: Utc::now(),
            expires_at,
            last_run_at: None,
            last_run_ok: None,
            log_path: log_path.to_string_lossy().to_string(),
        };

        self.db.insert_or_update_task(&task)
            .map_err(|e| e.to_string())?;

        Ok(Json(TaskAddResult {
            id: task.id.clone(),
            message: "Task registered successfully".to_string(),
            status: "ok".to_string(),
        }))
    }

    #[tool(name = "task_list", description = "List all registered tasks")]
    async fn task_list(&self, _params: Parameters<EmptyParams>) -> Result<Json<TaskListResult>, String> {
        let tasks = self.db.list_tasks()
            .map_err(|e| e.to_string())?;

        let task_infos: Vec<TaskInfo> = tasks.iter().map(|t| {
            let prompt_preview = if t.prompt.len() > 50 {
                format!("{}...", &t.prompt[..50])
            } else {
                t.prompt.clone()
            };

            TaskInfo {
                id: t.id.clone(),
                prompt_preview,
                schedule: t.schedule_expr.clone(),
                next_run: None,
                last_run: t.last_run_at.map(|dt| dt.to_rfc3339()),
                last_run_ok: t.last_run_ok,
                enabled: t.enabled,
                expires_in: t.expires_at.map(|dt| {
                    let now = Utc::now();
                    let duration = dt.signed_duration_since(now);
                    format!("{}s", duration.num_seconds())
                }),
            }
        }).collect();

        Ok(Json(TaskListResult {
            tasks: task_infos,
        }))
    }

    #[tool(name = "task_status", description = "Get daemon status")]
    async fn task_status(&self, _params: Parameters<EmptyParams>) -> Result<Json<StatusResult>, String> {
        let tasks = self.db.list_tasks()
            .map_err(|e| e.to_string())?;
        let watchers = self.db.list_watchers()
            .map_err(|e| e.to_string())?;

        Ok(Json(StatusResult {
            version: env!("CARGO_PKG_VERSION").to_string(),
            port: self.port,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            active_tasks: tasks.len(),
            active_watchers: watchers.len(),
            scheduler_available: true,
        }))
    }
}

fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

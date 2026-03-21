use crate::db::Database;
use crate::scheduler::natural_to_cron;
use crate::state::{Cli, Task, Watcher};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct SimpleHandler {
    pub db: Arc<Database>,
    pub start_time: std::time::Instant,
    pub port: u16,
}

impl SimpleHandler {
    pub fn new(db: Arc<Database>, port: u16) -> Self {
        Self {
            db,
            start_time: std::time::Instant::now(),
            port,
        }
    }

    /// Procesa una llamada a tool
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value, String> {
        match name {
            "task_add" => self.task_add(params).await,
            "task_watch" => self.task_watch(params).await,
            "task_list" => self.task_list().await,
            "task_watchers" => self.task_watchers().await,
            "task_remove" => self.task_remove(params).await,
            "task_unwatch" => self.task_unwatch(params).await,
            "task_enable" => self.task_enable(params).await,
            "task_disable" => self.task_disable(params).await,
            "task_status" => self.task_status().await,
            "task_logs" => self.task_logs(params).await,
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }

    async fn task_add(&self, params: Value) -> Result<Value, String> {
        let id: String = params["id"].as_str().ok_or("Missing id")?.to_string();
        let prompt: String = params["prompt"].as_str().ok_or("Missing prompt")?.to_string();
        let schedule: String = params["schedule"].as_str().ok_or("Missing schedule")?.to_string();
        let cli_str: String = params["cli"].as_str().ok_or("Missing cli")?.to_string();
        let model = params["model"].as_str().map(|s| s.to_string());
        let duration_minutes = params["duration_minutes"].as_i64();
        let working_dir = params["working_dir"].as_str().map(|s| s.to_string());

        if !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err("ID must be alphanumeric with hyphens/underscores".to_string());
        }

        let cli = match cli_str.as_str() {
            "kiro" => Cli::Kiro,
            "opencode" => Cli::OpenCode,
            _ => return Err("Invalid CLI".to_string()),
        };

        let schedule_expr = natural_to_cron(&schedule).unwrap_or(schedule);

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let log_dir = std::path::PathBuf::from(format!("{}/.task-trigger/logs", home));
        std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

        let log_path = log_dir.join(&id).with_extension("log");

        let expires_at = duration_minutes.map(|mins| Utc::now() + chrono::Duration::minutes(mins));

        let task = Task {
            id: id.clone(),
            prompt,
            schedule_expr,
            cli,
            model,
            working_dir,
            enabled: true,
            created_at: Utc::now(),
            expires_at,
            last_run_at: None,
            last_run_ok: None,
            log_path: log_path.to_string_lossy().to_string(),
        };

        self.db.insert_or_update_task(&task).map_err(|e| e.to_string())?;

        Ok(json!({
            "id": task.id,
            "message": "Task registered successfully",
            "status": "ok"
        }))
    }

    async fn task_watch(&self, params: Value) -> Result<Value, String> {
        let id: String = params["id"].as_str().ok_or("Missing id")?.to_string();
        let path: String = params["path"].as_str().ok_or("Missing path")?.to_string();
        let prompt: String = params["prompt"].as_str().ok_or("Missing prompt")?.to_string();
        let cli_str: String = params["cli"].as_str().ok_or("Missing cli")?.to_string();
        let model = params["model"].as_str().map(|s| s.to_string());
        let debounce_seconds = params["debounce_seconds"].as_u64().unwrap_or(2);
        let recursive = params["recursive"].as_bool().unwrap_or(false);

        let cli = match cli_str.as_str() {
            "kiro" => Cli::Kiro,
            "opencode" => Cli::OpenCode,
            _ => return Err("Invalid CLI".to_string()),
        };

        let events_arr = params["events"].as_array().ok_or("Missing events")?;
        let mut events = Vec::new();
        for event_val in events_arr {
            let event_str = event_val.as_str().ok_or("Invalid event")?;
            let event = match event_str {
                "create" => crate::state::WatchEvent::Create,
                "modify" => crate::state::WatchEvent::Modify,
                "delete" => crate::state::WatchEvent::Delete,
                "move" => crate::state::WatchEvent::Move,
                _ => return Err("Invalid event type".to_string()),
            };
            events.push(event);
        }

        let watcher = Watcher {
            id: id.clone(),
            path,
            events,
            prompt,
            cli,
            model,
            debounce_seconds,
            recursive,
            enabled: true,
            created_at: Utc::now(),
            last_triggered_at: None,
            trigger_count: 0,
        };

        self.db.insert_or_update_watcher(&watcher).map_err(|e| e.to_string())?;

        Ok(json!({
            "id": watcher.id,
            "message": "Watcher registered successfully",
            "status": "ok"
        }))
    }

    async fn task_list(&self) -> Result<Value, String> {
        let tasks = self.db.list_tasks().map_err(|e| e.to_string())?;

        let task_infos: Vec<Value> = tasks
            .iter()
            .map(|t| {
                let prompt_preview = if t.prompt.len() > 50 {
                    format!("{}...", &t.prompt[..50])
                } else {
                    t.prompt.clone()
                };

                json!({
                    "id": t.id,
                    "prompt_preview": prompt_preview,
                    "schedule": t.schedule_expr,
                    "last_run": t.last_run_at.map(|dt| dt.to_rfc3339()),
                    "last_run_ok": t.last_run_ok,
                    "enabled": t.enabled,
                    "expires_in": t.expires_at.map(|dt| {
                        let duration = dt.signed_duration_since(Utc::now());
                        format!("{}s", duration.num_seconds())
                    }),
                })
            })
            .collect();

        Ok(json!({
            "tasks": task_infos,
            "count": task_infos.len()
        }))
    }

    async fn task_watchers(&self) -> Result<Value, String> {
        let watchers = self.db.list_watchers().map_err(|e| e.to_string())?;

        let watcher_infos: Vec<Value> = watchers
            .iter()
            .map(|w| {
                json!({
                    "id": w.id,
                    "path": w.path,
                    "events": w.events.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
                    "cli": w.cli.to_string(),
                    "status": if w.enabled { "active" } else { "paused" },
                    "last_triggered": w.last_triggered_at.map(|dt| dt.to_rfc3339()),
                    "trigger_count": w.trigger_count,
                })
            })
            .collect();

        Ok(json!({
            "watchers": watcher_infos,
            "count": watcher_infos.len()
        }))
    }

    async fn task_remove(&self, params: Value) -> Result<Value, String> {
        let id: String = params["id"].as_str().ok_or("Missing id")?.to_string();
        self.db.delete_task(&id).map_err(|e| e.to_string())?;

        Ok(json!({
            "message": format!("Task {} removed", id),
            "status": "ok"
        }))
    }

    async fn task_unwatch(&self, params: Value) -> Result<Value, String> {
        let id: String = params["id"].as_str().ok_or("Missing id")?.to_string();
        self.db.update_watcher_enabled(&id, false).map_err(|e| e.to_string())?;

        Ok(json!({
            "message": format!("Watcher {} paused", id),
            "status": "ok"
        }))
    }

    async fn task_enable(&self, params: Value) -> Result<Value, String> {
        let id: String = params["id"].as_str().ok_or("Missing id")?.to_string();
        self.db.update_task_enabled(&id, true).map_err(|e| e.to_string())?;

        Ok(json!({
            "message": format!("Task {} enabled", id),
            "status": "ok"
        }))
    }

    async fn task_disable(&self, params: Value) -> Result<Value, String> {
        let id: String = params["id"].as_str().ok_or("Missing id")?.to_string();
        self.db.update_task_enabled(&id, false).map_err(|e| e.to_string())?;

        Ok(json!({
            "message": format!("Task {} disabled", id),
            "status": "ok"
        }))
    }

    async fn task_status(&self) -> Result<Value, String> {
        let tasks = self.db.list_tasks().map_err(|e| e.to_string())?;
        let watchers = self.db.list_watchers().map_err(|e| e.to_string())?;

        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "port": self.port,
            "uptime_seconds": self.start_time.elapsed().as_secs(),
            "active_tasks": tasks.len(),
            "active_watchers": watchers.len(),
            "scheduler_available": true,
        }))
    }

    async fn task_logs(&self, params: Value) -> Result<Value, String> {
        let _id: String = params["id"].as_str().ok_or("Missing id")?.to_string();

        Ok(json!({
            "logs": "Log functionality coming soon",
            "lines_count": 0
        }))
    }
}

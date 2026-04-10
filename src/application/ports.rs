//! Port definitions (traits) for the application layer.
//!
//! These traits define the contracts that infrastructure adapters must fulfill.
//! The application layer programs against these abstractions, not concrete types.

use anyhow::Result;

use crate::domain::models::{RunLog, RunStatus, Task, Watcher};

// ── Partial-update DTOs ──────────────────────────────────────────────

/// Fields to update on a task. Only `Some` values are written.
#[derive(Default)]
pub struct TaskFieldsUpdate<'a> {
    pub prompt: Option<&'a str>,
    pub schedule_expr: Option<&'a str>,
    pub cli: Option<&'a str>,
    pub model: Option<Option<&'a str>>,
    pub working_dir: Option<Option<&'a str>>,
    pub expires_at: Option<Option<&'a str>>,
}

/// Fields to update on a watcher. Only `Some` values are written.
#[derive(Default)]
pub struct WatcherFieldsUpdate<'a> {
    pub prompt: Option<&'a str>,
    pub path: Option<&'a str>,
    pub events: Option<&'a str>,
    pub cli: Option<&'a str>,
    pub model: Option<Option<&'a str>>,
    pub debounce_seconds: Option<u64>,
    pub recursive: Option<bool>,
}

// ── Repository traits ────────────────────────────────────────────────

/// Persistence operations for scheduled tasks.
pub trait TaskRepository {
    fn insert_or_update_task(&self, task: &Task) -> Result<()>;
    fn get_task(&self, id: &str) -> Result<Option<Task>>;
    fn list_tasks(&self) -> Result<Vec<Task>>;
    fn delete_task(&self, id: &str) -> Result<()>;
    fn update_task_enabled(&self, id: &str, enabled: bool) -> Result<()>;
    fn update_task_fields(&self, id: &str, fields: &TaskFieldsUpdate<'_>) -> Result<bool>;
    fn update_task_last_run(&self, id: &str, success: bool) -> Result<()>;
}

/// Persistence operations for file watchers.
pub trait WatcherRepository {
    fn insert_or_update_watcher(&self, watcher: &Watcher) -> Result<()>;
    fn get_watcher(&self, id: &str) -> Result<Option<Watcher>>;
    fn list_watchers(&self) -> Result<Vec<Watcher>>;
    fn list_enabled_watchers(&self) -> Result<Vec<Watcher>>;
    fn delete_watcher(&self, id: &str) -> Result<()>;
    fn update_watcher_enabled(&self, id: &str, enabled: bool) -> Result<()>;
    fn update_watcher_fields(&self, id: &str, fields: &WatcherFieldsUpdate<'_>) -> Result<bool>;
    fn update_watcher_triggered(&self, id: &str) -> Result<()>;
}

/// Persistence operations for execution run logs.
pub trait RunRepository {
    fn insert_run(&self, run: &RunLog) -> Result<()>;
    fn list_runs(&self, task_id: &str, limit: usize) -> Result<Vec<RunLog>>;
    fn list_all_recent_runs(&self, limit: usize) -> Result<Vec<RunLog>>;
    fn get_active_run(&self, task_id: &str) -> Result<Option<RunLog>>;
    fn update_run_status(
        &self,
        run_id: &str,
        status: RunStatus,
        summary: Option<&str>,
    ) -> Result<bool>;
    fn update_run_exit_code(&self, run_id: &str, exit_code: i32) -> Result<bool>;
    fn get_run(&self, run_id: &str) -> Result<Option<RunLog>>;
}

/// Key-value store for daemon state (e.g., PID, version).
pub trait StateRepository {
    fn set_state(&self, key: &str, value: &str) -> Result<()>;
    fn get_state(&self, key: &str) -> Result<Option<String>>;
}

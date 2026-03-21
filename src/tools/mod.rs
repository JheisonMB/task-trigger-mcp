use crate::state::{Cli, WatchEvent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskAddParams {
    pub id: String,
    pub prompt: String,
    pub schedule: String,
    pub cli: Cli,
    pub model: Option<String>,
    pub duration_minutes: Option<i64>,
    pub working_dir: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskWatchParams {
    pub id: String,
    pub path: String,
    pub events: Vec<WatchEvent>,
    pub prompt: String,
    pub cli: Cli,
    pub model: Option<String>,
    pub debounce_seconds: Option<u64>,
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdParam {
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskListParam {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskStatusParam {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskLogsParams {
    pub id: String,
    pub lines: Option<usize>,
    pub since: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskListResult {
    pub tasks: Vec<TaskInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskInfo {
    pub id: String,
    pub prompt_preview: String,
    pub schedule: String,
    pub next_run: Option<String>,
    pub last_run: Option<String>,
    pub last_run_ok: Option<bool>,
    pub enabled: bool,
    pub expires_in: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WatchersListResult {
    pub watchers: Vec<WatcherInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WatcherInfo {
    pub id: String,
    pub path: String,
    pub events: Vec<String>,
    pub cli: String,
    pub status: String,
    pub last_triggered: Option<String>,
    pub trigger_count: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusResult {
    pub version: String,
    pub port: u16,
    pub uptime_seconds: u64,
    pub active_tasks: usize,
    pub active_watchers: usize,
    pub scheduler_available: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LogsResult {
    pub logs: String,
    pub lines_count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskAddResult {
    pub id: String,
    pub message: String,
    pub status: String,
}

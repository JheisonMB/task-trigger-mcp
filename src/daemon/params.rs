use rmcp::schemars;
use serde::Deserialize;

// ── Legacy MCP tool parameter types (used by backward-compatible tools) ──

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskAddParams {
    /// Unique identifier. Lowercase, hyphens, underscores.
    pub id: String,
    /// The instruction the CLI will execute headlessly.
    pub prompt: String,
    /// Standard 5-field cron expression: minute hour day month weekday.
    pub schedule: String,
    /// CLI to use. Auto-detects if omitted.
    pub cli: Option<String>,
    /// Optional provider/model string.
    pub model: Option<String>,
    /// Auto-expire after N minutes from registration.
    pub duration_minutes: Option<i64>,
    /// Working directory for the CLI.
    pub working_dir: Option<String>,
    /// Timeout in minutes for execution locking. Default: 15.
    pub timeout_minutes: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskWatchParams {
    /// Unique identifier for the watcher.
    pub id: String,
    /// Absolute path to file or directory to watch.
    pub path: String,
    /// Events to watch: "create", "modify", "delete", "move", or "all".
    pub events: Vec<String>,
    /// Instruction for the CLI on trigger.
    pub prompt: String,
    /// CLI to use. Auto-detects if omitted.
    pub cli: Option<String>,
    /// Optional provider/model string.
    pub model: Option<String>,
    /// Debounce window in seconds (default: 2).
    pub debounce_seconds: Option<u64>,
    /// Watch subdirectories (default: false).
    pub recursive: Option<bool>,
    /// Timeout in minutes for execution locking. Default: 15.
    pub timeout_minutes: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskUpdateParams {
    /// ID of the agent to update.
    pub id: String,
    /// New prompt/instruction.
    pub prompt: Option<String>,
    /// New CLI platform name.
    pub cli: Option<String>,
    /// New provider/model string, or null to clear.
    pub model: Option<Option<String>>,
    /// New 5-field cron expression (cron agents only).
    pub schedule: Option<String>,
    /// New working directory, or null to clear.
    pub working_dir: Option<Option<String>>,
    /// New duration in minutes from now, or null to clear expiration.
    pub duration_minutes: Option<Option<i64>>,
    /// New absolute path to watch (watch agents only).
    pub path: Option<String>,
    /// New event list (watch agents only).
    pub events: Option<Vec<String>>,
    /// New debounce window in seconds (watch agents only).
    pub debounce_seconds: Option<u64>,
    /// Watch subdirectories (watch agents only).
    pub recursive: Option<bool>,
    /// Enable or disable the agent.
    pub enabled: Option<bool>,
}

// ── Shared parameter types ─────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskLogsParams {
    /// Agent ID.
    pub id: String,
    /// Last N lines to return (default: 50).
    pub lines: Option<usize>,
    /// ISO 8601 timestamp filter — only return logs after this time.
    pub since: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdParam {
    /// Agent ID.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskReportParams {
    /// The run ID (UUID) provided in the agent execution prompt.
    pub run_id: String,
    /// Execution status: `in_progress`, `success`, or `error`.
    pub status: String,
    /// Brief summary of what happened (required for success/error).
    pub summary: Option<String>,
}

// ── Sync tool parameter types ──────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SyncAcquireLockParams {
    /// Workdir this lock applies to.
    pub workdir: String,
    /// Agent ID requesting the lock.
    pub agent_id: String,
    /// Human-readable agent name (e.g. "kiro", "opencode").
    pub agent_name: String,
    /// Lock type: "resource" (path) or "command" (exclusive command).
    pub lock_type: String,
    /// Path or command to lock.
    pub resource: String,
    /// Timeout in seconds (0 = no timeout). Default: 300.
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SyncReleaseParams {
    /// Workdir the lock belongs to.
    pub workdir: String,
    /// Agent ID that holds the lock.
    pub agent_id: String,
    /// Human-readable agent name.
    pub agent_name: String,
    /// Lock ID returned by sync_acquire_lock.
    pub lock_id: String,
    /// Resource that was locked (for the release message).
    pub resource: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SyncBroadcastParams {
    /// Workdir channel to broadcast to.
    pub workdir: String,
    /// Agent ID sending the message.
    pub agent_id: String,
    /// Human-readable agent name.
    pub agent_name: String,
    /// Message kind: intent | info | query | answer | status.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// Optional JSON metadata.
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SyncGetContextParams {
    /// Workdir to query.
    pub workdir: String,
    /// Number of recent messages to return (default: 10).
    pub limit: Option<usize>,
}

// ── RAG tool parameter types ───────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectSearchParams {
    /// Search query matched against project name and description.
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectUpdateParams {
    /// Project hash (workdir_hash).
    pub project_hash: String,
    /// New description.
    pub description: Option<String>,
    /// New tags list.
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RagSearchParams {
    /// Natural-language search query.
    pub query: String,
    /// "global" (all projects) or "project" (single project).
    pub scope: Option<String>,
    /// Required when scope = "project".
    pub project_hash: Option<String>,
    /// Max results (default: 5).
    pub limit: Option<usize>,
}

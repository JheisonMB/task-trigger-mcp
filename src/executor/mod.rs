//! Task executor — spawns CLI subprocesses headlessly.
//!
//! Resolves the CLI binary path via `which`, spawns the process with
//! the appropriate flags, captures output to log files, and records
//! execution in the `runs` table.

use anyhow::Result;
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

use crate::application::ports::{RunRepository, TaskRepository, WatcherRepository};
use crate::db::Database;
use crate::domain::models::{Cli, RunLog, RunStatus, Task, TriggerType, Watcher};
use crate::scheduler::substitute_variables;

/// Maximum log file size before rotation (5 MB).
const MAX_LOG_SIZE: u64 = 5 * 1024 * 1024;

/// Inputs for a single CLI execution. Used by `run_cli_process` to
/// decouple the common spawn-capture-log logic from caller-specific setup.
struct CliRunParams<'a> {
    id: &'a str,
    cli: &'a Cli,
    prompt: String,
    model: Option<&'a str>,
    working_dir: Option<&'a str>,
    log_path: String,
    trigger: TriggerType,
}

/// Result of a CLI execution.
struct CliRunResult {
    exit_code: i32,
    success: bool,
}

/// Task execution engine.
pub struct Executor {
    db: Arc<Database>,
}

impl Executor {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Resolve a timed-out active run by marking it as timeout.
    /// Called lazily before checking the lock.
    fn resolve_timeout(&self, task_id: &str) {
        if let Ok(Some(run)) = self.db.get_active_run(task_id) {
            if let Some(timeout_at) = run.timeout_at {
                if Utc::now() > timeout_at {
                    tracing::info!("Run '{}' for '{}' timed out, unlocking", run.id, task_id);
                    let _ = self.db.update_run_status(
                        &run.id,
                        RunStatus::Timeout,
                        Some("Execution timed out"),
                    );
                    let _ = self.db.update_task_last_run(task_id, false);
                }
            }
        }
    }

    /// Execute a scheduled task.
    ///
    /// When `force` is true (manual runs), expiry and enabled checks are skipped.
    /// Returns the `run_id` if execution started, or None if skipped.
    pub async fn execute_task(
        &self,
        task: &Task,
        trigger: TriggerType,
        force: bool,
    ) -> Result<i32> {
        if !force {
            if task.is_expired() {
                tracing::info!("Task '{}' has expired, disabling", task.id);
                self.db.update_task_enabled(&task.id, false)?;
                return Ok(-1);
            }

            if !task.enabled {
                tracing::info!("Task '{}' is disabled, skipping", task.id);
                return Ok(-1);
            }
        }

        // Check lock: if there's an active run, record as missed
        self.resolve_timeout(&task.id);
        if let Ok(Some(active)) = self.db.get_active_run(&task.id) {
            tracing::info!(
                "Task '{}' is locked (run {}), recording as missed",
                task.id,
                active.id
            );
            let missed = RunLog {
                id: uuid::Uuid::new_v4().to_string(),
                task_id: task.id.clone(),
                status: RunStatus::Missed,
                trigger_type: trigger,
                summary: Some(format!("Skipped: task locked by run {}", active.id)),
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                exit_code: None,
                timeout_at: None,
            };
            let _ = self.db.insert_run(&missed);
            return Ok(-1);
        }

        // Create run and lock the task
        let run_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let timeout_at = now + chrono::Duration::minutes(i64::from(task.timeout_minutes));

        let run = RunLog {
            id: run_id.clone(),
            task_id: task.id.clone(),
            status: RunStatus::Pending,
            trigger_type: trigger,
            summary: None,
            started_at: now,
            finished_at: None,
            exit_code: None,
            timeout_at: Some(timeout_at),
        };
        self.db.insert_run(&run)?;

        let user_prompt = substitute_variables(&task.prompt, &task.id, &task.log_path, None, None);
        let wrapped = wrap_prompt(&user_prompt, &task.id, &run_id);

        let params = CliRunParams {
            id: &task.id,
            cli: &task.cli,
            prompt: wrapped,
            model: task.model.as_deref(),
            working_dir: task.working_dir.as_deref(),
            log_path: task.log_path.clone(),
            trigger,
        };

        let result = self.run_cli_process(&params).await?;

        // If the agent didn't report via task_report, auto-close the run
        // based on the process exit code.
        if let Ok(Some(run)) = self.db.get_run(&run_id) {
            if run.status.is_active() {
                let status = if result.success {
                    RunStatus::Success
                } else {
                    RunStatus::Error
                };
                let _ = self.db.update_run_status(
                    &run_id,
                    status,
                    Some(&format!(
                        "Auto-closed: process exited with code {}",
                        result.exit_code
                    )),
                );
            }
        }
        let _ = self.db.update_run_exit_code(&run_id, result.exit_code);

        if let Err(e) = self.db.update_task_last_run(&task.id, result.success) {
            tracing::error!("Failed to update last_run for task '{}': {}", task.id, e);
        }

        Ok(result.exit_code)
    }

    /// Execute a watcher-triggered task.
    pub async fn execute_watcher_task(
        &self,
        watcher: &Watcher,
        file_path: &str,
        event_type: &str,
    ) -> Result<i32> {
        if !watcher.enabled {
            return Ok(-1);
        }

        // Check lock
        self.resolve_timeout(&watcher.id);
        if let Ok(Some(active)) = self.db.get_active_run(&watcher.id) {
            tracing::info!(
                "Watcher '{}' is locked (run {}), recording as missed",
                watcher.id,
                active.id
            );
            let missed = RunLog {
                id: uuid::Uuid::new_v4().to_string(),
                task_id: watcher.id.clone(),
                status: RunStatus::Missed,
                trigger_type: TriggerType::Watch,
                summary: Some(format!("Skipped: locked by run {}", active.id)),
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                exit_code: None,
                timeout_at: None,
            };
            let _ = self.db.insert_run(&missed);
            return Ok(-1);
        }

        let log_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("No home directory"))?
            .join(".task-trigger/logs");
        let log_path = log_dir
            .join(&watcher.id)
            .with_extension("log")
            .to_string_lossy()
            .to_string();

        // Create run and lock
        let run_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let timeout_at = now + chrono::Duration::minutes(i64::from(watcher.timeout_minutes));

        let run = RunLog {
            id: run_id.clone(),
            task_id: watcher.id.clone(),
            status: RunStatus::Pending,
            trigger_type: TriggerType::Watch,
            summary: None,
            started_at: now,
            finished_at: None,
            exit_code: None,
            timeout_at: Some(timeout_at),
        };
        self.db.insert_run(&run)?;

        let user_prompt = substitute_variables(
            &watcher.prompt,
            &watcher.id,
            &log_path,
            Some(file_path),
            Some(event_type),
        );
        let wrapped = wrap_prompt(&user_prompt, &watcher.id, &run_id);

        let params = CliRunParams {
            id: &watcher.id,
            cli: &watcher.cli,
            prompt: wrapped,
            model: watcher.model.as_deref(),
            working_dir: None,
            log_path,
            trigger: TriggerType::Watch,
        };

        let result = self.run_cli_process(&params).await?;

        if let Ok(Some(run)) = self.db.get_run(&run_id) {
            if run.status.is_active() {
                let status = if result.success {
                    RunStatus::Success
                } else {
                    RunStatus::Error
                };
                let _ = self.db.update_run_status(
                    &run_id,
                    status,
                    Some(&format!(
                        "Auto-closed: process exited with code {}",
                        result.exit_code
                    )),
                );
            }
        }
        let _ = self.db.update_run_exit_code(&run_id, result.exit_code);

        if let Err(e) = self.db.update_watcher_triggered(&watcher.id) {
            tracing::error!(
                "Failed to update trigger count for watcher '{}': {}",
                watcher.id,
                e
            );
        }

        Ok(result.exit_code)
    }

    /// Core CLI execution: resolve binary, build command, spawn, capture output, write log.
    async fn run_cli_process(&self, params: &CliRunParams<'_>) -> Result<CliRunResult> {
        let cli_path = resolve_cli_binary(params.cli)?;
        let mut cmd = build_cli_command(
            &cli_path,
            params.cli,
            &params.prompt,
            params.model,
            params.working_dir,
        );

        tracing::info!(
            "Executing '{}' with {} (trigger: {})",
            params.id,
            params.cli,
            params.trigger,
        );

        let started_at = Utc::now();
        let output = cmd.output().await;

        let (exit_code, success) = match output {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                let success = out.status.success();
                append_to_log(
                    &params.log_path,
                    params.id,
                    &params.trigger,
                    &started_at,
                    code,
                    &out.stdout,
                    &out.stderr,
                )?;
                (code, success)
            }
            Err(e) => {
                tracing::error!("Failed to spawn CLI for '{}': {}", params.id, e);
                append_to_log(
                    &params.log_path,
                    params.id,
                    &params.trigger,
                    &started_at,
                    -1,
                    &[],
                    e.to_string().as_bytes(),
                )?;
                (-1, false)
            }
        };

        Ok(CliRunResult { exit_code, success })
    }
}

/// Resolve the full path to a CLI binary.
fn resolve_cli_binary(cli: &Cli) -> Result<PathBuf> {
    let cmd_name = cli.command_name();
    which::which(cmd_name).map_err(|e| {
        anyhow::anyhow!(
            "CLI binary '{}' not found in PATH: {}. Make sure it is installed.",
            cmd_name,
            e
        )
    })
}

/// Build the CLI command with appropriate flags.
fn build_cli_command(
    cli_path: &Path,
    cli: &Cli,
    prompt: &str,
    model: Option<&str>,
    working_dir: Option<&str>,
) -> Command {
    let mut cmd = Command::new(cli_path);

    match cli {
        Cli::OpenCode => {
            cmd.arg("run").arg(prompt);
            if let Some(m) = model {
                cmd.arg("-m").arg(m);
            }
            if let Some(dir) = working_dir {
                cmd.arg("--dir").arg(dir);
            }
        }
        Cli::Kiro => {
            cmd.arg("chat")
                .arg("--no-interactive")
                .arg("--trust-all-tools")
                .arg(prompt);
            if let Some(m) = model {
                cmd.arg("--model").arg(m);
            }
        }
    }

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    cmd
}

/// Append execution output to a task's log file with rotation.
fn append_to_log(
    log_path: &str,
    task_id: &str,
    trigger: &TriggerType,
    started_at: &chrono::DateTime<Utc>,
    exit_code: i32,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<()> {
    use std::io::Write;

    let path = Path::new(log_path);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    rotate_log_if_needed(path)?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    writeln!(file, "--- [{trigger}] {task_id} at {started_at} ---")?;
    writeln!(file, "exit_code: {exit_code}")?;

    if !stdout.is_empty() {
        writeln!(file, "=== stdout ===")?;
        file.write_all(stdout)?;
        if !stdout.ends_with(b"\n") {
            writeln!(file)?;
        }
    }

    if !stderr.is_empty() {
        writeln!(file, "=== stderr ===")?;
        file.write_all(stderr)?;
        if !stderr.ends_with(b"\n") {
            writeln!(file)?;
        }
    }

    writeln!(file)?;
    Ok(())
}

/// Rotate log file if it exceeds `MAX_LOG_SIZE`.
fn rotate_log_if_needed(path: &Path) -> Result<()> {
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.len() > MAX_LOG_SIZE {
            let rotated = path.with_extension("log.old");
            let _ = std::fs::remove_file(&rotated);
            std::fs::rename(path, &rotated)?;
            tracing::info!("Rotated log file: {}", path.display());
        }
    }
    Ok(())
}

/// Wrap the user's prompt with structured `task_report` instructions.
fn wrap_prompt(user_prompt: &str, task_id: &str, run_id: &str) -> String {
    format!(
        "[SYSTEM INSTRUCTIONS]\n\
         You are executing a managed task. You MUST follow these steps:\n\
         1. IMMEDIATELY call the task_report tool: task_report(run_id=\"{run_id}\", status=\"in_progress\")\n\
         2. Execute the user's task below\n\
         3. When finished, call: task_report(run_id=\"{run_id}\", status=\"success\", summary=\"<brief summary of what happened>\")\n\
            If the task failed: task_report(run_id=\"{run_id}\", status=\"error\", summary=\"<what went wrong>\")\n\
         \n\
         Task ID: {task_id}\n\
         Run ID: {run_id}\n\
         [/SYSTEM INSTRUCTIONS]\n\
         \n\
         [USER TASK]\n\
         {user_prompt}\n\
         [/USER TASK]"
    )
}

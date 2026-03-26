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
use crate::scheduler::substitute_variables;
use crate::domain::models::{Cli, RunLog, Task, TriggerType, Watcher};

/// Maximum log file size before rotation (5 MB).
const MAX_LOG_SIZE: u64 = 5 * 1024 * 1024;

/// Task execution engine.
pub struct Executor {
    db: Arc<Database>,
}

impl Executor {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Execute a scheduled task.
    ///
    /// Checks expiry, resolves CLI binary, spawns subprocess, captures output,
    /// and records the run in the database.
    pub async fn execute_task(&self, task: &Task, trigger: TriggerType) -> Result<i32> {
        if task.is_expired() {
            tracing::info!("Task '{}' has expired, disabling", task.id);
            self.db.update_task_enabled(&task.id, false)?;
            return Ok(-1);
        }

        if !task.enabled {
            tracing::info!("Task '{}' is disabled, skipping", task.id);
            return Ok(-1);
        }

        let started_at = Utc::now();

        let prompt = substitute_variables(
            &task.prompt,
            &task.id,
            &task.log_path,
            None,
            None,
        );

        let cli_path = resolve_cli_binary(&task.cli)?;

        let mut cmd = build_cli_command(
            &cli_path,
            &task.cli,
            &prompt,
            task.model.as_deref(),
            task.working_dir.as_deref(),
        );

        if let Some(ref dir) = task.working_dir {
            cmd.current_dir(dir);
        }

        tracing::info!(
            "Executing task '{}' with {} (trigger: {})",
            task.id,
            task.cli,
            trigger
        );

        let output = cmd.output().await;

        let finished_at = Utc::now();

        let (exit_code, success) = match output {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                let success = out.status.success();

                append_to_log(
                    &task.log_path,
                    &task.id,
                    &trigger,
                    &started_at,
                    code,
                    &out.stdout,
                    &out.stderr,
                )?;

                (code, success)
            }
            Err(e) => {
                tracing::error!("Failed to spawn CLI for task '{}': {}", task.id, e);
                append_to_log(
                    &task.log_path,
                    &task.id,
                    &trigger,
                    &started_at,
                    -1,
                    &[],
                    e.to_string().as_bytes(),
                )?;
                (-1, false)
            }
        };

        let run = RunLog {
            task_id: task.id.clone(),
            started_at,
            finished_at: Some(finished_at),
            exit_code: Some(exit_code),
            trigger_type: trigger,
        };
        if let Err(e) = self.db.insert_run(&run) {
            tracing::error!("Failed to record run for task '{}': {}", task.id, e);
        }

        if let Err(e) = self.db.update_task_last_run(&task.id, success) {
            tracing::error!("Failed to update last_run for task '{}': {}", task.id, e);
        }

        Ok(exit_code)
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

        let started_at = Utc::now();

        let log_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("No home directory"))?
            .join(".task-trigger/logs");
        let log_path = log_dir.join(&watcher.id).with_extension("log");
        let log_path_str = log_path.to_string_lossy().to_string();

        let prompt = substitute_variables(
            &watcher.prompt,
            &watcher.id,
            &log_path_str,
            Some(file_path),
            Some(event_type),
        );

        let cli_path = resolve_cli_binary(&watcher.cli)?;
        let mut cmd = build_cli_command(&cli_path, &watcher.cli, &prompt, watcher.model.as_deref(), None);

        tracing::info!(
            "Executing watcher '{}' (event: {} on {})",
            watcher.id,
            event_type,
            file_path
        );

        let output = cmd.output().await;
        let finished_at = Utc::now();

        let (exit_code, success) = match output {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                let success = out.status.success();
                append_to_log(
                    &log_path_str,
                    &watcher.id,
                    &TriggerType::Watch,
                    &started_at,
                    code,
                    &out.stdout,
                    &out.stderr,
                )?;
                (code, success)
            }
            Err(e) => {
                tracing::error!("Failed to spawn CLI for watcher '{}': {}", watcher.id, e);
                append_to_log(
                    &log_path_str,
                    &watcher.id,
                    &TriggerType::Watch,
                    &started_at,
                    -1,
                    &[],
                    e.to_string().as_bytes(),
                )?;
                (-1, false)
            }
        };

        let run = RunLog {
            task_id: watcher.id.clone(),
            started_at,
            finished_at: Some(finished_at),
            exit_code: Some(exit_code),
            trigger_type: TriggerType::Watch,
        };
        if let Err(e) = self.db.insert_run(&run) {
            tracing::error!("Failed to record run for watcher '{}': {}", watcher.id, e);
        }

        if let Err(e) = self.db.update_watcher_triggered(&watcher.id) {
            tracing::error!("Failed to update trigger count for watcher '{}': {}", watcher.id, e);
        }

        let _ = success;
        Ok(exit_code)
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

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

use crate::state::{Cli, Task, Watcher, WatchEvent, RunLog, TriggerType};

pub struct Database {
    db_path: PathBuf,
}

impl Database {
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let db = Database { db_path };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        let conn = Connection::open(&self.db_path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                schedule_expr TEXT NOT NULL,
                cli TEXT NOT NULL,
                model TEXT,
                working_dir TEXT,
                enabled BOOLEAN NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                expires_at TEXT,
                last_run_at TEXT,
                last_run_ok BOOLEAN,
                log_path TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS watchers (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                events TEXT NOT NULL,
                prompt TEXT NOT NULL,
                cli TEXT NOT NULL,
                model TEXT,
                debounce_seconds INTEGER NOT NULL DEFAULT 2,
                recursive BOOLEAN NOT NULL DEFAULT 0,
                enabled BOOLEAN NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                last_triggered_at TEXT,
                trigger_count INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                exit_code INTEGER,
                trigger_type TEXT NOT NULL,
                FOREIGN KEY(task_id) REFERENCES tasks(id)
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS daemon_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        conn.close().map_err(|_| anyhow::anyhow!("Failed to close db"))?;
        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    pub fn insert_or_update_task(&self, task: &Task) -> Result<()> {
        let conn = self.get_connection()?;
        let cli_str = match task.cli {
            Cli::OpenCode => "opencode",
            Cli::Kiro => "kiro",
        };

        conn.execute(
            "INSERT OR REPLACE INTO tasks 
            (id, prompt, schedule_expr, cli, model, working_dir, enabled, created_at, expires_at, log_path)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &task.id,
                &task.prompt,
                &task.schedule_expr,
                cli_str,
                &task.model,
                &task.working_dir,
                task.enabled,
                task.created_at.to_rfc3339(),
                task.expires_at.map(|t| t.to_rfc3339()),
                &task.log_path,
            ],
        )?;

        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled, created_at, expires_at, last_run_at, last_run_ok, log_path 
             FROM tasks ORDER BY created_at DESC"
        )?;

        let mut result = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<bool>>(10)?,
                row.get::<_, String>(11)?,
            ))
        })?;

        for row_result in rows {
            let (id, prompt, schedule_expr, cli_str, model, working_dir, enabled, created_at_str, expires_at_str, last_run_at_str, last_run_ok, log_path) = row_result?;
            let cli = match cli_str.as_str() {
                "kiro" => Cli::Kiro,
                _ => Cli::OpenCode,
            };

            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)?
                .with_timezone(&Utc);
            let expires_at = expires_at_str
                .as_ref()
                .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
                .transpose()?;
            let last_run_at = last_run_at_str
                .as_ref()
                .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
                .transpose()?;

            result.push(Task {
                id,
                prompt,
                schedule_expr,
                cli,
                model,
                working_dir,
                enabled,
                created_at,
                expires_at,
                last_run_at,
                last_run_ok,
                log_path,
            });
        }

        Ok(result)
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_task_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("UPDATE tasks SET enabled = ?1 WHERE id = ?2", params![enabled, id])?;
        Ok(())
    }

    pub fn insert_or_update_watcher(&self, watcher: &Watcher) -> Result<()> {
        let conn = self.get_connection()?;
        let cli_str = match watcher.cli {
            Cli::OpenCode => "opencode",
            Cli::Kiro => "kiro",
        };

        let events_json = serde_json::to_string(&watcher.events)?;

        conn.execute(
            "INSERT OR REPLACE INTO watchers 
            (id, path, events, prompt, cli, model, debounce_seconds, recursive, enabled, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &watcher.id,
                &watcher.path,
                &events_json,
                &watcher.prompt,
                cli_str,
                &watcher.model,
                watcher.debounce_seconds,
                watcher.recursive,
                watcher.enabled,
                watcher.created_at.to_rfc3339(),
            ],
        )?;

        Ok(())
    }

    pub fn list_watchers(&self) -> Result<Vec<Watcher>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive, enabled, created_at, last_triggered_at, trigger_count 
             FROM watchers ORDER BY created_at DESC"
        )?;

        let mut result = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, u64>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, bool>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, u64>(11)?,
            ))
        })?;

        for row_result in rows {
            let (id, path, events_json, prompt, cli_str, model, debounce_seconds, recursive, enabled, created_at_str, last_triggered_at_str, trigger_count) = row_result?;
            let cli = match cli_str.as_str() {
                "kiro" => Cli::Kiro,
                _ => Cli::OpenCode,
            };

            let events: Vec<WatchEvent> = serde_json::from_str(&events_json)?;
            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)?
                .with_timezone(&Utc);
            let last_triggered_at = last_triggered_at_str
                .as_ref()
                .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
                .transpose()?;

            result.push(Watcher {
                id,
                path,
                events,
                prompt,
                cli,
                model,
                debounce_seconds,
                recursive,
                enabled,
                created_at,
                last_triggered_at,
                trigger_count,
            });
        }

        Ok(result)
    }

    pub fn delete_watcher(&self, id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM watchers WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_watcher_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("UPDATE watchers SET enabled = ?1 WHERE id = ?2", params![enabled, id])?;
        Ok(())
    }

    pub fn update_watcher_triggered(&self, id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE watchers SET last_triggered_at = ?1, trigger_count = trigger_count + 1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn insert_run(&self, run: &RunLog) -> Result<()> {
        let conn = self.get_connection()?;
        let trigger_str = match run.trigger_type {
            TriggerType::Scheduled => "scheduled",
            TriggerType::Manual => "manual",
            TriggerType::Watch => "watch",
        };

        conn.execute(
            "INSERT INTO runs (task_id, started_at, finished_at, exit_code, trigger_type)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &run.task_id,
                run.started_at.to_rfc3339(),
                run.finished_at.map(|t| t.to_rfc3339()),
                run.exit_code,
                trigger_str,
            ],
        )?;

        Ok(())
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO daemon_state (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_state(&self, key: &str) -> Result<Option<String>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare("SELECT value FROM daemon_state WHERE key = ?1")?;
        let value = stmt.query_row(params![key], |row| row.get(0)).optional()?;
        Ok(value)
    }
}

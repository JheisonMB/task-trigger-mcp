use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

use crate::state::{Cli, RunLog, Task, TriggerType, WatchEvent, Watcher};

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

        // Crear tabla de tareas
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

        // Crear tabla de watchers
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

        // Crear tabla de logs de ejecución
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

        // Crear tabla de estado del daemon
        conn.execute(
            "CREATE TABLE IF NOT EXISTS daemon_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        conn.close()
            .map_err(|_| anyhow::anyhow!("Failed to close db"))?;
        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    // === TASKS ===

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

    pub fn get_task(&self, id: &str) -> Result<Option<Task>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled, created_at, expires_at, last_run_at, last_run_ok, log_path 
             FROM tasks WHERE id = ?1"
        )?;

        let task = stmt
            .query_row(params![id], |row| {
                let cli_str: String = row.get(3)?;
                let cli = match cli_str.as_str() {
                    "kiro" => Cli::Kiro,
                    _ => Cli::OpenCode,
                };

                Ok(Task {
                    id: row.get(0)?,
                    prompt: row.get(1)?,
                    schedule_expr: row.get(2)?,
                    cli,
                    model: row.get(4)?,
                    working_dir: row.get(5)?,
                    enabled: row.get(6)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)?
                        .with_timezone(&Utc),
                    expires_at: row
                        .get::<_, Option<String>>(8)?
                        .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
                    last_run_at: row
                        .get::<_, Option<String>>(9)?
                        .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
                    last_run_ok: row.get(10)?,
                    log_path: row.get(11)?,
                })
            })
            .optional()?;

        Ok(task)
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled, created_at, expires_at, last_run_at, last_run_ok, log_path 
             FROM tasks ORDER BY created_at DESC"
        )?;

        let tasks = stmt
            .query_map([], |row| {
                let cli_str: String = row.get(3)?;
                let cli = match cli_str.as_str() {
                    "kiro" => Cli::Kiro,
                    _ => Cli::OpenCode,
                };

                Ok(Task {
                    id: row.get(0)?,
                    prompt: row.get(1)?,
                    schedule_expr: row.get(2)?,
                    cli,
                    model: row.get(4)?,
                    working_dir: row.get(5)?,
                    enabled: row.get(6)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)?
                        .with_timezone(&Utc),
                    expires_at: row
                        .get::<_, Option<String>>(8)?
                        .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
                    last_run_at: row
                        .get::<_, Option<String>>(9)?
                        .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
                    last_run_ok: row.get(10)?,
                    log_path: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_task_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE tasks SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(())
    }

    // === WATCHERS ===

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

    pub fn get_watcher(&self, id: &str) -> Result<Option<Watcher>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive, enabled, created_at, last_triggered_at, trigger_count 
             FROM watchers WHERE id = ?1"
        )?;

        let watcher = stmt
            .query_row(params![id], |row| {
                let cli_str: String = row.get(4)?;
                let cli = match cli_str.as_str() {
                    "kiro" => Cli::Kiro,
                    _ => Cli::OpenCode,
                };

                let events_json: String = row.get(2)?;
                let events: Vec<WatchEvent> = serde_json::from_str(&events_json)?;

                Ok(Watcher {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    events,
                    prompt: row.get(3)?,
                    cli,
                    model: row.get(5)?,
                    debounce_seconds: row.get(6)?,
                    recursive: row.get(7)?,
                    enabled: row.get(8)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(9)?)?
                        .with_timezone(&Utc),
                    last_triggered_at: row
                        .get::<_, Option<String>>(10)?
                        .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
                    trigger_count: row.get(11)?,
                })
            })
            .optional()?;

        Ok(watcher)
    }

    pub fn list_watchers(&self) -> Result<Vec<Watcher>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive, enabled, created_at, last_triggered_at, trigger_count 
             FROM watchers ORDER BY created_at DESC"
        )?;

        let watchers = stmt
            .query_map([], |row| {
                let cli_str: String = row.get(4)?;
                let cli = match cli_str.as_str() {
                    "kiro" => Cli::Kiro,
                    _ => Cli::OpenCode,
                };

                let events_json: String = row.get(2)?;
                let events: Vec<WatchEvent> = serde_json::from_str(&events_json)?;

                Ok(Watcher {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    events,
                    prompt: row.get(3)?,
                    cli,
                    model: row.get(5)?,
                    debounce_seconds: row.get(6)?,
                    recursive: row.get(7)?,
                    enabled: row.get(8)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(9)?)?
                        .with_timezone(&Utc),
                    last_triggered_at: row
                        .get::<_, Option<String>>(10)?
                        .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
                    trigger_count: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(watchers)
    }

    pub fn delete_watcher(&self, id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM watchers WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_watcher_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE watchers SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
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

    // === RUNS ===

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

    // === DAEMON STATE ===

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

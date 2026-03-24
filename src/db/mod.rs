//! `SQLite` database layer for persistent storage.
//!
//! Handles all CRUD operations for tasks, watchers, execution logs,
//! and daemon state using a single persistent connection.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::state::{Cli, RunLog, Task, TriggerType, WatchEvent, Watcher};

/// Thread-safe `SQLite` database wrapper.
///
/// Uses a `Mutex<Connection>` instead of opening a new connection per operation,
/// which is more efficient for `SQLite`'s single-writer model.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Create and initialize a new database at the given path.
    ///
    /// Creates all required tables if they don't exist.
    pub fn new(db_path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let db = Database {
            conn: Mutex::new(conn),
        };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        conn.execute_batch(
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
            );

            CREATE TABLE IF NOT EXISTS watchers (
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
            );

            CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                exit_code INTEGER,
                trigger_type TEXT NOT NULL,
                FOREIGN KEY(task_id) REFERENCES tasks(id)
            );

            CREATE TABLE IF NOT EXISTS daemon_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        Ok(())
    }

    // ── Task operations ──────────────────────────────────────────────

    /// Insert or update a task (upsert).
    pub fn insert_or_update_task(&self, task: &Task) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT OR REPLACE INTO tasks
            (id, prompt, schedule_expr, cli, model, working_dir, enabled, created_at, expires_at, log_path)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &task.id,
                &task.prompt,
                &task.schedule_expr,
                task.cli.as_str(),
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

    /// Get a single task by ID.
    pub fn get_task(&self, id: &str) -> Result<Option<Task>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled,
                    created_at, expires_at, last_run_at, last_run_ok, log_path
             FROM tasks WHERE id = ?1",
        )?;

        let task = stmt
            .query_row(params![id], |row| {
                Ok(TaskRow {
                    id: row.get(0)?,
                    prompt: row.get(1)?,
                    schedule_expr: row.get(2)?,
                    cli_str: row.get(3)?,
                    model: row.get(4)?,
                    working_dir: row.get(5)?,
                    enabled: row.get(6)?,
                    created_at_str: row.get(7)?,
                    expires_at_str: row.get(8)?,
                    last_run_at_str: row.get(9)?,
                    last_run_ok: row.get(10)?,
                    log_path: row.get(11)?,
                })
            })
            .optional()?;

        match task {
            Some(row) => Ok(Some(row.into_task()?)),
            None => Ok(None),
        }
    }

    /// Retrieve all tasks ordered by creation date (newest first).
    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled,
                    created_at, expires_at, last_run_at, last_run_ok, log_path
             FROM tasks ORDER BY created_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                prompt: row.get(1)?,
                schedule_expr: row.get(2)?,
                cli_str: row.get(3)?,
                model: row.get(4)?,
                working_dir: row.get(5)?,
                enabled: row.get(6)?,
                created_at_str: row.get(7)?,
                expires_at_str: row.get(8)?,
                last_run_at_str: row.get(9)?,
                last_run_ok: row.get(10)?,
                log_path: row.get(11)?,
            })
        })?;

        let mut tasks = Vec::new();
        for row_result in rows {
            tasks.push(row_result?.into_task()?);
        }
        Ok(tasks)
    }

    /// Delete a task by ID.
    pub fn delete_task(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        // Delete associated runs first (FK constraint)
        conn.execute("DELETE FROM runs WHERE task_id = ?1", params![id])?;
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Update task enabled status.
    pub fn update_task_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE tasks SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(())
    }

    /// Update last run info for a task.
    pub fn update_task_last_run(&self, id: &str, success: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE tasks SET last_run_at = ?1, last_run_ok = ?2 WHERE id = ?3",
            params![Utc::now().to_rfc3339(), success, id],
        )?;
        Ok(())
    }

    // ── Watcher operations ───────────────────────────────────────────

    /// Insert or update a watcher (upsert).
    pub fn insert_or_update_watcher(&self, watcher: &Watcher) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
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
                watcher.cli.as_str(),
                &watcher.model,
                watcher.debounce_seconds,
                watcher.recursive,
                watcher.enabled,
                watcher.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Get a single watcher by ID.
    pub fn get_watcher(&self, id: &str) -> Result<Option<Watcher>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive,
                    enabled, created_at, last_triggered_at, trigger_count
             FROM watchers WHERE id = ?1",
        )?;

        let watcher = stmt
            .query_row(params![id], |row| {
                Ok(WatcherRow {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    events_json: row.get(2)?,
                    prompt: row.get(3)?,
                    cli_str: row.get(4)?,
                    model: row.get(5)?,
                    debounce_seconds: row.get(6)?,
                    recursive: row.get(7)?,
                    enabled: row.get(8)?,
                    created_at_str: row.get(9)?,
                    last_triggered_at_str: row.get(10)?,
                    trigger_count: row.get(11)?,
                })
            })
            .optional()?;

        match watcher {
            Some(row) => Ok(Some(row.into_watcher()?)),
            None => Ok(None),
        }
    }

    /// Retrieve all watchers ordered by creation date (newest first).
    pub fn list_watchers(&self) -> Result<Vec<Watcher>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive,
                    enabled, created_at, last_triggered_at, trigger_count
             FROM watchers ORDER BY created_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(WatcherRow {
                id: row.get(0)?,
                path: row.get(1)?,
                events_json: row.get(2)?,
                prompt: row.get(3)?,
                cli_str: row.get(4)?,
                model: row.get(5)?,
                debounce_seconds: row.get(6)?,
                recursive: row.get(7)?,
                enabled: row.get(8)?,
                created_at_str: row.get(9)?,
                last_triggered_at_str: row.get(10)?,
                trigger_count: row.get(11)?,
            })
        })?;

        let mut watchers = Vec::new();
        for row_result in rows {
            watchers.push(row_result?.into_watcher()?);
        }
        Ok(watchers)
    }

    /// List only enabled watchers (for reload on startup).
    pub fn list_enabled_watchers(&self) -> Result<Vec<Watcher>> {
        let all = self.list_watchers()?;
        Ok(all.into_iter().filter(|w| w.enabled).collect())
    }

    /// Delete a watcher by ID.
    pub fn delete_watcher(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute("DELETE FROM watchers WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Update watcher enabled status.
    pub fn update_watcher_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE watchers SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(())
    }

    /// Record that a watcher has been triggered.
    pub fn update_watcher_triggered(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE watchers SET last_triggered_at = ?1, trigger_count = trigger_count + 1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    // ── Run log operations ───────────────────────────────────────────

    /// Insert a task execution log entry.
    pub fn insert_run(&self, run: &RunLog) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO runs (task_id, started_at, finished_at, exit_code, trigger_type)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &run.task_id,
                run.started_at.to_rfc3339(),
                run.finished_at.map(|t| t.to_rfc3339()),
                run.exit_code,
                run.trigger_type.as_str(),
            ],
        )?;
        Ok(())
    }

    /// Get recent runs for a task.
    pub fn list_runs(&self, task_id: &str, limit: usize) -> Result<Vec<RunLog>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT task_id, started_at, finished_at, exit_code, trigger_type
             FROM runs WHERE task_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![task_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i32>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        let mut runs = Vec::new();
        for row_result in rows {
            let (task_id, started_at_str, finished_at_str, exit_code, trigger_str) = row_result?;
            let started_at =
                chrono::DateTime::parse_from_rfc3339(&started_at_str)?.with_timezone(&Utc);
            let finished_at = finished_at_str
                .as_ref()
                .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
                .transpose()?;
            let trigger_type = TriggerType::from_str(&trigger_str);

            runs.push(RunLog {
                task_id,
                started_at,
                finished_at,
                exit_code,
                trigger_type,
            });
        }
        Ok(runs)
    }

    // ── Daemon state operations ──────────────────────────────────────

    /// Store a key-value pair in daemon state.
    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT OR REPLACE INTO daemon_state (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Retrieve a value from daemon state by key.
    pub fn get_state(&self, key: &str) -> Result<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT value FROM daemon_state WHERE key = ?1")?;
        let value = stmt.query_row(params![key], |row| row.get(0)).optional()?;
        Ok(value)
    }
}

// ── Internal row types for deserialization ───────────────────────────

struct TaskRow {
    id: String,
    prompt: String,
    schedule_expr: String,
    cli_str: String,
    model: Option<String>,
    working_dir: Option<String>,
    enabled: bool,
    created_at_str: String,
    expires_at_str: Option<String>,
    last_run_at_str: Option<String>,
    last_run_ok: Option<bool>,
    log_path: String,
}

impl TaskRow {
    fn into_task(self) -> Result<Task> {
        let cli = Cli::from_str(&self.cli_str);
        let created_at =
            chrono::DateTime::parse_from_rfc3339(&self.created_at_str)?.with_timezone(&Utc);
        let expires_at = self
            .expires_at_str
            .as_ref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()?;
        let last_run_at = self
            .last_run_at_str
            .as_ref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()?;

        Ok(Task {
            id: self.id,
            prompt: self.prompt,
            schedule_expr: self.schedule_expr,
            cli,
            model: self.model,
            working_dir: self.working_dir,
            enabled: self.enabled,
            created_at,
            expires_at,
            last_run_at,
            last_run_ok: self.last_run_ok,
            log_path: self.log_path,
        })
    }
}

struct WatcherRow {
    id: String,
    path: String,
    events_json: String,
    prompt: String,
    cli_str: String,
    model: Option<String>,
    debounce_seconds: u64,
    recursive: bool,
    enabled: bool,
    created_at_str: String,
    last_triggered_at_str: Option<String>,
    trigger_count: u64,
}

impl WatcherRow {
    fn into_watcher(self) -> Result<Watcher> {
        let cli = Cli::from_str(&self.cli_str);
        let events: Vec<WatchEvent> = serde_json::from_str(&self.events_json)?;
        let created_at =
            chrono::DateTime::parse_from_rfc3339(&self.created_at_str)?.with_timezone(&Utc);
        let last_triggered_at = self
            .last_triggered_at_str
            .as_ref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()?;

        Ok(Watcher {
            id: self.id,
            path: self.path,
            events,
            prompt: self.prompt,
            cli,
            model: self.model,
            debounce_seconds: self.debounce_seconds,
            recursive: self.recursive,
            enabled: self.enabled,
            created_at,
            last_triggered_at,
            trigger_count: self.trigger_count,
        })
    }
}


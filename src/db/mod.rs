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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Cli, RunLog, Task, TriggerType, WatchEvent, Watcher};
    use chrono::{Duration, Utc};
    use tempfile::NamedTempFile;

    /// Create an in-memory-like DB backed by a temp file (`SQLite` needs a real file for WAL).
    fn test_db() -> Database {
        let tmp = NamedTempFile::new().expect("create temp file");
        let path = tmp.path().to_path_buf();
        // Keep the temp file alive by leaking it (tests are short-lived).
        std::mem::forget(tmp);
        Database::new(&path).expect("create test db")
    }

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            prompt: "Run tests".to_string(),
            schedule_expr: "0 9 * * *".to_string(),
            cli: Cli::OpenCode,
            model: None,
            working_dir: Some("/tmp/project".to_string()),
            enabled: true,
            created_at: Utc::now(),
            expires_at: None,
            last_run_at: None,
            last_run_ok: None,
            log_path: "/tmp/test.log".to_string(),
        }
    }

    fn sample_watcher(id: &str) -> Watcher {
        Watcher {
            id: id.to_string(),
            path: "/tmp/watched".to_string(),
            events: vec![WatchEvent::Create, WatchEvent::Modify],
            prompt: "Handle file change".to_string(),
            cli: Cli::Kiro,
            model: Some("claude-4".to_string()),
            debounce_seconds: 5,
            recursive: true,
            enabled: true,
            created_at: Utc::now(),
            last_triggered_at: None,
            trigger_count: 0,
        }
    }

    // ── Task CRUD ─────────────────────────────────────────────────

    #[test]
    fn test_insert_and_get_task() {
        let db = test_db();
        let task = sample_task("build-daily");
        db.insert_or_update_task(&task).unwrap();

        let retrieved = db.get_task("build-daily").unwrap().expect("task exists");
        assert_eq!(retrieved.id, "build-daily");
        assert_eq!(retrieved.prompt, "Run tests");
        assert_eq!(retrieved.schedule_expr, "0 9 * * *");
        assert!(matches!(retrieved.cli, Cli::OpenCode));
        assert_eq!(retrieved.working_dir.as_deref(), Some("/tmp/project"));
        assert!(retrieved.enabled);
    }

    #[test]
    fn test_get_nonexistent_task() {
        let db = test_db();
        let result = db.get_task("does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_upsert_task_overwrites() {
        let db = test_db();
        let mut task = sample_task("my-task");
        db.insert_or_update_task(&task).unwrap();

        task.prompt = "Updated prompt".to_string();
        task.schedule_expr = "*/10 * * * *".to_string();
        db.insert_or_update_task(&task).unwrap();

        let retrieved = db.get_task("my-task").unwrap().unwrap();
        assert_eq!(retrieved.prompt, "Updated prompt");
        assert_eq!(retrieved.schedule_expr, "*/10 * * * *");
    }

    #[test]
    fn test_list_tasks_ordered_by_created_at_desc() {
        let db = test_db();

        let mut t1 = sample_task("first");
        t1.created_at = Utc::now() - Duration::hours(2);
        let mut t2 = sample_task("second");
        t2.created_at = Utc::now() - Duration::hours(1);
        let mut t3 = sample_task("third");
        t3.created_at = Utc::now();

        db.insert_or_update_task(&t1).unwrap();
        db.insert_or_update_task(&t2).unwrap();
        db.insert_or_update_task(&t3).unwrap();

        let tasks = db.list_tasks().unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "third");
        assert_eq!(tasks[1].id, "second");
        assert_eq!(tasks[2].id, "first");
    }

    #[test]
    fn test_delete_task() {
        let db = test_db();
        db.insert_or_update_task(&sample_task("to-delete")).unwrap();
        assert!(db.get_task("to-delete").unwrap().is_some());

        db.delete_task("to-delete").unwrap();
        assert!(db.get_task("to-delete").unwrap().is_none());
    }

    #[test]
    fn test_update_task_enabled() {
        let db = test_db();
        db.insert_or_update_task(&sample_task("toggle-me")).unwrap();

        db.update_task_enabled("toggle-me", false).unwrap();
        let task = db.get_task("toggle-me").unwrap().unwrap();
        assert!(!task.enabled);

        db.update_task_enabled("toggle-me", true).unwrap();
        let task = db.get_task("toggle-me").unwrap().unwrap();
        assert!(task.enabled);
    }

    #[test]
    fn test_update_task_last_run() {
        let db = test_db();
        db.insert_or_update_task(&sample_task("run-me")).unwrap();

        db.update_task_last_run("run-me", true).unwrap();
        let task = db.get_task("run-me").unwrap().unwrap();
        assert!(task.last_run_at.is_some());
        assert_eq!(task.last_run_ok, Some(true));

        db.update_task_last_run("run-me", false).unwrap();
        let task = db.get_task("run-me").unwrap().unwrap();
        assert_eq!(task.last_run_ok, Some(false));
    }

    #[test]
    fn test_task_with_expiration() {
        let db = test_db();
        let mut task = sample_task("expiring");
        task.expires_at = Some(Utc::now() + Duration::hours(1));
        db.insert_or_update_task(&task).unwrap();

        let retrieved = db.get_task("expiring").unwrap().unwrap();
        assert!(retrieved.expires_at.is_some());
        assert!(!retrieved.is_expired());
    }

    // ── Watcher CRUD ──────────────────────────────────────────────

    #[test]
    fn test_insert_and_get_watcher() {
        let db = test_db();
        let watcher = sample_watcher("watch-src");
        db.insert_or_update_watcher(&watcher).unwrap();

        let retrieved = db
            .get_watcher("watch-src")
            .unwrap()
            .expect("watcher exists");
        assert_eq!(retrieved.id, "watch-src");
        assert_eq!(retrieved.path, "/tmp/watched");
        assert_eq!(retrieved.events.len(), 2);
        assert!(retrieved.events.contains(&WatchEvent::Create));
        assert!(retrieved.events.contains(&WatchEvent::Modify));
        assert!(matches!(retrieved.cli, Cli::Kiro));
        assert_eq!(retrieved.model.as_deref(), Some("claude-4"));
        assert_eq!(retrieved.debounce_seconds, 5);
        assert!(retrieved.recursive);
    }

    #[test]
    fn test_get_nonexistent_watcher() {
        let db = test_db();
        assert!(db.get_watcher("nope").unwrap().is_none());
    }

    #[test]
    fn test_list_and_delete_watchers() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("w1")).unwrap();
        db.insert_or_update_watcher(&sample_watcher("w2")).unwrap();

        assert_eq!(db.list_watchers().unwrap().len(), 2);

        db.delete_watcher("w1").unwrap();
        assert_eq!(db.list_watchers().unwrap().len(), 1);
        assert!(db.get_watcher("w1").unwrap().is_none());
    }

    #[test]
    fn test_list_enabled_watchers() {
        let db = test_db();
        let mut w1 = sample_watcher("enabled-w");
        w1.enabled = true;
        let mut w2 = sample_watcher("disabled-w");
        w2.enabled = false;

        db.insert_or_update_watcher(&w1).unwrap();
        db.insert_or_update_watcher(&w2).unwrap();

        let enabled = db.list_enabled_watchers().unwrap();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "enabled-w");
    }

    #[test]
    fn test_update_watcher_enabled() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("toggle-w"))
            .unwrap();

        db.update_watcher_enabled("toggle-w", false).unwrap();
        let w = db.get_watcher("toggle-w").unwrap().unwrap();
        assert!(!w.enabled);
    }

    #[test]
    fn test_update_watcher_triggered() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("trig-w"))
            .unwrap();

        db.update_watcher_triggered("trig-w").unwrap();
        let w = db.get_watcher("trig-w").unwrap().unwrap();
        assert!(w.last_triggered_at.is_some());
        assert_eq!(w.trigger_count, 1);

        db.update_watcher_triggered("trig-w").unwrap();
        let w = db.get_watcher("trig-w").unwrap().unwrap();
        assert_eq!(w.trigger_count, 2);
    }

    // ── Run log operations ────────────────────────────────────────

    #[test]
    fn test_insert_and_list_runs() {
        let db = test_db();
        // Need a task first for FK
        db.insert_or_update_task(&sample_task("run-task")).unwrap();

        let run = RunLog {
            task_id: "run-task".to_string(),
            started_at: Utc::now() - Duration::minutes(5),
            finished_at: Some(Utc::now()),
            exit_code: Some(0),
            trigger_type: TriggerType::Scheduled,
        };
        db.insert_run(&run).unwrap();

        let runs = db.list_runs("run-task", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].task_id, "run-task");
        assert_eq!(runs[0].exit_code, Some(0));
        assert!(matches!(runs[0].trigger_type, TriggerType::Scheduled));
    }

    #[test]
    fn test_list_runs_limit() {
        let db = test_db();
        db.insert_or_update_task(&sample_task("many-runs")).unwrap();

        for i in 0..10 {
            let run = RunLog {
                task_id: "many-runs".to_string(),
                started_at: Utc::now() - Duration::minutes(i),
                finished_at: Some(Utc::now()),
                exit_code: Some(0),
                trigger_type: TriggerType::Manual,
            };
            db.insert_run(&run).unwrap();
        }

        let runs = db.list_runs("many-runs", 3).unwrap();
        assert_eq!(runs.len(), 3);
    }

    #[test]
    fn test_delete_task_cascades_runs() {
        let db = test_db();
        db.insert_or_update_task(&sample_task("cascade-task"))
            .unwrap();
        let run = RunLog {
            task_id: "cascade-task".to_string(),
            started_at: Utc::now(),
            finished_at: None,
            exit_code: None,
            trigger_type: TriggerType::Watch,
        };
        db.insert_run(&run).unwrap();
        assert_eq!(db.list_runs("cascade-task", 10).unwrap().len(), 1);

        db.delete_task("cascade-task").unwrap();
        assert_eq!(db.list_runs("cascade-task", 10).unwrap().len(), 0);
    }

    // ── Daemon state ──────────────────────────────────────────────

    #[test]
    fn test_set_and_get_state() {
        let db = test_db();
        db.set_state("port", "7755").unwrap();
        assert_eq!(db.get_state("port").unwrap(), Some("7755".to_string()));
    }

    #[test]
    fn test_get_state_missing_key() {
        let db = test_db();
        assert!(db.get_state("missing").unwrap().is_none());
    }

    #[test]
    fn test_set_state_overwrites() {
        let db = test_db();
        db.set_state("version", "0.1.0").unwrap();
        db.set_state("version", "0.2.0").unwrap();
        assert_eq!(db.get_state("version").unwrap(), Some("0.2.0".to_string()));
    }
}

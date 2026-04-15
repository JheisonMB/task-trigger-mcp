

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::application::ports::{
    BackgroundAgentFieldsUpdate, BackgroundAgentRepository, RunRepository, StateRepository,
    WatcherFieldsUpdate, WatcherRepository,
};
use crate::domain::models::{
    BackgroundAgent, Cli, RunLog, RunStatus, TriggerType, WatchEvent, Watcher,
};

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
            "CREATE TABLE IF NOT EXISTS background_agents (
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
                log_path TEXT NOT NULL,
                timeout_minutes INTEGER NOT NULL DEFAULT 15
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
                trigger_count INTEGER NOT NULL DEFAULT 0,
                timeout_minutes INTEGER NOT NULL DEFAULT 15
            );

            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                background_agent_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                trigger_type TEXT NOT NULL,
                summary TEXT,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                exit_code INTEGER,
                timeout_at TEXT
            );

            CREATE TABLE IF NOT EXISTS daemon_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        // Migrate old schema if needed
        self.migrate(&conn)?;

        Ok(())
    }

    /// Run schema migrations for existing databases.
    fn migrate(&self, conn: &Connection) -> Result<()> {
        // Add timeout_minutes to background_agents if missing
        let has_timeout = conn
            .prepare("SELECT timeout_minutes FROM background_agents LIMIT 0")
            .is_ok();
        if !has_timeout {
            conn.execute_batch(
                "ALTER TABLE background_agents ADD COLUMN timeout_minutes INTEGER NOT NULL DEFAULT 15;",
            )?;
        }

        // Add timeout_minutes to watchers if missing
        let has_watcher_timeout = conn
            .prepare("SELECT timeout_minutes FROM watchers LIMIT 0")
            .is_ok();
        if !has_watcher_timeout {
            conn.execute_batch(
                "ALTER TABLE watchers ADD COLUMN timeout_minutes INTEGER NOT NULL DEFAULT 15;",
            )?;
        }

        // Migrate runs table from INTEGER id to TEXT id with new columns
        let has_status = conn.prepare("SELECT status FROM runs LIMIT 0").is_ok();
        if !has_status {
            conn.execute_batch(
                "ALTER TABLE runs RENAME TO runs_old;
                 CREATE TABLE runs (
                     id TEXT PRIMARY KEY,
                     background_agent_id TEXT NOT NULL,
                     status TEXT NOT NULL DEFAULT 'pending',
                     trigger_type TEXT NOT NULL,
                     summary TEXT,
                     started_at TEXT NOT NULL,
                     finished_at TEXT,
                     exit_code INTEGER,
                     timeout_at TEXT
                 );
                 INSERT INTO runs (id, background_agent_id, status, trigger_type, started_at, finished_at, exit_code)
                     SELECT CAST(id AS TEXT), background_agent_id, 'success', trigger_type, started_at, finished_at, exit_code
                     FROM runs_old;
                 DROP TABLE runs_old;",
            )?;
        }

        // Remove FK constraint from runs table so watchers can have runs too
        let has_fk: bool = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='runs'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|sql| sql.contains("FOREIGN KEY"))
            .unwrap_or(false);
        if has_fk {
            conn.execute_batch(
                "ALTER TABLE runs RENAME TO runs_old;
                 CREATE TABLE runs (
                     id TEXT PRIMARY KEY,
                     background_agent_id TEXT NOT NULL,
                     status TEXT NOT NULL DEFAULT 'pending',
                     trigger_type TEXT NOT NULL,
                     summary TEXT,
                     started_at TEXT NOT NULL,
                     finished_at TEXT,
                     exit_code INTEGER,
                     timeout_at TEXT
                 );
                 INSERT INTO runs SELECT * FROM runs_old;
                 DROP TABLE runs_old;",
            )?;
        }

        Ok(())
    }
}

// ── BackgroundAgent operations ──────────────────────────────────────────────

impl BackgroundAgentRepository for Database {
    fn insert_or_update_background_agent(&self, background_agent: &BackgroundAgent) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT OR REPLACE INTO background_agents
            (id, prompt, schedule_expr, cli, model, working_dir, enabled, created_at, expires_at, log_path, timeout_minutes)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &background_agent.id,
                &background_agent.prompt,
                &background_agent.schedule_expr,
                background_agent.cli.as_str(),
                &background_agent.model,
                &background_agent.working_dir,
                background_agent.enabled,
                background_agent.created_at.to_rfc3339(),
                background_agent.expires_at.map(|t| t.to_rfc3339()),
                &background_agent.log_path,
                background_agent.timeout_minutes as i64,
            ],
        )?;
        Ok(())
    }

    fn get_background_agent(&self, id: &str) -> Result<Option<BackgroundAgent>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled,
                    created_at, expires_at, last_run_at, last_run_ok, log_path, timeout_minutes
             FROM background_agents WHERE id = ?1",
        )?;

        let background_agent = stmt
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
                    timeout_minutes: row.get(12)?,
                })
            })
            .optional()?;

        match background_agent {
            Some(row) => Ok(Some(row.into_task()?)),
            None => Ok(None),
        }
    }

    fn list_background_agents(&self) -> Result<Vec<BackgroundAgent>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, prompt, schedule_expr, cli, model, working_dir, enabled,
                    created_at, expires_at, last_run_at, last_run_ok, log_path, timeout_minutes
             FROM background_agents ORDER BY created_at DESC",
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
                timeout_minutes: row.get(12)?,
            })
        })?;

        let mut background_agents = Vec::new();
        for row_result in rows {
            background_agents.push(row_result?.into_task()?);
        }
        Ok(background_agents)
    }

    fn delete_background_agent(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        // Delete associated runs first (FK constraint)
        conn.execute(
            "DELETE FROM runs WHERE background_agent_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM background_agents WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn update_background_agent_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE background_agents SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(())
    }

    fn update_background_agent_fields(
        &self,
        id: &str,
        fields: &BackgroundAgentFieldsUpdate<'_>,
    ) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        let mut sets = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(v) = fields.prompt {
            sets.push("prompt = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.schedule_expr {
            sets.push("schedule_expr = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.cli {
            sets.push("cli = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.model {
            sets.push("model = ?");
            values.push(Box::new(v.map(|s| s.to_string())));
        }
        if let Some(v) = fields.working_dir {
            sets.push("working_dir = ?");
            values.push(Box::new(v.map(|s| s.to_string())));
        }
        if let Some(v) = fields.expires_at {
            sets.push("expires_at = ?");
            values.push(Box::new(v.map(|s| s.to_string())));
        }

        if sets.is_empty() {
            return Ok(false);
        }

        let placeholders: Vec<String> = sets
            .iter()
            .enumerate()
            .map(|(i, s)| s.replace('?', &format!("?{}", i + 1)))
            .collect();

        let id_param = sets.len() + 1;
        let sql = format!(
            "UPDATE background_agents SET {} WHERE id = ?{}",
            placeholders.join(", "),
            id_param
        );
        values.push(Box::new(id.to_string()));

        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = conn.execute(&sql, params.as_slice())?;
        Ok(rows > 0)
    }

    fn update_background_agent_last_run(&self, id: &str, success: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE background_agents SET last_run_at = ?1, last_run_ok = ?2 WHERE id = ?3",
            params![Utc::now().to_rfc3339(), success, id],
        )?;
        Ok(())
    }
}

// ── Watcher operations ───────────────────────────────────────────

impl WatcherRepository for Database {
    fn insert_or_update_watcher(&self, watcher: &Watcher) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let events_json = serde_json::to_string(&watcher.events)?;

        conn.execute(
            "INSERT OR REPLACE INTO watchers
            (id, path, events, prompt, cli, model, debounce_seconds, recursive, enabled, created_at, timeout_minutes)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &watcher.id,
                &watcher.path,
                &events_json,
                &watcher.prompt,
                watcher.cli.as_str(),
                &watcher.model,
                watcher.debounce_seconds as i64,
                watcher.recursive,
                watcher.enabled,
                watcher.created_at.to_rfc3339(),
                watcher.timeout_minutes as i64,
            ],
        )?;
        Ok(())
    }

    fn get_watcher(&self, id: &str) -> Result<Option<Watcher>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive,
                    enabled, created_at, last_triggered_at, trigger_count, timeout_minutes
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
                    timeout_minutes: row.get(12)?,
                })
            })
            .optional()?;

        match watcher {
            Some(row) => Ok(Some(row.into_watcher()?)),
            None => Ok(None),
        }
    }

    fn list_watchers(&self) -> Result<Vec<Watcher>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, path, events, prompt, cli, model, debounce_seconds, recursive,
                    enabled, created_at, last_triggered_at, trigger_count, timeout_minutes
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
                timeout_minutes: row.get(12)?,
            })
        })?;

        let mut watchers = Vec::new();
        for row_result in rows {
            watchers.push(row_result?.into_watcher()?);
        }
        Ok(watchers)
    }

    fn list_enabled_watchers(&self) -> Result<Vec<Watcher>> {
        let all = self.list_watchers()?;
        Ok(all.into_iter().filter(|w| w.enabled).collect())
    }

    fn delete_watcher(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "DELETE FROM runs WHERE background_agent_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM watchers WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn update_watcher_enabled(&self, id: &str, enabled: bool) -> Result<()> {
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

    fn update_watcher_fields(&self, id: &str, fields: &WatcherFieldsUpdate<'_>) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        let mut sets = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(v) = fields.prompt {
            sets.push("prompt = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.path {
            sets.push("path = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.events {
            sets.push("events = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.cli {
            sets.push("cli = ?");
            values.push(Box::new(v.to_string()));
        }
        if let Some(v) = fields.model {
            sets.push("model = ?");
            values.push(Box::new(v.map(|s| s.to_string())));
        }
        if let Some(v) = fields.debounce_seconds {
            sets.push("debounce_seconds = ?");
            values.push(Box::new(v as i64));
        }
        if let Some(v) = fields.recursive {
            sets.push("recursive = ?");
            values.push(Box::new(v));
        }

        if sets.is_empty() {
            return Ok(false);
        }

        let placeholders: Vec<String> = sets
            .iter()
            .enumerate()
            .map(|(i, s)| s.replace('?', &format!("?{}", i + 1)))
            .collect();

        let id_param = sets.len() + 1;
        let sql = format!(
            "UPDATE watchers SET {} WHERE id = ?{}",
            placeholders.join(", "),
            id_param
        );
        values.push(Box::new(id.to_string()));

        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = conn.execute(&sql, params.as_slice())?;
        Ok(rows > 0)
    }

    fn update_watcher_triggered(&self, id: &str) -> Result<()> {
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
}

// ── Run log operations ───────────────────────────────────────────

impl RunRepository for Database {
    fn insert_run(&self, run: &RunLog) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO runs (id, background_agent_id, status, trigger_type, summary, started_at, finished_at, exit_code, timeout_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &run.id,
                &run.background_agent_id,
                run.status.as_str(),
                run.trigger_type.as_str(),
                &run.summary,
                run.started_at.to_rfc3339(),
                run.finished_at.map(|t| t.to_rfc3339()),
                run.exit_code,
                run.timeout_at.map(|t| t.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    fn list_runs(&self, background_agent_id: &str, limit: usize) -> Result<Vec<RunLog>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, background_agent_id, status, trigger_type, summary, started_at, finished_at, exit_code, timeout_at
             FROM runs WHERE background_agent_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![background_agent_id, limit as i64], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                background_agent_id: row.get(1)?,
                status_str: row.get(2)?,
                trigger_str: row.get(3)?,
                summary: row.get(4)?,
                started_at_str: row.get(5)?,
                finished_at_str: row.get(6)?,
                exit_code: row.get(7)?,
                timeout_at_str: row.get(8)?,
            })
        })?;

        let mut runs = Vec::new();
        for row_result in rows {
            runs.push(row_result?.into_run_log()?);
        }
        Ok(runs)
    }

    fn list_all_recent_runs(&self, limit: usize) -> Result<Vec<RunLog>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, background_agent_id, status, trigger_type, summary, started_at, finished_at, exit_code, timeout_at
             FROM runs ORDER BY started_at DESC LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                background_agent_id: row.get(1)?,
                status_str: row.get(2)?,
                trigger_str: row.get(3)?,
                summary: row.get(4)?,
                started_at_str: row.get(5)?,
                finished_at_str: row.get(6)?,
                exit_code: row.get(7)?,
                timeout_at_str: row.get(8)?,
            })
        })?;

        let mut runs = Vec::new();
        for row_result in rows {
            runs.push(row_result?.into_run_log()?);
        }
        Ok(runs)
    }

    fn get_active_run(&self, background_agent_id: &str) -> Result<Option<RunLog>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, background_agent_id, status, trigger_type, summary, started_at, finished_at, exit_code, timeout_at
             FROM runs WHERE background_agent_id = ?1 AND status IN ('pending', 'in_progress') LIMIT 1",
        )?;

        let run = stmt
            .query_row(params![background_agent_id], |row| {
                Ok(RunRow {
                    id: row.get(0)?,
                    background_agent_id: row.get(1)?,
                    status_str: row.get(2)?,
                    trigger_str: row.get(3)?,
                    summary: row.get(4)?,
                    started_at_str: row.get(5)?,
                    finished_at_str: row.get(6)?,
                    exit_code: row.get(7)?,
                    timeout_at_str: row.get(8)?,
                })
            })
            .optional()?;

        match run {
            Some(row) => Ok(Some(row.into_run_log()?)),
            None => Ok(None),
        }
    }

    fn update_run_status(
        &self,
        run_id: &str,
        status: RunStatus,
        summary: Option<&str>,
    ) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let finished_at = if status.is_active() {
            None
        } else {
            Some(Utc::now().to_rfc3339())
        };
        let rows = conn.execute(
            "UPDATE runs SET status = ?1, summary = COALESCE(?2, summary), finished_at = COALESCE(?3, finished_at)
             WHERE id = ?4",
            params![status.as_str(), summary, finished_at, run_id],
        )?;
        Ok(rows > 0)
    }

    fn update_run_exit_code(&self, run_id: &str, exit_code: i32) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let rows = conn.execute(
            "UPDATE runs SET exit_code = ?1 WHERE id = ?2",
            params![exit_code, run_id],
        )?;
        Ok(rows > 0)
    }

    fn get_run(&self, run_id: &str) -> Result<Option<RunLog>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, background_agent_id, status, trigger_type, summary, started_at, finished_at, exit_code, timeout_at
             FROM runs WHERE id = ?1",
        )?;

        let run = stmt
            .query_row(params![run_id], |row| {
                Ok(RunRow {
                    id: row.get(0)?,
                    background_agent_id: row.get(1)?,
                    status_str: row.get(2)?,
                    trigger_str: row.get(3)?,
                    summary: row.get(4)?,
                    started_at_str: row.get(5)?,
                    finished_at_str: row.get(6)?,
                    exit_code: row.get(7)?,
                    timeout_at_str: row.get(8)?,
                })
            })
            .optional()?;

        match run {
            Some(row) => Ok(Some(row.into_run_log()?)),
            None => Ok(None),
        }
    }
}

// ── Daemon state operations ──────────────────────────────────────

impl StateRepository for Database {
    fn set_state(&self, key: &str, value: &str) -> Result<()> {
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

    fn get_state(&self, key: &str) -> Result<Option<String>> {
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
    timeout_minutes: i64,
}

impl TaskRow {
    fn into_task(self) -> Result<BackgroundAgent> {
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

        Ok(BackgroundAgent {
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
            timeout_minutes: self.timeout_minutes as u32,
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
    debounce_seconds: i64,
    recursive: bool,
    enabled: bool,
    created_at_str: String,
    last_triggered_at_str: Option<String>,
    trigger_count: i64,
    timeout_minutes: i64,
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
            debounce_seconds: self.debounce_seconds as u64,
            recursive: self.recursive,
            enabled: self.enabled,
            created_at,
            last_triggered_at,
            trigger_count: self.trigger_count as u64,
            timeout_minutes: self.timeout_minutes as u32,
        })
    }
}

struct RunRow {
    id: String,
    background_agent_id: String,
    status_str: String,
    trigger_str: String,
    summary: Option<String>,
    started_at_str: String,
    finished_at_str: Option<String>,
    exit_code: Option<i32>,
    timeout_at_str: Option<String>,
}

impl RunRow {
    fn into_run_log(self) -> Result<RunLog> {
        let started_at =
            chrono::DateTime::parse_from_rfc3339(&self.started_at_str)?.with_timezone(&Utc);
        let finished_at = self
            .finished_at_str
            .as_ref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()?;
        let timeout_at = self
            .timeout_at_str
            .as_ref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()?;

        Ok(RunLog {
            id: self.id,
            background_agent_id: self.background_agent_id,
            status: RunStatus::from_str(&self.status_str),
            trigger_type: TriggerType::from_str(&self.trigger_str),
            summary: self.summary,
            started_at,
            finished_at,
            exit_code: self.exit_code,
            timeout_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{
        BackgroundAgent, Cli, RunLog, RunStatus, TriggerType, WatchEvent, Watcher,
    };
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

    fn sample_task(id: &str) -> BackgroundAgent {
        BackgroundAgent {
            id: id.to_string(),
            prompt: "Run tests".to_string(),
            schedule_expr: "0 9 * * *".to_string(),
            cli: Cli::new("opencode"),
            model: None,
            working_dir: Some("/tmp/project".to_string()),
            enabled: true,
            created_at: Utc::now(),
            expires_at: None,
            last_run_at: None,
            last_run_ok: None,
            log_path: "/tmp/test.log".to_string(),
            timeout_minutes: 15,
        }
    }

    fn sample_watcher(id: &str) -> Watcher {
        Watcher {
            id: id.to_string(),
            path: "/tmp/watched".to_string(),
            events: vec![WatchEvent::Create, WatchEvent::Modify],
            prompt: "Handle file change".to_string(),
            cli: Cli::new("kiro"),
            model: Some("claude-4".to_string()),
            debounce_seconds: 5,
            recursive: true,
            enabled: true,
            created_at: Utc::now(),
            last_triggered_at: None,
            trigger_count: 0,
            timeout_minutes: 15,
        }
    }

    // ── BackgroundAgent CRUD ─────────────────────────────────────────────────

    #[test]
    fn test_insert_and_get_task() {
        let db = test_db();
        let background_agent = sample_task("build-daily");
        db.insert_or_update_background_agent(&background_agent)
            .unwrap();

        let retrieved = db
            .get_background_agent("build-daily")
            .unwrap()
            .expect("background_agent exists");
        assert_eq!(retrieved.id, "build-daily");
        assert_eq!(retrieved.prompt, "Run tests");
        assert_eq!(retrieved.schedule_expr, "0 9 * * *");
        assert_eq!(retrieved.cli.as_str(), "opencode");
        assert_eq!(retrieved.working_dir.as_deref(), Some("/tmp/project"));
        assert!(retrieved.enabled);
    }

    #[test]
    fn test_get_nonexistent_task() {
        let db = test_db();
        let result = db.get_background_agent("does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_upsert_task_overwrites() {
        let db = test_db();
        let mut background_agent = sample_task("my-background_agent");
        db.insert_or_update_background_agent(&background_agent)
            .unwrap();

        background_agent.prompt = "Updated prompt".to_string();
        background_agent.schedule_expr = "*/10 * * * *".to_string();
        db.insert_or_update_background_agent(&background_agent)
            .unwrap();

        let retrieved = db
            .get_background_agent("my-background_agent")
            .unwrap()
            .unwrap();
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

        db.insert_or_update_background_agent(&t1).unwrap();
        db.insert_or_update_background_agent(&t2).unwrap();
        db.insert_or_update_background_agent(&t3).unwrap();

        let background_agents = db.list_background_agents().unwrap();
        assert_eq!(background_agents.len(), 3);
        assert_eq!(background_agents[0].id, "third");
        assert_eq!(background_agents[1].id, "second");
        assert_eq!(background_agents[2].id, "first");
    }

    #[test]
    fn test_delete_task() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("to-delete"))
            .unwrap();
        assert!(db.get_background_agent("to-delete").unwrap().is_some());

        db.delete_background_agent("to-delete").unwrap();
        assert!(db.get_background_agent("to-delete").unwrap().is_none());
    }

    #[test]
    fn test_update_task_enabled() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("toggle-me"))
            .unwrap();

        db.update_background_agent_enabled("toggle-me", false)
            .unwrap();
        let background_agent = db.get_background_agent("toggle-me").unwrap().unwrap();
        assert!(!background_agent.enabled);

        db.update_background_agent_enabled("toggle-me", true)
            .unwrap();
        let background_agent = db.get_background_agent("toggle-me").unwrap().unwrap();
        assert!(background_agent.enabled);
    }

    #[test]
    fn test_update_task_last_run() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("run-me"))
            .unwrap();

        db.update_background_agent_last_run("run-me", true).unwrap();
        let background_agent = db.get_background_agent("run-me").unwrap().unwrap();
        assert!(background_agent.last_run_at.is_some());
        assert_eq!(background_agent.last_run_ok, Some(true));

        db.update_background_agent_last_run("run-me", false)
            .unwrap();
        let background_agent = db.get_background_agent("run-me").unwrap().unwrap();
        assert_eq!(background_agent.last_run_ok, Some(false));
    }

    #[test]
    fn test_task_with_expiration() {
        let db = test_db();
        let mut background_agent = sample_task("expiring");
        background_agent.expires_at = Some(Utc::now() + Duration::hours(1));
        db.insert_or_update_background_agent(&background_agent)
            .unwrap();

        let retrieved = db.get_background_agent("expiring").unwrap().unwrap();
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
        assert_eq!(retrieved.cli.as_str(), "kiro");
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
        // Need a background_agent first for FK
        db.insert_or_update_background_agent(&sample_task("run-background_agent"))
            .unwrap();

        let run = RunLog {
            id: uuid::Uuid::new_v4().to_string(),
            background_agent_id: "run-background_agent".to_string(),
            status: RunStatus::Success,
            trigger_type: TriggerType::Scheduled,
            summary: None,
            started_at: Utc::now() - Duration::minutes(5),
            finished_at: Some(Utc::now()),
            exit_code: Some(0),
            timeout_at: None,
        };
        db.insert_run(&run).unwrap();

        let runs = db.list_runs("run-background_agent", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].background_agent_id, "run-background_agent");
        assert_eq!(runs[0].exit_code, Some(0));
        assert!(matches!(runs[0].trigger_type, TriggerType::Scheduled));
    }

    #[test]
    fn test_list_runs_limit() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("many-runs"))
            .unwrap();

        for i in 0..10 {
            let run = RunLog {
                id: uuid::Uuid::new_v4().to_string(),
                background_agent_id: "many-runs".to_string(),
                status: RunStatus::Success,
                trigger_type: TriggerType::Manual,
                summary: None,
                started_at: Utc::now() - Duration::minutes(i),
                finished_at: Some(Utc::now()),
                exit_code: Some(0),
                timeout_at: None,
            };
            db.insert_run(&run).unwrap();
        }

        let runs = db.list_runs("many-runs", 3).unwrap();
        assert_eq!(runs.len(), 3);
    }

    #[test]
    fn test_delete_task_cascades_runs() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("cascade-background_agent"))
            .unwrap();
        let run = RunLog {
            id: uuid::Uuid::new_v4().to_string(),
            background_agent_id: "cascade-background_agent".to_string(),
            status: RunStatus::Pending,
            trigger_type: TriggerType::Watch,
            summary: None,
            started_at: Utc::now(),
            finished_at: None,
            exit_code: None,
            timeout_at: None,
        };
        db.insert_run(&run).unwrap();
        assert_eq!(
            db.list_runs("cascade-background_agent", 10).unwrap().len(),
            1
        );

        db.delete_background_agent("cascade-background_agent")
            .unwrap();
        assert_eq!(
            db.list_runs("cascade-background_agent", 10).unwrap().len(),
            0
        );
    }

    // ── BackgroundAgent field updates ────────────────────────────────────────

    #[test]
    fn test_update_task_fields_prompt() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("upd-background_agent"))
            .unwrap();

        let fields = BackgroundAgentFieldsUpdate {
            prompt: Some("New prompt"),
            ..Default::default()
        };
        assert!(db
            .update_background_agent_fields("upd-background_agent", &fields)
            .unwrap());

        let t = db
            .get_background_agent("upd-background_agent")
            .unwrap()
            .unwrap();
        assert_eq!(t.prompt, "New prompt");
        assert_eq!(t.schedule_expr, "0 9 * * *"); // unchanged
    }

    #[test]
    fn test_update_task_fields_multiple() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("upd-multi"))
            .unwrap();

        let fields = BackgroundAgentFieldsUpdate {
            prompt: Some("Updated prompt"),
            schedule_expr: Some("*/10 * * * *"),
            cli: Some("kiro"),
            model: Some(Some("gpt-5")),
            ..Default::default()
        };
        assert!(db
            .update_background_agent_fields("upd-multi", &fields)
            .unwrap());

        let t = db.get_background_agent("upd-multi").unwrap().unwrap();
        assert_eq!(t.prompt, "Updated prompt");
        assert_eq!(t.schedule_expr, "*/10 * * * *");
        assert_eq!(t.cli.as_str(), "kiro");
        assert_eq!(t.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn test_update_task_fields_clear_optional() {
        let db = test_db();
        let mut background_agent = sample_task("upd-clear");
        background_agent.model = Some("claude-4".to_string());
        db.insert_or_update_background_agent(&background_agent)
            .unwrap();

        let fields = BackgroundAgentFieldsUpdate {
            model: Some(None), // clear model
            ..Default::default()
        };
        assert!(db
            .update_background_agent_fields("upd-clear", &fields)
            .unwrap());

        let t = db.get_background_agent("upd-clear").unwrap().unwrap();
        assert!(t.model.is_none());
    }

    #[test]
    fn test_update_task_fields_no_fields_returns_false() {
        let db = test_db();
        db.insert_or_update_background_agent(&sample_task("upd-noop"))
            .unwrap();

        let fields = BackgroundAgentFieldsUpdate::default();
        assert!(!db
            .update_background_agent_fields("upd-noop", &fields)
            .unwrap());
    }

    #[test]
    fn test_update_task_fields_nonexistent_returns_false() {
        let db = test_db();

        let fields = BackgroundAgentFieldsUpdate {
            prompt: Some("ghost"),
            ..Default::default()
        };
        assert!(!db
            .update_background_agent_fields("nonexistent", &fields)
            .unwrap());
    }

    // ── Watcher field updates ─────────────────────────────────────

    #[test]
    fn test_update_watcher_fields_prompt() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("upd-watch"))
            .unwrap();

        let fields = WatcherFieldsUpdate {
            prompt: Some("New watcher prompt"),
            ..Default::default()
        };
        assert!(db.update_watcher_fields("upd-watch", &fields).unwrap());

        let w = db.get_watcher("upd-watch").unwrap().unwrap();
        assert_eq!(w.prompt, "New watcher prompt");
        assert_eq!(w.path, "/tmp/watched"); // unchanged
    }

    #[test]
    fn test_update_watcher_fields_multiple() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("upd-wmulti"))
            .unwrap();

        let events_json = serde_json::to_string(&vec![WatchEvent::Delete]).unwrap();
        let fields = WatcherFieldsUpdate {
            path: Some("/new/path"),
            events: Some(&events_json),
            debounce_seconds: Some(10),
            recursive: Some(false),
            ..Default::default()
        };
        assert!(db.update_watcher_fields("upd-wmulti", &fields).unwrap());

        let w = db.get_watcher("upd-wmulti").unwrap().unwrap();
        assert_eq!(w.path, "/new/path");
        assert_eq!(w.events, vec![WatchEvent::Delete]);
        assert_eq!(w.debounce_seconds, 10);
        assert!(!w.recursive);
    }

    #[test]
    fn test_update_watcher_fields_clear_model() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("upd-wclr"))
            .unwrap();
        // sample_watcher has model = Some("claude-4")

        let fields = WatcherFieldsUpdate {
            model: Some(None),
            ..Default::default()
        };
        assert!(db.update_watcher_fields("upd-wclr", &fields).unwrap());

        let w = db.get_watcher("upd-wclr").unwrap().unwrap();
        assert!(w.model.is_none());
    }

    #[test]
    fn test_update_watcher_fields_no_fields_returns_false() {
        let db = test_db();
        db.insert_or_update_watcher(&sample_watcher("upd-wnoop"))
            .unwrap();

        let fields = WatcherFieldsUpdate::default();
        assert!(!db.update_watcher_fields("upd-wnoop", &fields).unwrap());
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

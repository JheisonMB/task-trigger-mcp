use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::application::ports::{AgentRepository, RunRepository, StateRepository};
use crate::domain::models::{Agent, Cli, RunLog, RunStatus, Trigger, TriggerType};

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
            "CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                trigger_type TEXT,
                trigger_config TEXT,
                cli TEXT NOT NULL,
                model TEXT,
                working_dir TEXT,
                enabled BOOLEAN NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                log_path TEXT NOT NULL,
                timeout_minutes INTEGER NOT NULL DEFAULT 15,
                expires_at TEXT,
                last_run_at TEXT,
                last_run_ok BOOLEAN,
                last_triggered_at TEXT,
                trigger_count INTEGER NOT NULL DEFAULT 0
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
            );

            CREATE TABLE IF NOT EXISTS interactive_sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                cli TEXT NOT NULL,
                working_dir TEXT NOT NULL,
                args TEXT,
                started_at TEXT NOT NULL,
                exited_at TEXT,
                exit_code INTEGER,
                status TEXT NOT NULL DEFAULT 'active'
            );

            CREATE TABLE IF NOT EXISTS terminal_sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                shell TEXT NOT NULL,
                working_dir TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_active TEXT,
                status TEXT NOT NULL DEFAULT 'idle'
            );

            CREATE TABLE IF NOT EXISTS groups (
                id TEXT PRIMARY KEY,
                orientation TEXT NOT NULL DEFAULT 'horizontal',
                session_a TEXT NOT NULL,
                session_b TEXT NOT NULL,
                created_at TEXT NOT NULL
            );",
        )?;

        Ok(())
    }
}

// ── Interactive session registry ────────────────────────────────────────

/// Record of an interactive agent session (persisted in SQLite).
#[allow(dead_code)]
pub struct InteractiveSession {
    pub id: String,
    pub name: String,
    pub cli: String,
    pub working_dir: String,
    pub args: Option<String>,
    pub started_at: String,
    pub status: String, // active, completed, error
}

#[allow(dead_code)]
pub struct TerminalSession {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub working_dir: String,
    pub created_at: String,
}

impl Database {
    /// Insert a new interactive session as active.
    pub fn insert_interactive_session(
        &self,
        id: &str,
        name: &str,
        cli: &str,
        working_dir: &str,
        args: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO interactive_sessions (id, name, cli, working_dir, args, started_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active')",
            params![id, name, cli, working_dir, args, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Mark a session as exited with a status and optional exit code.
    pub fn finish_interactive_session(&self, id: &str, exit_code: i32) -> Result<()> {
        let status = if exit_code == 0 { "completed" } else { "error" };
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE interactive_sessions SET exited_at = ?1, exit_code = ?2, status = ?3 WHERE id = ?4",
            params![Utc::now().to_rfc3339(), exit_code, status, id],
        )?;
        Ok(())
    }

    /// Get all sessions with status = 'active'.
    pub fn get_active_sessions(&self) -> Result<Vec<InteractiveSession>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, cli, working_dir, args, started_at, status
             FROM interactive_sessions WHERE status = 'active' ORDER BY started_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(InteractiveSession {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    cli: row.get(2)?,
                    working_dir: row.get(3)?,
                    args: row.get(4)?,
                    started_at: row.get(5)?,
                    status: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Mark all 'active' sessions as 'orphaned' (called on startup cleanup).
    pub fn mark_orphaned_sessions(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE interactive_sessions SET status = 'orphaned' WHERE status = 'active'",
            [],
        )?;
        Ok(())
    }

    /// Insert a terminal session record.
    pub fn insert_terminal_session(
        &self,
        id: &str,
        name: &str,
        shell: &str,
        working_dir: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO terminal_sessions (id, name, shell, working_dir, created_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, 'idle')",
            params![id, name, shell, working_dir, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Mark a terminal session as finished.
    pub fn finish_terminal_session(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE terminal_sessions SET status = 'finished', last_active = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Get all terminal sessions that are still active (idle = was active when canopy last ran).
    pub fn get_active_terminal_sessions(&self) -> Result<Vec<TerminalSession>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, shell, working_dir, created_at
             FROM terminal_sessions WHERE status = 'idle' ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(TerminalSession {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    shell: row.get(2)?,
                    working_dir: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Mark all active terminal sessions as orphaned (called on startup cleanup).
    pub fn mark_orphaned_terminal_sessions(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE terminal_sessions SET status = 'orphaned' WHERE status = 'idle'",
            [],
        )?;
        Ok(())
    }

    /// Persist a split group to the database.
    pub fn insert_group(
        &self,
        id: &str,
        orientation: &str,
        session_a: &str,
        session_b: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO groups (id, orientation, session_a, session_b, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, orientation, session_a, session_b, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Remove a split group from the database.
    pub fn delete_group(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute("DELETE FROM groups WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Agent operations ────────────────────────────────────────────────────

const AGENT_COLUMNS: &str = "id, prompt, trigger_type, trigger_config, cli, model, working_dir, \
                             enabled, created_at, log_path, timeout_minutes, expires_at, last_run_at, \
                             last_run_ok, last_triggered_at, trigger_count";

impl AgentRepository for Database {
    fn upsert_agent(&self, agent: &Agent) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        let (trigger_type, trigger_config) = match &agent.trigger {
            Some(trigger) => (Some(trigger.type_str().to_string()), Some(serde_json::to_string(trigger)?)),
            None => (None, None),
        };

        conn.execute(
            &format!("INSERT OR REPLACE INTO agents ({AGENT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"),
            params![
                &agent.id,
                &agent.prompt,
                trigger_type,
                trigger_config,
                agent.cli.as_str(),
                &agent.model,
                &agent.working_dir,
                agent.enabled,
                agent.created_at.to_rfc3339(),
                &agent.log_path,
                agent.timeout_minutes as i64,
                agent.expires_at.map(|t| t.to_rfc3339()),
                agent.last_run_at.map(|t| t.to_rfc3339()),
                agent.last_run_ok,
                agent.last_triggered_at.map(|t| t.to_rfc3339()),
                agent.trigger_count as i64,
            ],
        )?;
        Ok(())
    }

    fn get_agent(&self, id: &str) -> Result<Option<Agent>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"))?;

        let row = stmt
            .query_row(params![id], |row| {
                Ok(AgentRow {
                    id: row.get(0)?,
                    prompt: row.get(1)?,
                    trigger_type: row.get(2)?,
                    trigger_config: row.get(3)?,
                    cli_str: row.get(4)?,
                    model: row.get(5)?,
                    working_dir: row.get(6)?,
                    enabled: row.get(7)?,
                    created_at_str: row.get(8)?,
                    log_path: row.get(9)?,
                    timeout_minutes: row.get(10)?,
                    expires_at_str: row.get(11)?,
                    last_run_at_str: row.get(12)?,
                    last_run_ok: row.get(13)?,
                    last_triggered_at_str: row.get(14)?,
                    trigger_count: row.get(15)?,
                })
            })
            .optional()?;

        match row {
            Some(r) => Ok(Some(r.into_agent()?)),
            None => Ok(None),
        }
    }

    fn list_agents(&self) -> Result<Vec<Agent>> {
        self.list_agents_where("")
    }

    fn list_cron_agents(&self) -> Result<Vec<Agent>> {
        self.list_agents_where("WHERE trigger_type = 'cron' AND enabled = 1")
    }

    fn list_watch_agents(&self) -> Result<Vec<Agent>> {
        self.list_agents_where("WHERE trigger_type = 'watch' AND enabled = 1")
    }

    fn delete_agent(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute("DELETE FROM runs WHERE background_agent_id = ?1", params![id])?;
        conn.execute("DELETE FROM agents WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn update_agent_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE agents SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(())
    }

    fn update_agent_last_run(&self, id: &str, success: bool) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE agents SET last_run_at = ?1, last_run_ok = ?2 WHERE id = ?3",
            params![Utc::now().to_rfc3339(), success, id],
        )?;
        Ok(())
    }

    fn update_agent_triggered(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "UPDATE agents SET last_triggered_at = ?1, trigger_count = trigger_count + 1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }
}

impl Database {
    fn list_agents_where(&self, where_clause: &str) -> Result<Vec<Agent>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let sql = format!("SELECT {AGENT_COLUMNS} FROM agents {where_clause} ORDER BY created_at DESC");
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt.query_map([], |row| {
            Ok(AgentRow {
                id: row.get(0)?,
                prompt: row.get(1)?,
                trigger_type: row.get(2)?,
                trigger_config: row.get(3)?,
                cli_str: row.get(4)?,
                model: row.get(5)?,
                working_dir: row.get(6)?,
                enabled: row.get(7)?,
                created_at_str: row.get(8)?,
                log_path: row.get(9)?,
                timeout_minutes: row.get(10)?,
                expires_at_str: row.get(11)?,
                last_run_at_str: row.get(12)?,
                last_run_ok: row.get(13)?,
                last_triggered_at_str: row.get(14)?,
                trigger_count: row.get(15)?,
            })
        })?;

        let mut agents = Vec::new();
        for row_result in rows {
            agents.push(row_result?.into_agent()?);
        }
        Ok(agents)
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

// ── Internal row types for deserialization ────────────────────────────

struct AgentRow {
    id: String,
    prompt: String,
    #[allow(dead_code)]
    trigger_type: Option<String>,
    trigger_config: Option<String>,
    cli_str: String,
    model: Option<String>,
    working_dir: Option<String>,
    enabled: bool,
    created_at_str: String,
    log_path: String,
    timeout_minutes: i64,
    expires_at_str: Option<String>,
    last_run_at_str: Option<String>,
    last_run_ok: Option<bool>,
    last_triggered_at_str: Option<String>,
    trigger_count: i64,
}

impl AgentRow {
    fn into_agent(self) -> Result<Agent> {
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
        let last_triggered_at = self
            .last_triggered_at_str
            .as_ref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()?;

        let trigger = self
            .trigger_config
            .as_ref()
            .map(|json| serde_json::from_str::<Trigger>(json))
            .transpose()?;

        Ok(Agent {
            id: self.id,
            prompt: self.prompt,
            trigger,
            cli,
            model: self.model,
            working_dir: self.working_dir,
            enabled: self.enabled,
            created_at,
            log_path: self.log_path,
            timeout_minutes: self.timeout_minutes as u32,
            expires_at,
            last_run_at,
            last_run_ok: self.last_run_ok,
            last_triggered_at,
            trigger_count: self.trigger_count as u64,
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
mod tests;
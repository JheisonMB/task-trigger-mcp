//! SQLite repositories for sync_messages and sync_locks.

use anyhow::Result;

use crate::db::Database;
use crate::domain::sync::{LockType, MessageKind, SyncLock, SyncMessage};

impl Database {
    // ── sync_messages ──────────────────────────────────────────────────

    pub fn insert_sync_message(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        kind: MessageKind,
        message: &str,
        payload: Option<&str>,
    ) -> Result<SyncMessage> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO sync_messages (workdir, agent_id, agent_name, kind, message, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![workdir, agent_id, agent_name, kind.as_str(), message, payload, now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(SyncMessage {
            id,
            workdir: workdir.to_owned(),
            agent_id: agent_id.to_owned(),
            agent_name: agent_name.to_owned(),
            kind,
            message: message.to_owned(),
            payload: payload.map(str::to_owned),
            created_at: now,
        })
    }

    pub fn list_sync_messages(&self, workdir: &str, limit: usize) -> Result<Vec<SyncMessage>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, workdir, agent_id, agent_name, kind, message, payload, created_at
             FROM sync_messages WHERE workdir = ?1
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![workdir, limit as i64], |row| {
            let kind_str: String = row.get(4)?;
            Ok(SyncMessage {
                id: row.get(0)?,
                workdir: row.get(1)?,
                agent_id: row.get(2)?,
                agent_name: row.get(3)?,
                kind: MessageKind::from_str(&kind_str).unwrap_or(MessageKind::Info),
                message: row.get(5)?,
                payload: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        let mut msgs: Vec<SyncMessage> = rows.filter_map(|r| r.ok()).collect();
        msgs.reverse();
        Ok(msgs)
    }

    // ── sync_locks ─────────────────────────────────────────────────────

    pub fn insert_sync_lock(&self, lock: &SyncLock) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO sync_locks (id, workdir, agent_id, lock_type, resource, acquired_at, expires_at, released_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                lock.id,
                lock.workdir,
                lock.agent_id,
                lock.lock_type.as_str(),
                lock.resource,
                lock.acquired_at,
                lock.expires_at,
                lock.released_at,
            ],
        )?;
        Ok(())
    }

    pub fn release_sync_lock(&self, lock_id: &str, agent_id: &str) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let now = chrono::Utc::now().timestamp();
        let n = conn.execute(
            "UPDATE sync_locks SET released_at = ?1 WHERE id = ?2 AND agent_id = ?3 AND released_at IS NULL",
            rusqlite::params![now, lock_id, agent_id],
        )?;
        Ok(n > 0)
    }

    pub fn list_active_sync_locks(&self, workdir: &str) -> Result<Vec<SyncLock>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, workdir, agent_id, lock_type, resource, acquired_at, expires_at, released_at
             FROM sync_locks WHERE workdir = ?1 AND released_at IS NULL",
        )?;
        let rows = stmt.query_map(rusqlite::params![workdir], |row| {
            let lt: String = row.get(3)?;
            Ok(SyncLock {
                id: row.get(0)?,
                workdir: row.get(1)?,
                agent_id: row.get(2)?,
                lock_type: LockType::from_str(&lt).unwrap_or(LockType::Resource),
                resource: row.get(4)?,
                acquired_at: row.get(5)?,
                expires_at: row.get(6)?,
                released_at: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Release all locks held by `agent_id` in `workdir`.
    #[allow(dead_code)]
    pub fn release_all_agent_locks(&self, workdir: &str, agent_id: &str) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let now = chrono::Utc::now().timestamp();
        let n = conn.execute(
            "UPDATE sync_locks SET released_at = ?1
             WHERE workdir = ?2 AND agent_id = ?3 AND released_at IS NULL",
            rusqlite::params![now, workdir, agent_id],
        )?;
        Ok(n)
    }
}

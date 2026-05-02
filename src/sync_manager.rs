//! `SyncManager` — in-memory broadcast channel + blocking lock arbitration.
//!
//! One `SyncManager` is shared across all MCP sessions (via `Arc`).
//! It owns:
//! - A `tokio::sync::broadcast` sender per workdir for real-time fan-out.
//! - A `Notify` per pending lock request so waiters wake up when a lock is released.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex, Notify};
use uuid::Uuid;

use crate::db::Database;
use crate::domain::sync::{locks_conflict, LockType, MessageKind, SyncLock, SyncMessage};

const BROADCAST_CAPACITY: usize = 64;
const LOCK_TIMEOUT_SECS: u64 = 300; // 5 min default

/// Shared state per workdir.
struct WorkdirState {
    tx: broadcast::Sender<SyncMessage>,
    /// Notified whenever any lock in this workdir is released.
    lock_released: Arc<Notify>,
}

pub struct SyncManager {
    db: Arc<Database>,
    state: Mutex<HashMap<String, WorkdirState>>,
}

impl SyncManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to the broadcast channel for `workdir`.
    #[allow(dead_code)]
    pub async fn subscribe(&self, workdir: &str) -> broadcast::Receiver<SyncMessage> {
        let mut map = self.state.lock().await;
        map.entry(workdir.to_owned())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
                WorkdirState {
                    tx,
                    lock_released: Arc::new(Notify::new()),
                }
            })
            .tx
            .subscribe()
    }

    /// Broadcast a message to all subscribers of `workdir` and persist it.
    pub async fn broadcast(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        kind: MessageKind,
        message: &str,
        payload: Option<&str>,
    ) -> anyhow::Result<SyncMessage> {
        let msg = self
            .db
            .insert_sync_message(workdir, agent_id, agent_name, kind, message, payload)?;

        let map = self.state.lock().await;
        if let Some(ws) = map.get(workdir) {
            let _ = ws.tx.send(msg.clone());
        }
        Ok(msg)
    }

    /// Acquire a lock — **blocks** until the lock is free or timeout expires.
    ///
    /// Returns the `lock_id` on success.
    pub async fn acquire_lock(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        lock_type: LockType,
        resource: &str,
        timeout_secs: Option<u64>,
    ) -> anyhow::Result<String> {
        let timeout = timeout_secs.unwrap_or(LOCK_TIMEOUT_SECS);
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(if timeout == 0 { u64::MAX / 2 } else { timeout });

        loop {
            // Snapshot active locks from DB (single writer, Mutex-protected).
            let active = self.db.list_active_sync_locks(workdir)?;

            let candidate = SyncLock {
                id: String::new(), // placeholder
                workdir: workdir.to_owned(),
                agent_id: agent_id.to_owned(),
                lock_type,
                resource: resource.to_owned(),
                acquired_at: 0,
                expires_at: None,
                released_at: None,
            };

            let blocked_by: Vec<&SyncLock> = active
                .iter()
                .filter(|l| l.agent_id != agent_id && locks_conflict(l, &candidate))
                .collect();

            if blocked_by.is_empty() {
                // Free — acquire.
                let lock_id = Uuid::new_v4().to_string();
                let now = chrono::Utc::now().timestamp();
                let lock = SyncLock {
                    id: lock_id.clone(),
                    workdir: workdir.to_owned(),
                    agent_id: agent_id.to_owned(),
                    lock_type,
                    resource: resource.to_owned(),
                    acquired_at: now,
                    expires_at: None,
                    released_at: None,
                };
                self.db.insert_sync_lock(&lock)?;

                let payload =
                    serde_json::json!({ "lock_id": lock_id, "resource": resource }).to_string();
                self.broadcast(
                    workdir,
                    agent_id,
                    agent_name,
                    MessageKind::LockAcquired,
                    &format!("{agent_name} acquired lock on '{resource}'"),
                    Some(&payload),
                )
                .await?;

                return Ok(lock_id);
            }

            // Blocked — emit waiting message once, then wait for a release notification.
            let holder = &blocked_by[0].agent_id;
            let payload =
                serde_json::json!({ "resource": resource, "blocked_by": holder }).to_string();
            self.broadcast(
                workdir,
                agent_id,
                agent_name,
                MessageKind::Waiting,
                &format!("{agent_name} waiting for lock on '{resource}' (held by {holder})"),
                Some(&payload),
            )
            .await?;

            // Get the Notify handle before releasing the state lock.
            let notify = {
                let mut map = self.state.lock().await;
                let ws = map.entry(workdir.to_owned()).or_insert_with(|| {
                    let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
                    WorkdirState {
                        tx,
                        lock_released: Arc::new(Notify::new()),
                    }
                });
                Arc::clone(&ws.lock_released)
            };

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("Timeout waiting for lock on '{resource}'");
            }

            // Wait for any release in this workdir (or timeout).
            tokio::select! {
                _ = notify.notified() => {}
                _ = tokio::time::sleep(remaining) => {
                    anyhow::bail!("Timeout waiting for lock on '{resource}'");
                }
            }
        }
    }

    /// Release a lock by `lock_id`.
    pub async fn release_lock(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        lock_id: &str,
        resource: &str,
    ) -> anyhow::Result<bool> {
        let released = self.db.release_sync_lock(lock_id, agent_id)?;
        if released {
            let payload =
                serde_json::json!({ "lock_id": lock_id, "resource": resource }).to_string();
            self.broadcast(
                workdir,
                agent_id,
                agent_name,
                MessageKind::Release,
                &format!("{agent_name} released lock on '{resource}'"),
                Some(&payload),
            )
            .await?;
            self.notify_lock_released(workdir).await;
        }
        Ok(released)
    }

    /// Notify all waiters in `workdir` that a lock was released.
    async fn notify_lock_released(&self, workdir: &str) {
        let map = self.state.lock().await;
        if let Some(ws) = map.get(workdir) {
            ws.lock_released.notify_waiters();
        }
    }

    /// Release all locks for `agent_id` in `workdir` (called on agent exit).
    #[allow(dead_code)]
    pub async fn release_all_for_agent(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
    ) -> anyhow::Result<()> {
        let n = self.db.release_all_agent_locks(workdir, agent_id)?;
        if n > 0 {
            self.broadcast(
                workdir,
                agent_id,
                agent_name,
                MessageKind::Release,
                &format!("{agent_name} released all locks ({n} total)"),
                None,
            )
            .await?;
            self.notify_lock_released(workdir).await;
        }
        Ok(())
    }
}

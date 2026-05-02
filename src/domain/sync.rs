//! Collaborative sync domain models.
//!
//! `SyncMessage` — channel entry persisted in SQLite and broadcast in-memory.
//! `SyncLock`    — active lock record persisted in SQLite.

use serde::{Deserialize, Serialize};

/// Kind of message in the sync channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Intent,
    LockAcquired,
    Waiting,
    Release,
    Info,
    Query,
    Answer,
    Status,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::LockAcquired => "lock_acquired",
            Self::Waiting => "waiting",
            Self::Release => "release",
            Self::Info => "info",
            Self::Query => "query",
            Self::Answer => "answer",
            Self::Status => "status",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "intent" => Some(Self::Intent),
            "lock_acquired" => Some(Self::LockAcquired),
            "waiting" => Some(Self::Waiting),
            "release" => Some(Self::Release),
            "info" => Some(Self::Info),
            "query" => Some(Self::Query),
            "answer" => Some(Self::Answer),
            "status" => Some(Self::Status),
            _ => None,
        }
    }
}

/// Type of lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockType {
    /// Lock on a file-system path or module.
    Resource,
    /// Lock on a mutually-exclusive command (e.g. `cargo test`).
    Command,
}

impl LockType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resource => "resource",
            Self::Command => "command",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "resource" => Some(Self::Resource),
            "command" => Some(Self::Command),
            _ => None,
        }
    }
}

/// A message in the per-workdir sync channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMessage {
    pub id: i64,
    pub workdir: String,
    pub agent_id: String,
    pub agent_name: String,
    pub kind: MessageKind,
    pub message: String,
    /// Optional JSON payload (lock_id, resource, …).
    pub payload: Option<String>,
    pub created_at: i64,
}

/// An active (or released) lock record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncLock {
    pub id: String,
    pub workdir: String,
    pub agent_id: String,
    pub lock_type: LockType,
    /// Path for resource locks; command string for command locks.
    pub resource: String,
    pub acquired_at: i64,
    pub expires_at: Option<i64>,
    pub released_at: Option<i64>,
}

impl SyncLock {
    pub fn is_active(&self) -> bool {
        self.released_at.is_none()
    }
}

/// Returns `true` when `a` and `b` conflict (one must wait for the other).
///
/// Conflict rules:
/// - Two resource locks conflict when one path is a prefix of the other.
/// - Two command locks always conflict (exclusive commands).
/// - Resource vs command never conflict.
pub fn locks_conflict(a: &SyncLock, b: &SyncLock) -> bool {
    if !a.is_active() || !b.is_active() {
        return false;
    }
    match (a.lock_type, b.lock_type) {
        (LockType::Resource, LockType::Resource) => paths_overlap(&a.resource, &b.resource),
        (LockType::Command, LockType::Command) => true,
        _ => false,
    }
}

fn paths_overlap(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    // a is prefix of b or b is prefix of a
    b.starts_with(a) || a.starts_with(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(resource: &str, lock_type: LockType) -> SyncLock {
        SyncLock {
            id: "x".into(),
            workdir: "/w".into(),
            agent_id: "a".into(),
            lock_type,
            resource: resource.into(),
            acquired_at: 0,
            expires_at: None,
            released_at: None,
        }
    }

    #[test]
    fn resource_locks_on_distinct_paths_do_not_conflict() {
        let a = lock("src/auth", LockType::Resource);
        let b = lock("src/payments", LockType::Resource);
        assert!(!locks_conflict(&a, &b));
    }

    #[test]
    fn resource_lock_conflicts_with_child_path() {
        let a = lock("src/auth", LockType::Resource);
        let b = lock("src/auth/session.rs", LockType::Resource);
        assert!(locks_conflict(&a, &b));
    }

    #[test]
    fn resource_lock_conflicts_with_same_path() {
        let a = lock("src/auth", LockType::Resource);
        let b = lock("src/auth", LockType::Resource);
        assert!(locks_conflict(&a, &b));
    }

    #[test]
    fn command_locks_always_conflict() {
        let a = lock("cargo test", LockType::Command);
        let b = lock("cargo test", LockType::Command);
        assert!(locks_conflict(&a, &b));
    }

    #[test]
    fn resource_and_command_do_not_conflict() {
        let a = lock("src/auth", LockType::Resource);
        let b = lock("cargo test", LockType::Command);
        assert!(!locks_conflict(&a, &b));
    }

    #[test]
    fn released_lock_does_not_conflict() {
        let a = lock("src/auth", LockType::Resource);
        let mut b = lock("src/auth", LockType::Resource);
        b.released_at = Some(1);
        assert!(!locks_conflict(&a, &b));
    }
}

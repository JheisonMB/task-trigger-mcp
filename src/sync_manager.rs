//! `SyncManager` — in-memory fan-out plus advisory sync context.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

use crate::db::Database;
use crate::domain::sync::{
    summarize_sync_context, IntentPayload, MessageKind, MissionImpact, StatusPayload,
    SyncContextSnapshot, SyncMessage, WorkspaceStatus,
};

const BROADCAST_CAPACITY: usize = 64;
const CONTEXT_WINDOW: usize = 100;

struct WorkdirState {
    tx: broadcast::Sender<SyncMessage>,
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

    #[allow(dead_code)]
    pub async fn subscribe(&self, workdir: &str) -> broadcast::Receiver<SyncMessage> {
        self.ensure_sender(workdir).await.subscribe()
    }

    pub async fn declare_intent(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        mission: &str,
        impact: MissionImpact,
        description: &str,
    ) -> anyhow::Result<SyncMessage> {
        let payload = serde_json::to_string(&IntentPayload {
            mission: mission.to_owned(),
            impact,
            description: description.to_owned(),
        })?;

        self.publish(
            workdir,
            agent_id,
            agent_name,
            MessageKind::Intent,
            &format!("{agent_name}: {mission}"),
            Some(&payload),
        )
        .await
    }

    pub async fn report_status(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        status: WorkspaceStatus,
        message: &str,
    ) -> anyhow::Result<SyncMessage> {
        let payload = serde_json::to_string(&StatusPayload {
            status,
            message: message.to_owned(),
        })?;

        self.publish(
            workdir,
            agent_id,
            agent_name,
            MessageKind::Status,
            message,
            Some(&payload),
        )
        .await
    }

    pub async fn broadcast(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        kind: MessageKind,
        message: &str,
        payload: Option<&str>,
    ) -> anyhow::Result<SyncMessage> {
        self.publish(workdir, agent_id, agent_name, kind, message, payload)
            .await
    }

    pub fn get_context(
        &self,
        workdir: &str,
        chatter_limit: usize,
    ) -> anyhow::Result<SyncContextSnapshot> {
        let recent_messages = self.db.list_sync_messages(workdir, CONTEXT_WINDOW)?;
        let active_agent_ids: HashSet<String> = self
            .db
            .list_active_sync_agent_ids(workdir)?
            .into_iter()
            .collect();

        Ok(summarize_sync_context(
            &recent_messages,
            &active_agent_ids,
            chatter_limit,
        ))
    }

    async fn publish(
        &self,
        workdir: &str,
        agent_id: &str,
        agent_name: &str,
        kind: MessageKind,
        message: &str,
        payload: Option<&str>,
    ) -> anyhow::Result<SyncMessage> {
        let sync_message = self
            .db
            .insert_sync_message(workdir, agent_id, agent_name, kind, message, payload)?;
        let sender = self.ensure_sender(workdir).await;
        let _ = sender.send(sync_message.clone());
        Ok(sync_message)
    }

    async fn ensure_sender(&self, workdir: &str) -> broadcast::Sender<SyncMessage> {
        let mut state = self.state.lock().await;
        state
            .entry(workdir.to_owned())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
                WorkdirState { tx }
            })
            .tx
            .clone()
    }
}

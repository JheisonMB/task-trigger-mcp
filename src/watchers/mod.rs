//! File watcher engine using the `notify` crate.
//!
//! Manages filesystem watchers that trigger CLI executions when
//! specified events occur. Watchers survive agent disconnection
//! and are reloaded from `SQLite` on daemon startup.

use anyhow::Result;
use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::application::ports::AgentRepository;
use crate::db::Database;
use crate::domain::models::{Agent, Trigger, WatchEvent};
use crate::executor::Executor;

/// Manages all active file system watchers.
pub struct WatcherEngine {
    db: Arc<Database>,
    executor: Arc<Executor>,
    /// Active notify watchers keyed by agent ID.
    active: Arc<Mutex<HashMap<String, ActiveWatcher>>>,
}

struct ActiveWatcher {
    /// The notify watcher handle — dropping this stops the watcher.
    _watcher: RecommendedWatcher,
    /// Agent config from database.
    #[allow(dead_code)]
    agent: Agent,
}

impl WatcherEngine {
    pub fn new(db: Arc<Database>, executor: Arc<Executor>) -> Self {
        Self {
            db,
            executor,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Load and start all enabled watch agents from the database.
    pub async fn reload_from_db(&self) -> Result<()> {
        let agents = self.db.list_watch_agents()?;
        tracing::info!(
            "Reloading {} enabled watch agents from database",
            agents.len()
        );

        for agent in agents {
            if let Err(e) = self.start_watcher(&agent).await {
                tracing::error!("Failed to start watcher for agent '{}': {}", agent.id, e);
            }
        }
        Ok(())
    }

    /// Start watching for a specific agent configuration.
    pub async fn start_watcher(&self, agent: &Agent) -> Result<()> {
        let Trigger::Watch {
            path,
            events,
            debounce_seconds,
            recursive,
        } = agent.trigger.as_ref().ok_or_else(|| anyhow::anyhow!("Agent '{}' has no Watch trigger", agent.id))?
        else {
            return Err(anyhow::anyhow!("Agent '{}' trigger is not Watch", agent.id));
        };

        let id = agent.id.clone();
        let path = path.clone();
        let events = events.clone();
        let debounce_secs = *debounce_seconds;
        let recursive = *recursive;

        // On macOS, FSEvents works at directory level. If watching a single file,
        // watch the parent directory and filter events by filename.
        // Also handles non-existent files by watching the parent directory.
        let watch_path_buf = std::path::PathBuf::from(&path);
        let (actual_watch_path, file_filter) = if watch_path_buf.is_file()
            || (!watch_path_buf.exists()
                && watch_path_buf.extension().is_some()
                && watch_path_buf.parent().map(|p| p.is_dir()).unwrap_or(false))
        {
            let parent = watch_path_buf
                .parent()
                .unwrap_or(&watch_path_buf)
                .to_path_buf();
            let filename = watch_path_buf
                .file_name()
                .map(|f| f.to_string_lossy().to_string());
            (parent, filename)
        } else {
            (watch_path_buf.clone(), None)
        };

        // Clone what we need for the event handler closure
        let db = Arc::clone(&self.db);
        let executor = Arc::clone(&self.executor);
        let agent_clone = agent.clone();
        let last_trigger: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        // Create the notify watcher with event handler
        let notify_watcher = {
            let id = id.clone();
            let events = events.clone();
            let last_trigger = Arc::clone(&last_trigger);
            let db = Arc::clone(&db);
            let executor = Arc::clone(&executor);
            let agent_for_handler = agent_clone.clone();
            let file_filter = file_filter.clone();

            let rt = tokio::runtime::Handle::current();

            RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    let event = match res {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::error!("Watcher '{}' error: {}", id, e);
                            return;
                        }
                    };

                    if let Some(ref filter) = file_filter {
                        let matches = event.paths.iter().any(|p| {
                            p.file_name()
                                .map(|f| f.to_string_lossy() == *filter)
                                .unwrap_or(false)
                        });
                        if !matches {
                            return;
                        }
                    }

                    let our_event = match event.kind {
                        EventKind::Create(_) => Some(WatchEvent::Create),
                        EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
                            Some(WatchEvent::Move)
                        }
                        EventKind::Modify(_) => Some(WatchEvent::Modify),
                        EventKind::Remove(_) => Some(WatchEvent::Delete),
                        _ => {
                            tracing::debug!(
                                "Watcher '{}' ignoring event kind: {:?}",
                                id,
                                event.kind
                            );
                            None
                        }
                    };

                    let Some(evt) = our_event else { return };

                    let matched = events.contains(&evt)
                        || (evt == WatchEvent::Modify
                            && events.contains(&WatchEvent::Create));
                    if !matched {
                        return;
                    }

                    let last_trigger = Arc::clone(&last_trigger);
                    let _db = Arc::clone(&db);
                    let executor = Arc::clone(&executor);
                    let agent = agent_for_handler.clone();
                    let event_paths = event.paths;
                    let evt_str = evt.to_string();

                    rt.spawn(async move {
                        {
                            let mut lt = last_trigger.lock().await;
                            if let Some(last) = *lt {
                                if last.elapsed() < Duration::from_secs(debounce_secs) {
                                    return;
                                }
                            }
                            *lt = Some(Instant::now());
                        }

                        let file_path = event_paths
                            .first()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();

                        tracing::info!(
                            "Watcher '{}' triggered: {} on {}",
                            agent.id,
                            evt_str,
                            file_path
                        );

                        if let Err(e) = executor
                            .execute_agent_with_context(&agent, &file_path, &evt_str)
                            .await
                        {
                            tracing::error!(
                                "Watcher '{}' execution failed: {}",
                                agent.id,
                                e
                            );
                        }
                    });
                },
                Config::default(),
            )?
        };

        // Register the path with the watcher
        let mut watcher = notify_watcher;

        let watch_path = &actual_watch_path;
        // When watching a single file, always use NonRecursive on the parent dir
        let mode = if file_filter.is_some() {
            RecursiveMode::NonRecursive
        } else if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        if !watch_path.exists() {
            if file_filter.is_some() {
                tracing::info!(
                    "Watcher '{}': file '{}' does not exist yet, watching parent dir for creation",
                    id,
                    path
                );
            } else {
                tracing::warn!(
                    "Watcher '{}': path '{}' does not exist, watcher will activate when it's created",
                    id,
                    path
                );
            }
        }

        watcher.watch(watch_path, mode)?;

        if file_filter.is_some() {
            tracing::info!(
                "Started watcher '{}' on file '{}' (via parent dir '{}', events: {:?})",
                id,
                path,
                watch_path.display(),
                events
            );
        } else {
            tracing::info!(
                "Started watcher '{}' on '{}' (events: {:?}, recursive: {})",
                id,
                path,
                events,
                recursive
            );
        }

        // Store the active watcher
        let mut active = self.active.lock().await;
        active.insert(
            id,
            ActiveWatcher {
                _watcher: watcher,
                agent: agent_clone,
            },
        );

        Ok(())
    }

    /// Stop a specific watcher by ID (dropping the handle stops it).
    pub async fn stop_watcher(&self, id: &str) -> Result<()> {
        let mut active = self.active.lock().await;
        if active.remove(id).is_some() {
            tracing::info!("Stopped watcher '{}'", id);
        }
        Ok(())
    }

    /// Stop all active watchers.
    pub async fn stop_all(&self) {
        let mut active = self.active.lock().await;
        let count = active.len();
        active.clear();
        tracing::info!("Stopped {} watchers", count);
    }

    /// Get the count of active watchers.
    pub async fn active_count(&self) -> usize {
        self.active.lock().await.len()
    }

    /// Check if a specific watcher is active.
    pub async fn is_active(&self, id: &str) -> bool {
        self.active.lock().await.contains_key(id)
    }
}
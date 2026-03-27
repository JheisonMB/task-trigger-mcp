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

use crate::application::ports::WatcherRepository;
use crate::db::Database;
use crate::executor::Executor;
use crate::domain::models::{WatchEvent, Watcher};

/// Manages all active file system watchers.
pub struct WatcherEngine {
    db: Arc<Database>,
    executor: Arc<Executor>,
    /// Active notify watchers keyed by watcher ID.
    active: Arc<Mutex<HashMap<String, ActiveWatcher>>>,
}

#[allow(dead_code)]
struct ActiveWatcher {
    /// The notify watcher handle — dropping this stops the watcher.
    _watcher: RecommendedWatcher,
    /// Watcher config from database.
    config: Watcher,
}

impl WatcherEngine {
    pub fn new(db: Arc<Database>, executor: Arc<Executor>) -> Self {
        Self {
            db,
            executor,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Load and start all enabled watchers from the database.
    pub async fn reload_from_db(&self) -> Result<()> {
        let watchers = self.db.list_enabled_watchers()?;
        tracing::info!("Reloading {} enabled watchers from database", watchers.len());

        for w in watchers {
            if let Err(e) = self.start_watcher(w).await {
                tracing::error!("Failed to start watcher: {}", e);
            }
        }
        Ok(())
    }

    /// Start watching for a specific watcher configuration.
    pub async fn start_watcher(&self, watcher_config: Watcher) -> Result<()> {
        let id = watcher_config.id.clone();
        let path = watcher_config.path.clone();
        let events = watcher_config.events.clone();
        let debounce_secs = watcher_config.debounce_seconds;
        let recursive = watcher_config.recursive;

        // On macOS, FSEvents works at directory level. If watching a single file,
        // watch the parent directory and filter events by filename.
        // Also handles non-existent files by watching the parent directory.
        let watch_path_buf = std::path::PathBuf::from(&path);
        let (actual_watch_path, file_filter) = if watch_path_buf.is_file()
            || (!watch_path_buf.exists()
                && watch_path_buf.extension().is_some()
                && watch_path_buf
                    .parent()
                    .map(|p| p.is_dir())
                    .unwrap_or(false))
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
        let watcher_config_for_handler = watcher_config.clone();
        let last_trigger: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        // Create the notify watcher with event handler
        let notify_watcher = {
            let id = id.clone();
            let events = events.clone();
            let last_trigger = Arc::clone(&last_trigger);
            let db = Arc::clone(&db);
            let executor = Arc::clone(&executor);
            let watcher_config = watcher_config_for_handler.clone();
            let file_filter = file_filter.clone();

            let rt = tokio::runtime::Handle::current();

            RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    match res {
                        Ok(event) => {
                            // If watching a single file, filter by filename
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

                            // Map notify event kind to our WatchEvent type
                            let our_event = match event.kind {
                                EventKind::Create(_) => Some(WatchEvent::Create),
                                EventKind::Modify(notify::event::ModifyKind::Name(_)) => Some(WatchEvent::Move),
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

                            // On macOS, FSEvents may report file creation as Modify.
                            // Also try matching Access events that some platforms emit.

                            // Check if this event type is in our watched events.
                            // On macOS, create events often arrive as modify — if the
                            // user watches for "create", also accept modify events.
                            if let Some(evt) = our_event {
                                let matched = events.contains(&evt)
                                    || (evt == WatchEvent::Modify && events.contains(&WatchEvent::Create));
                                if !matched {
                                    return;
                                }

                                // Debounce check
                                let last_trigger = Arc::clone(&last_trigger);
                                let _db = Arc::clone(&db);
                                let executor = Arc::clone(&executor);
                                let watcher_config = watcher_config.clone();
                                let event_paths = event.paths;
                                let evt_str = evt.to_string();

                                rt.spawn(async move {
                                    // Debounce
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
                                        watcher_config.id,
                                        evt_str,
                                        file_path
                                    );

                                    if let Err(e) = executor
                                        .execute_watcher_task(
                                            &watcher_config,
                                            &file_path,
                                            &evt_str,
                                        )
                                        .await
                                    {
                                        tracing::error!(
                                            "Watcher '{}' execution failed: {}",
                                            watcher_config.id,
                                            e
                                        );
                                    }
                                });
                            }
                        }
                        Err(e) => {
                            tracing::error!("Watcher '{}' error: {}", id, e);
                        }
                    }
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
                config: watcher_config,
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

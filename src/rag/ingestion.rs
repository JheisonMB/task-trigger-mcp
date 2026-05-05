#![allow(dead_code)]
//! `IngestionManager` — async queue + background worker for RAG indexing.
//!
//! Uses SQLite FTS5 as the search index (bundled, zero extra deps).
//! Embedding is deferred to a future spec iteration; for now chunks are
//! stored as plain text and searched via FTS5 keyword matching.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::application::ports::StateRepository;
use crate::db::project::Chunk;
use crate::db::Database;
use crate::domain::project::Project;
use crate::rag::chunker::{chunk, detect_lang};

const QUEUE_MAX: usize = 10_000;
const FILE_MAX_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

struct Queue {
    /// Ordered list of (project_hash, source_path) to process.
    order: VecDeque<(String, String)>,
    /// Set for dedup — if already queued, move to end.
    set: HashSet<(String, String)>,
}

impl Queue {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    fn push(&mut self, project_hash: &str, path: &str) -> bool {
        let key = (project_hash.to_owned(), path.to_owned());
        if self.set.contains(&key) {
            self.order.retain(|k| k != &key);
        }
        if self.order.len() >= QUEUE_MAX {
            return false;
        }
        self.order.push_back(key.clone());
        self.set.insert(key);
        true
    }

    fn pop(&mut self) -> Option<(String, String)> {
        let item = self.order.pop_front()?;
        self.set.remove(&item);
        Some(item)
    }
}

pub struct IngestionManager {
    db: Arc<Database>,
    data_dir: std::path::PathBuf,
    queue: Arc<Mutex<Queue>>,
    notify: Arc<Notify>,
}

impl IngestionManager {
    pub fn new(db: Arc<Database>, data_dir: std::path::PathBuf) -> Self {
        crate::rag::ragignore::ensure_ragignore(&data_dir);
        Self {
            db,
            data_dir,
            queue: Arc::new(Mutex::new(Queue::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Register a project path and enqueue all its files into the in-memory worker.
    pub async fn register_and_enqueue(&self, path: &std::path::Path) {
        let project = match self.db.register_project_path(path) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("RAG register_and_enqueue failed for {:?}: {e}", path);
                return;
            }
        };
        // Merge project's .gitignore into global ragignore
        crate::rag::ragignore::merge_gitignore(&self.data_dir, path);
        self.enqueue_project(&project).await;
    }

    /// Enqueue a file for (re)indexing. Returns false if queue is full.
    pub async fn enqueue(&self, source_path: &str) -> bool {
        let mut q = self.queue.lock().await;
        let ok = q.push("", source_path);
        if ok {
            let now = chrono::Utc::now().timestamp();
            if let Err(e) = self.db.enqueue_rag_item(source_path, now) {
                tracing::warn!("RAG queue state error {source_path}: {e}");
            }
            self.notify.notify_one();
        }
        ok
    }

    /// Queue size snapshot.
    pub async fn queue_len(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Enqueue all supported files under `root` for a project.
    pub async fn enqueue_project(&self, project: &Project) {
        let root = std::path::Path::new(&project.path);
        let patterns = crate::rag::ragignore::load_patterns(&self.data_dir);

        let walker = walkdir::WalkDir::new(root)
            .max_depth(10)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| !crate::rag::ragignore::is_ignored(e.path(), root, &patterns));

        for entry in walker {
            let path = entry.path().to_string_lossy().to_string();
            if detect_lang(&path).is_some() {
                self.enqueue(&path).await;
            }
        }
    }

    /// Start the background worker. Returns a handle to cancel it.
    pub fn start(self: Arc<Self>) -> tokio_util::sync::CancellationToken {
        let ct = tokio_util::sync::CancellationToken::new();
        let ct_child = ct.child_token();
        tokio::spawn(async move {
            self.run(ct_child).await;
        });
        ct
    }

    async fn run(&self, ct: tokio_util::sync::CancellationToken) {
        loop {
            tokio::select! {
                _ = ct.cancelled() => break,
                _ = self.notify.notified() => {
                    // Debounce: wait 3 s for burst to settle
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    self.drain_queue(&ct).await;
                }
            }
        }
    }

    fn is_paused(&self) -> bool {
        self.db.get_state("rag_paused").ok().flatten().as_deref() == Some("1")
    }

    async fn drain_queue(&self, ct: &tokio_util::sync::CancellationToken) {
        let mut processed = 0usize;

        loop {
            if !self.wait_while_paused(ct).await {
                return;
            }

            let Some((_ignored, source_path)) = self.queue.lock().await.pop() else {
                break;
            };

            let now = chrono::Utc::now().timestamp();
            let _ = self.db.mark_rag_item_processing(&source_path, now);

            if let Err(e) = self.index_file(&source_path).await {
                tracing::warn!("RAG index error {source_path}: {e}");
            } else {
                processed += 1;
            }
            let _ = self.db.remove_rag_item(&source_path);
        }

        if processed > 0 {
            tracing::info!("RAG: indexed {processed} file(s)");
        }
    }

    /// Spin-wait while RAG is paused. Returns `false` if cancelled.
    async fn wait_while_paused(&self, ct: &tokio_util::sync::CancellationToken) -> bool {
        while self.is_paused() {
            tokio::select! {
                _ = ct.cancelled() => return false,
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
        !ct.is_cancelled()
    }

    async fn index_file(&self, source_path: &str) -> anyhow::Result<()> {
        let path = std::path::Path::new(source_path);

        if !path.exists() {
            self.db.replace_chunks(source_path, &[])?;
            return Ok(());
        }

        let meta = std::fs::metadata(path)?;
        if meta.len() > FILE_MAX_BYTES {
            tracing::debug!("RAG: skipping large file {source_path}");
            return Ok(());
        }

        let Some(lang) = detect_lang(source_path) else {
            return Ok(());
        };

        let content = std::fs::read_to_string(path)?;
        let now = chrono::Utc::now().timestamp();

        let chunks: Vec<Chunk> = chunk(&content, lang)
            .into_iter()
            .map(|(i, text)| Chunk {
                id: Uuid::new_v4().to_string(),
                project_hash: None,
                source_path: source_path.to_owned(),
                chunk_index: i as i32,
                content: text,
                lang: lang.to_owned(),
                updated_at: now,
            })
            .collect();

        self.db.replace_chunks(source_path, &chunks)?;
        Ok(())
    }
}

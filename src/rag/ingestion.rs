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
    queue: Arc<Mutex<Queue>>,
    notify: Arc<Notify>,
}

impl IngestionManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            queue: Arc::new(Mutex::new(Queue::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Enqueue a file for (re)indexing. Returns false if queue is full.
    pub async fn enqueue(&self, project_hash: &str, source_path: &str) -> bool {
        let mut q = self.queue.lock().await;
        let ok = q.push(project_hash, source_path);
        if ok {
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
        let walker = walkdir::WalkDir::new(root)
            .max_depth(10)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file());

        for entry in walker {
            let path = entry.path().to_string_lossy().to_string();
            if detect_lang(&path).is_some() {
                self.enqueue(&project.hash, &path).await;
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
                    self.drain_queue().await;
                }
            }
        }
    }

    async fn drain_queue(&self) {
        let mut processed = 0usize;
        loop {
            let item = {
                let mut q = self.queue.lock().await;
                q.pop()
            };
            let Some((project_hash, source_path)) = item else {
                break;
            };
            if let Err(e) = self.index_file(&project_hash, &source_path).await {
                tracing::warn!("RAG index error {source_path}: {e}");
            } else {
                processed += 1;
            }
        }
        if processed > 0 {
            tracing::info!("RAG: indexed {processed} file(s)");
        }
    }

    async fn index_file(&self, project_hash: &str, source_path: &str) -> anyhow::Result<()> {
        let path = std::path::Path::new(source_path);

        // File deleted → just remove chunks
        if !path.exists() {
            self.db.replace_chunks(project_hash, source_path, &[])?;
            return Ok(());
        }

        // Size guard
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
                project_hash: project_hash.to_owned(),
                source_path: source_path.to_owned(),
                chunk_index: i as i32,
                content: text,
                lang: lang.to_owned(),
                updated_at: now,
            })
            .collect();

        self.db.replace_chunks(project_hash, source_path, &chunks)?;
        Ok(())
    }
}

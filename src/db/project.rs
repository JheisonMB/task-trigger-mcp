#![allow(dead_code)]
//! SQLite repositories for projects and RAG chunks (FTS5).

use anyhow::Result;
use std::path::Path;

use crate::db::Database;
use crate::domain::project::{extract_readme_description, Project};

// ── Chunk record (stored in FTS5 + metadata table) ─────────────────────

#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub project_hash: String,
    pub source_path: String,
    pub chunk_index: i32,
    pub content: String,
    pub lang: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct RagQueueItem {
    pub project_hash: String,
    pub project_name: Option<String>,
    pub source_path: String,
    pub status: String,
    pub queued_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct RagInfoSummary {
    pub total_chunks: i64,
    pub indexed_projects: i64,
    pub queued_items: i64,
    pub processing_items: i64,
}

impl Database {
    // ── projects ───────────────────────────────────────────────────────

    pub fn upsert_project(&self, p: &Project) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO projects (hash, path, name, description, tags, indexed_at, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(hash) DO UPDATE SET
               path=excluded.path,
                name=excluded.name,
                description=COALESCE(description, excluded.description),
                tags=COALESCE(tags, excluded.tags),
                indexed_at=COALESCE(projects.indexed_at, excluded.indexed_at)",
            rusqlite::params![
                p.hash,
                p.path,
                p.name,
                p.description,
                p.tags,
                p.indexed_at,
                p.created_at
            ],
        )?;
        Ok(())
    }

    pub fn register_project_path(&self, path: &Path) -> Result<Project> {
        let canonical = std::fs::canonicalize(path)?;
        let canonical_str = canonical.to_string_lossy().to_string();
        let mut project = Project::new(&canonical_str);

        let readme_path = canonical.join("README.md");
        if readme_path.exists() {
            let readme = std::fs::read_to_string(&readme_path)?;
            project.description = extract_readme_description(&readme);
        }

        // Create .canopy/ragignore seeded from .gitignore if not present.
        crate::rag::ragignore::ensure_ragignore(&canonical);

        self.upsert_project(&project)?;
        self.queue_project_files(&project.hash, &canonical)?;
        Ok(project)
    }

    /// Scan project directory and queue all discoverable files for RAG ingestion.
    /// Max depth: 10 levels. Max file size: 5 MB.
    fn queue_project_files(&self, project_hash: &str, root: &Path) -> Result<()> {
        const MAX_DEPTH: usize = 10;
        const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MB
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        let patterns = crate::rag::ragignore::load_patterns(root);

        let mut queue = vec![(root.to_path_buf(), 0)];
        while let Some((current_path, depth)) = queue.pop() {
            if depth > MAX_DEPTH {
                continue;
            }

            if let Ok(entries) = std::fs::read_dir(&current_path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if crate::rag::ragignore::is_ignored(&entry_path, root, &patterns) {
                        continue;
                    }
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_dir() {
                            queue.push((entry_path, depth + 1));
                        } else if metadata.len() <= MAX_FILE_SIZE {
                            if let Some(file_path_str) = entry_path.to_str() {
                                if is_indexable_path(file_path_str) {
                                    let _ = self.enqueue_rag_item(project_hash, file_path_str, now);
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn delete_project(&self, hash: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "DELETE FROM rag_queue WHERE project_hash=?1",
            rusqlite::params![hash],
        )?;
        conn.execute(
            "DELETE FROM rag_chunks WHERE project_hash=?1",
            rusqlite::params![hash],
        )?;
        conn.execute(
            "DELETE FROM projects WHERE hash=?1",
            rusqlite::params![hash],
        )?;
        Ok(())
    }

    /// Remove all entries from the RAG queue (used on daemon startup to purge
    /// stale entries queued without the language filter in older versions).
    pub fn clear_rag_queue(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute("DELETE FROM rag_queue", [])?;
        Ok(())
    }

    pub fn get_project(&self, hash: &str) -> Result<Option<Project>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT hash,path,name,description,tags,indexed_at,created_at
             FROM projects WHERE hash=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![hash], row_to_project)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_project_by_path(&self, path: &Path) -> Result<Option<Project>> {
        let canonical = std::fs::canonicalize(path)?;
        let canonical_str = canonical.to_string_lossy().to_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT hash,path,name,description,tags,indexed_at,created_at
             FROM projects WHERE path=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![canonical_str], row_to_project)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT hash,path,name,description,tags,indexed_at,created_at
             FROM projects ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Full-text search over name + description.
    pub fn search_projects(&self, query: &str) -> Result<Vec<Project>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        // Simple LIKE search — FTS5 is on chunks, not projects (small table)
        let pattern = format!("%{}%", query.to_lowercase());
        let mut stmt = conn.prepare(
            "SELECT hash,path,name,description,tags,indexed_at,created_at FROM projects
             WHERE lower(name) LIKE ?1 OR lower(description) LIKE ?1
             ORDER BY created_at DESC LIMIT 20",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern], row_to_project)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn update_project_meta(
        &self,
        hash: &str,
        description: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let tags_str = tags.map(|t| t.join(","));
        let n = conn.execute(
            "UPDATE projects SET
               description = COALESCE(?2, description),
               tags        = COALESCE(?3, tags)
             WHERE hash = ?1",
            rusqlite::params![hash, description, tags_str],
        )?;
        Ok(n > 0)
    }

    pub fn mark_project_indexed(&self, hash: &str, indexed_at: i64) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let n = conn.execute(
            "UPDATE projects SET indexed_at = ?2 WHERE hash = ?1",
            rusqlite::params![hash, indexed_at],
        )?;
        Ok(n > 0)
    }

    pub fn enqueue_rag_item(
        &self,
        project_hash: &str,
        source_path: &str,
        queued_at: i64,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO rag_queue (project_hash, source_path, status, queued_at, updated_at)
             VALUES (?1, ?2, 'queued', ?3, ?3)
             ON CONFLICT(project_hash, source_path) DO UPDATE SET
                status='queued',
                queued_at=excluded.queued_at,
                updated_at=excluded.updated_at",
            rusqlite::params![project_hash, source_path, queued_at],
        )?;
        Ok(())
    }

    pub fn mark_rag_item_processing(
        &self,
        project_hash: &str,
        source_path: &str,
        updated_at: i64,
    ) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let n = conn.execute(
            "UPDATE rag_queue
             SET status='processing', updated_at=?3
             WHERE project_hash=?1 AND source_path=?2",
            rusqlite::params![project_hash, source_path, updated_at],
        )?;
        Ok(n > 0)
    }

    pub fn remove_rag_item(&self, project_hash: &str, source_path: &str) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let n = conn.execute(
            "DELETE FROM rag_queue WHERE project_hash=?1 AND source_path=?2",
            rusqlite::params![project_hash, source_path],
        )?;
        Ok(n > 0)
    }

    pub fn list_rag_queue(
        &self,
        project_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RagQueueItem>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let sql = if project_hash.is_some() {
            "SELECT q.project_hash, p.name, q.source_path, q.status, q.queued_at
             FROM rag_queue q
             LEFT JOIN projects p ON p.hash = q.project_hash
             WHERE q.project_hash=?1
             ORDER BY
                CASE q.status WHEN 'processing' THEN 0 ELSE 1 END,
                q.queued_at ASC
             LIMIT ?2"
        } else {
            "SELECT q.project_hash, p.name, q.source_path, q.status, q.queued_at
             FROM rag_queue q
             LEFT JOIN projects p ON p.hash = q.project_hash
             ORDER BY
                CASE q.status WHEN 'processing' THEN 0 ELSE 1 END,
                q.queued_at ASC
             LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(hash) = project_hash {
            stmt.query_map(rusqlite::params![hash, limit as i64], row_to_rag_queue_item)?
        } else {
            stmt.query_map(rusqlite::params![limit as i64], row_to_rag_queue_item)?
        };
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    pub fn rag_info_summary(&self) -> Result<RagInfoSummary> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        let total_chunks =
            conn.query_row("SELECT COUNT(*) FROM rag_chunks", [], |row| row.get(0))?;
        let indexed_projects = conn.query_row(
            "SELECT COUNT(DISTINCT project_hash) FROM rag_chunks",
            [],
            |row| row.get(0),
        )?;
        let queued_items = conn.query_row(
            "SELECT COUNT(*) FROM rag_queue WHERE status='queued'",
            [],
            |row| row.get(0),
        )?;
        let processing_items = conn.query_row(
            "SELECT COUNT(*) FROM rag_queue WHERE status='processing'",
            [],
            |row| row.get(0),
        )?;

        Ok(RagInfoSummary {
            total_chunks,
            indexed_projects,
            queued_items,
            processing_items,
        })
    }

    // ── RAG chunks (FTS5) ──────────────────────────────────────────────

    /// Delete all chunks for a file, then insert new ones.
    pub fn replace_chunks(
        &self,
        project_hash: &str,
        source_path: &str,
        chunks: &[Chunk],
    ) -> Result<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        let tx = conn.transaction()?;

        tx.execute(
            "DELETE FROM rag_chunks WHERE project_hash=?1 AND source_path=?2",
            rusqlite::params![project_hash, source_path],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO rag_chunks(id,project_hash,source_path,chunk_index,content,lang,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;

            for c in chunks {
                stmt.execute(rusqlite::params![
                    c.id,
                    c.project_hash,
                    c.source_path,
                    c.chunk_index,
                    c.content,
                    c.lang,
                    c.updated_at
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// FTS5 search over chunk content.
    pub fn search_chunks(
        &self,
        query: &str,
        project_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Chunk>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;

        // Escape FTS5 special chars minimally
        let fts_query = query.replace('"', "\"\"");

        if let Some(ph) = project_hash {
            let mut stmt = conn.prepare(
                "SELECT c.id,c.project_hash,c.source_path,c.chunk_index,c.content,c.lang,c.updated_at
                 FROM rag_chunks_fts f
                 JOIN rag_chunks c ON c.rowid = f.rowid
                 WHERE rag_chunks_fts MATCH ?1 AND c.project_hash=?2
                 ORDER BY rank LIMIT ?3",
            )?;
            let rows: Vec<Chunk> = stmt
                .query_map(rusqlite::params![fts_query, ph, limit as i64], row_to_chunk)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT c.id,c.project_hash,c.source_path,c.chunk_index,c.content,c.lang,c.updated_at
                 FROM rag_chunks_fts f
                 JOIN rag_chunks c ON c.rowid = f.rowid
                 WHERE rag_chunks_fts MATCH ?1
                 ORDER BY rank LIMIT ?2",
            )?;
            let rows: Vec<Chunk> = stmt
                .query_map(rusqlite::params![fts_query, limit as i64], row_to_chunk)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        }
    }
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        hash: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        tags: row.get(4)?,
        indexed_at: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn row_to_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chunk> {
    Ok(Chunk {
        id: row.get(0)?,
        project_hash: row.get(1)?,
        source_path: row.get(2)?,
        chunk_index: row.get(3)?,
        content: row.get(4)?,
        lang: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_rag_queue_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<RagQueueItem> {
    Ok(RagQueueItem {
        project_hash: row.get(0)?,
        project_name: row.get(1)?,
        source_path: row.get(2)?,
        status: row.get(3)?,
        queued_at: row.get(4)?,
    })
}

/// Returns true if the file at `path` has an extension the RAG indexer can process.
/// Mirrors the extension list in `crate::rag::chunker::detect_lang` without
/// creating a cross-module dependency cycle (db → rag → db).
fn is_indexable_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "rs" | "py"
            | "java"
            | "kt"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "c"
            | "cpp"
            | "h"
            | "md"
            | "mdx"
            | "yaml"
            | "yml"
            | "toml"
            | "txt"
    )
}

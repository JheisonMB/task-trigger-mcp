#![allow(dead_code)]
//! SQLite repositories for projects and RAG chunks (FTS5).

use anyhow::Result;

use crate::db::Database;
use crate::domain::project::Project;

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
               name=excluded.name,
               description=COALESCE(excluded.description, description),
               tags=COALESCE(excluded.tags, tags),
               indexed_at=excluded.indexed_at",
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

    // ── RAG chunks (FTS5) ──────────────────────────────────────────────

    /// Delete all chunks for a file, then insert new ones.
    pub fn replace_chunks(
        &self,
        project_hash: &str,
        source_path: &str,
        chunks: &[Chunk],
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        conn.execute(
            "DELETE FROM rag_chunks WHERE project_hash=?1 AND source_path=?2",
            rusqlite::params![project_hash, source_path],
        )?;
        for c in chunks {
            conn.execute(
                "INSERT INTO rag_chunks(id,project_hash,source_path,chunk_index,content,lang,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![
                    c.id, c.project_hash, c.source_path, c.chunk_index,
                    c.content, c.lang, c.updated_at
                ],
            )?;
        }
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

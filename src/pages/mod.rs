//! Page read/write pipeline.
//!
//! Writing a page is a single logical operation that touches four places:
//! 1. The markdown file on disk under `paths.content_root`.
//! 2. A git commit attributed to the writing actor.
//! 3. A row in `pages` (insert or update) + `page_edits`.
//! 4. The `pages_fts` index.
//!
//! Disk → git → DB → FTS, in that order. If DB/FTS update fails after the
//! commit lands, we surface an error but the commit stays — git is the
//! authoritative content store; the DB is a derived index that can be
//! repaired (see DESIGN.md §11 FTS index sync).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::error::{AppError, Result};
use crate::git::{CommitInfo, Repo};

#[derive(Debug, Clone)]
pub struct PageMeta {
    pub id: i64,
    pub path: String,
    pub layer: String,
    pub title: String,
    pub last_edit_at: Option<i64>,
    pub last_edit_by: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub meta: PageMeta,
    pub content: String,
}

/// Validate that `path` is a safe relative wiki path:
/// - No leading slash, no `..` segments, no backslashes.
/// - Segments must match `[A-Za-z0-9._-]+`.
/// - At least one segment, max 64 chars per segment, max 200 chars total.
pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 200 {
        return Err(AppError::BadRequest("path length out of range".into()));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(AppError::BadRequest("path must be relative, forward-slash only".into()));
    }
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(AppError::BadRequest(format!("illegal segment: '{seg}'")));
        }
        if seg.len() > 64 {
            return Err(AppError::BadRequest("segment too long".into()));
        }
        if !seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err(AppError::BadRequest(format!("illegal character in '{seg}'")));
        }
    }
    Ok(())
}

fn file_path(content_root: &Path, page_path: &str) -> PathBuf {
    let mut p = content_root.to_path_buf();
    for seg in page_path.split('/') {
        p.push(seg);
    }
    p.set_extension("md");
    p
}

/// Derive a title from the first `# heading` line, or fall back to the
/// last path segment with hyphens replaced by spaces.
pub fn derive_title(content: &str, page_path: &str) -> String {
    for line in content.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("# ") {
            return rest.trim().to_string();
        }
    }
    page_path
        .rsplit('/')
        .next()
        .unwrap_or(page_path)
        .replace(['-', '_'], " ")
}

fn content_hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

pub async fn read(pool: &SqlitePool, content_root: &Path, page_path: &str) -> Result<Page> {
    validate_path(page_path)?;
    let meta = read_meta(pool, page_path).await?
        .ok_or_else(|| AppError::NotFound(format!("page {page_path}")))?;
    let fp = file_path(content_root, page_path);
    let content = std::fs::read_to_string(&fp)
        .map_err(|e| AppError::Internal(format!("read {}: {e}", fp.display())))?;
    Ok(Page { meta, content })
}

pub async fn read_meta(pool: &SqlitePool, page_path: &str) -> Result<Option<PageMeta>> {
    let row = sqlx::query_as::<_, (i64, String, String, String, Option<i64>, Option<i64>)>(
        "SELECT id, path, layer, title, last_edit_at, last_edit_by
         FROM pages WHERE path = ?",
    )
    .bind(page_path)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| PageMeta {
        id: r.0,
        path: r.1,
        layer: r.2,
        title: r.3,
        last_edit_at: r.4,
        last_edit_by: r.5,
    }))
}

/// List pages, newest first.
pub async fn recent(pool: &SqlitePool, limit: i64) -> Result<Vec<PageMeta>> {
    let rows = sqlx::query_as::<_, (i64, String, String, String, Option<i64>, Option<i64>)>(
        "SELECT id, path, layer, title, last_edit_at, last_edit_by
         FROM pages ORDER BY COALESCE(last_edit_at, updated_at) DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PageMeta {
            id: r.0,
            path: r.1,
            layer: r.2,
            title: r.3,
            last_edit_at: r.4,
            last_edit_by: r.5,
        })
        .collect())
}

/// Write (create or update) `page_path` with `content`. Performs the full
/// disk → git → DB → FTS pipeline and returns the updated metadata.
pub async fn write(
    pool: &SqlitePool,
    repo: &Repo,
    content_root: &Path,
    page_path: &str,
    layer: &str,
    content: &str,
    actor_id: i64,
    actor_type: &str,
    actor_handle: &str,
    summary: Option<&str>,
) -> Result<PageMeta> {
    validate_path(page_path)?;

    // 1. Disk.
    let fp = file_path(content_root, page_path);
    if let Some(parent) = fp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&fp, content.as_bytes())?;

    // 2. Git.
    let rel = relative_to(repo.root(), &fp).ok_or_else(|| {
        AppError::Internal(format!(
            "file {} is not under repo root {}",
            fp.display(),
            repo.root().display()
        ))
    })?;
    let msg = summary
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("edit: {page_path}"));
    let commit_oid = repo.commit_file(&rel, actor_type, actor_handle, &msg).await?;

    // 3. DB.
    let now = crate::db::now_ts();
    let title = derive_title(content, page_path);
    let hash = content_hash(content);

    let existing = read_meta(pool, page_path).await?;
    let page_id = if let Some(prev) = existing {
        sqlx::query(
            "UPDATE pages SET title = ?, content_hash = ?, updated_at = ?, last_edit_at = ?, last_edit_by = ?, layer = ?
             WHERE id = ?",
        )
        .bind(&title)
        .bind(&hash)
        .bind(now)
        .bind(now)
        .bind(actor_id)
        .bind(layer)
        .bind(prev.id)
        .execute(pool)
        .await?;
        prev.id
    } else {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO pages(path, layer, title, content_hash, created_at, updated_at, last_edit_at, last_edit_by)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(page_path)
        .bind(layer)
        .bind(&title)
        .bind(&hash)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(actor_id)
        .fetch_one(pool)
        .await?
    };

    sqlx::query(
        "INSERT INTO page_edits(page_id, actor_id, git_commit, summary, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(page_id)
    .bind(actor_id)
    .bind(&commit_oid)
    .bind(summary)
    .bind(now)
    .execute(pool)
    .await?;

    // 4. FTS5: delete-then-insert for the rowid-by-path.
    let fts_rowid = page_id;
    sqlx::query("DELETE FROM pages_fts WHERE rowid = ?")
        .bind(fts_rowid)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO pages_fts(rowid, title, content, path) VALUES (?, ?, ?, ?)")
        .bind(fts_rowid)
        .bind(&title)
        .bind(content)
        .bind(page_path)
        .execute(pool)
        .await?;

    Ok(PageMeta {
        id: page_id,
        path: page_path.to_string(),
        layer: layer.to_string(),
        title,
        last_edit_at: Some(now),
        last_edit_by: Some(actor_id),
    })
}

fn relative_to(root: &Path, child: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let child = child.canonicalize().ok()?;
    child.strip_prefix(&root).ok().map(|p| p.to_path_buf())
}

pub async fn history(repo: &Repo, page_path: &str, limit: usize) -> Result<Vec<CommitInfo>> {
    validate_path(page_path)?;
    let mut rel = PathBuf::new();
    for seg in page_path.split('/') {
        rel.push(seg);
    }
    rel.set_extension("md");
    repo.file_history(&rel, limit).await
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub snippet: String,
}

pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let q = fts_match_query(query);
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT path, title, snippet(pages_fts, 1, '<mark>', '</mark>', '…', 16) AS snippet
         FROM pages_fts WHERE pages_fts MATCH ?
         ORDER BY rank LIMIT ?",
    )
    .bind(&q)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SearchHit {
            path: r.0,
            title: r.1,
            snippet: r.2,
        })
        .collect())
}

/// Convert a user query into an FTS5 MATCH expression. We use prefix matches
/// per token so partial-word search is forgiving; quotes / special FTS
/// punctuation are stripped to avoid syntax errors.
fn fts_match_query(q: &str) -> String {
    q.split_whitespace()
        .filter_map(|tok| {
            let t: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
                .collect();
            if t.is_empty() { None } else { Some(format!("\"{t}\"*")) }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_ok_and_err() {
        assert!(validate_path("welcome").is_ok());
        assert!(validate_path("notes/2026/may").is_ok());
        assert!(validate_path("/leading-slash").is_err());
        assert!(validate_path("..").is_err());
        assert!(validate_path("a/../b").is_err());
        assert!(validate_path("a\\b").is_err());
        assert!(validate_path("hello world").is_err());
        assert!(validate_path("").is_err());
    }

    #[test]
    fn derive_title_from_heading_or_path() {
        assert_eq!(derive_title("# Hello\n\nbody", "hello"), "Hello");
        assert_eq!(derive_title("no heading", "notes/big-idea"), "big idea");
    }

    #[test]
    fn fts_query_sanitization() {
        assert_eq!(fts_match_query("hello world"), "\"hello\"* \"world\"*");
        assert_eq!(fts_match_query("  drop;table--"), "\"droptable--\"*");
        assert_eq!(fts_match_query(""), "");
    }
}

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
use crate::git::{CommitInfo, Repo, WriteSession};

#[derive(Debug, Clone)]
pub struct PageMeta {
    pub id: i64,
    pub path: String,
    pub layer: String,
    pub title: String,
    pub last_edit_at: Option<i64>,
    pub last_edit_by: Option<i64>,
    /// Resolved from `last_edit_by` so a page can say *who* touched it
    /// without every caller joining `actors` itself. Attribution that costs
    /// an extra query tends to quietly not get shown.
    pub last_edit_handle: Option<String>,
    pub last_edit_type: Option<String>,
}

/// `SELECT` list shared by every page-metadata query, so the column order
/// the row tuples depend on is written once.
const PAGE_META_COLUMNS: &str = "p.id, p.path, p.layer, p.title, p.last_edit_at, p.last_edit_by, \
                                 a.handle, a.type";

type PageMetaRow = (
    i64,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

fn page_meta_from_row(r: PageMetaRow) -> PageMeta {
    PageMeta {
        id: r.0,
        path: r.1,
        layer: r.2,
        title: r.3,
        last_edit_at: r.4,
        last_edit_by: r.5,
        last_edit_handle: r.6,
        last_edit_type: r.7,
    }
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

/// The page's file, relative to the content root.
///
/// The rule is *append* `.md`, never `set_extension`. `set_extension`
/// **replaces** a trailing dotted segment, which silently collapses distinct
/// pages onto one file: `notes/v1.2` and `notes/v1.9` would both become
/// `notes/v1.md`, and `notes` would collide with `notes.old`. Two rows in
/// `pages`, one file on disk — a write to either destroys the other.
/// Appending keeps `path` ↔ file bijective for every path `validate_path`
/// admits.
fn relative_file_path(page_path: &str) -> PathBuf {
    let mut p = PathBuf::new();
    let mut segs = page_path.split('/').peekable();
    while let Some(seg) = segs.next() {
        if segs.peek().is_none() {
            p.push(format!("{seg}.md"));
        } else {
            p.push(seg);
        }
    }
    p
}

fn file_path(content_root: &Path, page_path: &str) -> PathBuf {
    content_root.join(relative_file_path(page_path))
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
    let sql = format!(
        "SELECT {PAGE_META_COLUMNS}
         FROM pages p LEFT JOIN actors a ON a.id = p.last_edit_by
         WHERE p.path = ?"
    );
    let row = sqlx::query_as::<_, PageMetaRow>(&sql)
        .bind(page_path)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(page_meta_from_row))
}

/// List pages, most recently touched first.
pub async fn recent(pool: &SqlitePool, limit: i64) -> Result<Vec<PageMeta>> {
    let sql = format!(
        "SELECT {PAGE_META_COLUMNS}
         FROM pages p LEFT JOIN actors a ON a.id = p.last_edit_by
         ORDER BY COALESCE(p.last_edit_at, p.updated_at) DESC LIMIT ?"
    );
    let rows = sqlx::query_as::<_, PageMetaRow>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(page_meta_from_row).collect())
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

    // A cancelled request may stop waiting, but must not abandon the database
    // projection after its Git commit. Only accepted writes are detached:
    // cancellation while waiting for the writer lock has no side effects.
    let writer = repo.write_session().await;
    let pool = pool.clone();
    let repo = repo.clone();
    let content_root = content_root.to_path_buf();
    let page_path = page_path.to_string();
    let layer = layer.to_string();
    let content = content.to_string();
    let actor_type = actor_type.to_string();
    let actor_handle = actor_handle.to_string();
    let summary = summary.map(str::to_string);

    tokio::spawn(async move {
        let result = write_serialized(
            &pool, &repo, &writer, &content_root, &page_path, &layer, &content,
            actor_id, &actor_type, &actor_handle, summary.as_deref(),
        )
        .await;
        if let Err(error) = &result {
            tracing::warn!(path = %page_path, error = %error, "page write failed");
        }
        result
    })
    .await
    .map_err(|error| AppError::Internal(format!("page write task: {error}")))?
}

async fn write_serialized(
    pool: &SqlitePool,
    repo: &Repo,
    writer: &WriteSession,
    content_root: &Path,
    page_path: &str,
    layer: &str,
    content: &str,
    actor_id: i64,
    actor_type: &str,
    actor_handle: &str,
    summary: Option<&str>,
) -> Result<PageMeta> {
    // Reject invalid input before changing any content or history. The actor
    // ID used by SQLite and the identity written into Git must describe the
    // same active actor, even when this service is called outside HTTP.
    let active_layer: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM layers WHERE code = ? AND is_active = 1)",
    )
    .bind(layer)
    .fetch_one(pool)
    .await?;
    if !active_layer {
        return Err(AppError::BadRequest(format!("unknown or inactive layer: {layer}")));
    }
    let actor = crate::db::queries::actor_by_id(pool, actor_id)
        .await?
        .ok_or(AppError::Forbidden)?;
    if !actor.is_active || actor.actor_type != actor_type || actor.handle != actor_handle {
        return Err(AppError::Forbidden);
    }

    // The caller retains this write session through the SQLite commit, so
    // the database cannot project successful Git writes in reverse order.
    std::fs::create_dir_all(content_root)?;
    let prefix = relative_to(repo.root(), content_root).ok_or_else(|| {
        AppError::Internal(format!(
            "content root {} is not under repo root {}",
            content_root.display(),
            repo.root().display()
        ))
    })?;
    let repo_rel = prefix.join(relative_file_path(page_path));
    let msg = summary
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("edit: {page_path}"));
    let commit_oid = writer
        .write_and_commit(&repo_rel, content, actor_type, actor_handle, &msg)
        .await?;

    // 3 + 4. DB and FTS, in one transaction.
    //
    // The metadata row, its edit record and the search index are three views
    // of a single fact ("this page now says that"). Committing them
    // separately can leave search returning text that no page contains, or a
    // page with no attribution for its latest edit. Git remains the
    // authoritative content store. A process crash or database failure still
    // needs reconciliation: this transaction cannot roll back a Git commit.
    let now = crate::db::now_ts();
    let title = derive_title(content, page_path);
    let hash = content_hash(content);

    // Reserve the SQLite writer before reading the old row. A deferred
    // read-to-write upgrade can fail immediately with SQLITE_BUSY rather
    // than honoring the busy timeout when bookkeeping writes contend.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM pages WHERE path = ?")
        .bind(page_path)
        .fetch_optional(&mut *tx)
        .await?;

    let page_id = if let Some((prev_id,)) = existing {
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
        .bind(prev_id)
        .execute(&mut *tx)
        .await?;
        prev_id
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
        .fetch_one(&mut *tx)
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
    .execute(&mut *tx)
    .await?;

    // FTS5: delete-then-insert for the rowid-by-path.
    sqlx::query("DELETE FROM pages_fts WHERE rowid = ?")
        .bind(page_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO pages_fts(rowid, title, content, path) VALUES (?, ?, ?, ?)")
        .bind(page_id)
        .bind(&title)
        .bind(content)
        .bind(page_path)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(PageMeta {
        id: page_id,
        path: page_path.to_string(),
        layer: layer.to_string(),
        title,
        last_edit_at: Some(now),
        last_edit_by: Some(actor_id),
        last_edit_handle: Some(actor_handle.to_string()),
        last_edit_type: Some(actor_type.to_string()),
    })
}

fn relative_to(root: &Path, child: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let child = child.canonicalize().ok()?;
    child.strip_prefix(&root).ok().map(|p| p.to_path_buf())
}

pub async fn history(repo: &Repo, page_path: &str, limit: usize) -> Result<Vec<CommitInfo>> {
    validate_path(page_path)?;
    repo.file_history(&relative_file_path(page_path), limit).await
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub snippet: String,
}

/// Sentinels delimiting a match inside [`SearchHit::snippet`].
///
/// FTS5's `snippet()` splices its markers into the *raw indexed text* — which
/// here is raw page markdown. Asking for `<mark>`/`</mark>` and rendering the
/// result as HTML would therefore let any actor who can write a page inject
/// arbitrary markup into every reader's search results. Control characters
/// cannot occur in rendered output, so the web layer can escape the whole
/// snippet as ordinary text and turn only these two bytes into tags. See
/// `web::templates::render_snippet`.
pub const SNIPPET_OPEN: char = '\u{1}';
pub const SNIPPET_CLOSE: char = '\u{2}';

pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let q = fts_match_query(query);
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT path, title, snippet(pages_fts, 1, char(1), char(2), '…', 16) AS snippet
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

    /// Distinct wiki paths must never share a file. This is the regression
    /// guard for the `set_extension` collision described on
    /// `relative_file_path`.
    #[test]
    fn distinct_paths_map_to_distinct_files() {
        let cases = [
            "notes", "notes.old", "notes/v1.2", "notes/v1.9", "a.b.c", "readme.md",
        ];
        for p in cases {
            assert!(validate_path(p).is_ok(), "{p} should be a legal path");
        }
        let files: Vec<_> = cases.iter().map(|p| relative_file_path(p)).collect();
        for (i, a) in files.iter().enumerate() {
            for (j, b) in files.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{} and {} collide on {a:?}", cases[i], cases[j]);
                }
            }
        }
    }

    #[test]
    fn file_path_appends_rather_than_replaces() {
        assert_eq!(relative_file_path("notes"), PathBuf::from("notes.md"));
        assert_eq!(
            relative_file_path("notes/v1.2"),
            PathBuf::from("notes").join("v1.2.md")
        );
        assert_eq!(
            relative_file_path("a/b/c"),
            PathBuf::from("a").join("b").join("c.md")
        );
    }

    #[test]
    fn fts_query_sanitization() {
        assert_eq!(fts_match_query("hello world"), "\"hello\"* \"world\"*");
        assert_eq!(fts_match_query("  drop;table--"), "\"droptable--\"*");
        assert_eq!(fts_match_query(""), "");
    }
}

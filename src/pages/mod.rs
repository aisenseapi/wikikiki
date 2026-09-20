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
    /// The page's current revision: a hash of its content.
    ///
    /// Content-derived rather than a counter, which makes it meaningful on
    /// its own — "the version that says *this*" — and lets a client verify it
    /// from bytes it already holds. `None` only for rows written outside this
    /// service, which therefore cannot prove what a conditional write is
    /// being applied to.
    pub revision: Option<String>,
}

/// `SELECT` list shared by every page-metadata query, so the column order
/// the row tuples depend on is written once.
const PAGE_META_COLUMNS: &str = "p.id, p.path, p.layer, p.title, p.last_edit_at, p.last_edit_by, \
                                 a.handle, a.type, p.content_hash";

type PageMetaRow = (
    i64,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
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
        revision: r.8,
    }
}

#[derive(Debug, Clone)]
pub struct Page {
    pub meta: PageMeta,
    pub content: String,
}

/// What the page must currently look like for a write to be allowed.
///
/// This is optimistic concurrency, and it exists because the alternative is
/// silent loss: an actor that read a page minutes ago, reasoned about it, and
/// writes back its conclusion will otherwise erase whatever landed in
/// between — without either actor noticing. In a substrate where agents write
/// unattended, "last writer wins" means "whoever is slowest is right".
///
/// The condition is evaluated **inside the writer lock**, immediately before
/// the Git commit. Checking it in a handler would be worse than not checking:
/// the gap between reading the current revision and writing is precisely
/// where the lost update happens, so a check on the wrong side of the lock
/// provides reassurance without protection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Precondition {
    /// No condition; last writer wins. The historical behaviour, kept as the
    /// default so existing clients — including `scripts/wikikiki-sync.sh` —
    /// keep working unchanged.
    #[default]
    None,
    /// The page must currently have this revision (`If-Match: "<rev>"`).
    Revision(String),
    /// The page must exist, whatever it says (`If-Match: *`).
    Exists,
    /// The page must not exist (`If-None-Match: *`). This is how a caller
    /// creates a page without risking an overwrite.
    Absent,
}

/// The state a precondition is evaluated against.
///
/// Three cases, not two: a row written outside this service can exist without
/// a recorded revision, and a conditional write must fail there rather than
/// guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentState<'a> {
    Absent,
    Present(Option<&'a str>),
}

fn short_rev(rev: &str) -> &str {
    &rev[..rev.len().min(12)]
}

impl Precondition {
    fn check(&self, current: CurrentState<'_>) -> Result<()> {
        let exists = matches!(current, CurrentState::Present(_));
        match self {
            Precondition::None => Ok(()),

            Precondition::Exists if exists => Ok(()),
            Precondition::Exists => {
                Err(AppError::PreconditionFailed("page does not exist".into()))
            }

            Precondition::Absent if !exists => Ok(()),
            Precondition::Absent => {
                Err(AppError::PreconditionFailed("page already exists".into()))
            }

            Precondition::Revision(want) => match current {
                CurrentState::Present(Some(have)) if want == have => Ok(()),
                CurrentState::Present(Some(have)) => {
                    Err(AppError::PreconditionFailed(format!(
                        "page is at revision {}, expected {}",
                        short_rev(have),
                        short_rev(want)
                    )))
                }
                // A row this service did not write has no revision to compare
                // against. Refusing is the only honest answer: the caller
                // asked to modify a specific version and we cannot tell which
                // one is here.
                CurrentState::Present(_) => Err(AppError::PreconditionFailed(
                    "page has no recorded revision to compare against".into(),
                )),
                CurrentState::Absent => {
                    Err(AppError::PreconditionFailed("page does not exist".into()))
                }
            },
        }
    }
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

/// One page write, as a named record.
///
/// This was eleven positional parameters, of which `layer`, `actor_type`,
/// `actor_handle` and `summary` are all strings — transposing two of them
/// compiles cleanly and attributes an edit to the wrong actor or files it
/// under the wrong memory layer. Naming the fields removes that class of
/// mistake at no cost to the caller.
#[derive(Debug, Clone)]
pub struct WriteRequest<'a> {
    pub path: &'a str,
    pub layer: &'a str,
    pub content: &'a str,
    pub actor_id: i64,
    pub actor_type: &'a str,
    pub actor_handle: &'a str,
    pub summary: Option<&'a str>,
    /// Checked under the writer lock; see [`Precondition`].
    pub precondition: Precondition,
}

/// Write (create or update) a page. Performs the full disk → git → DB → FTS
/// pipeline and returns the updated metadata.
pub async fn write(
    pool: &SqlitePool,
    repo: &Repo,
    content_root: &Path,
    req: WriteRequest<'_>,
) -> Result<PageMeta> {
    let WriteRequest {
        path: page_path,
        layer,
        content,
        actor_id,
        actor_type,
        actor_handle,
        summary,
        precondition,
    } = req;
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
            &pool,
            &repo,
            &writer,
            &content_root,
            WriteRequest {
                path: &page_path,
                layer: &layer,
                content: &content,
                actor_id,
                actor_type: &actor_type,
                actor_handle: &actor_handle,
                summary: summary.as_deref(),
                precondition,
            },
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
    req: WriteRequest<'_>,
) -> Result<PageMeta> {
    let WriteRequest {
        path: page_path,
        layer,
        content,
        actor_id,
        actor_type,
        actor_handle,
        summary,
        precondition,
    } = req;
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

    // Evaluate the precondition here, not in the handler: the writer lock is
    // already held, so nothing can change the page between this read and the
    // commit below. That ordering is the entire guarantee — the same check
    // one layer up would be a race with a reassuring name.
    let existing_row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT content_hash FROM pages WHERE path = ?")
            .bind(page_path)
            .fetch_optional(pool)
            .await?;
    let current = match &existing_row {
        Some((hash,)) => CurrentState::Present(hash.as_deref()),
        None => CurrentState::Absent,
    };
    precondition.check(current)?;

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
        revision: Some(hash),
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

    fn present(rev: &str) -> CurrentState<'_> {
        CurrentState::Present(Some(rev))
    }

    #[test]
    fn no_precondition_accepts_anything() {
        let p = Precondition::None;
        assert!(p.check(CurrentState::Absent).is_ok());
        assert!(p.check(present("abc")).is_ok());
        assert!(p.check(CurrentState::Present(None)).is_ok());
    }

    #[test]
    fn matching_revision_is_accepted_and_a_stale_one_is_not() {
        let p = Precondition::Revision("abc123".into());
        assert!(p.check(present("abc123")).is_ok());
        assert!(matches!(
            p.check(present("def456")),
            Err(AppError::PreconditionFailed(_))
        ));
    }

    /// The error has to name both revisions. "Precondition failed" alone
    /// leaves a writer unable to tell a stale edit from a wrong path.
    #[test]
    fn stale_revision_error_names_both_sides() {
        let p = Precondition::Revision("aaaaaaaaaaaaaaaa".into());
        let Err(AppError::PreconditionFailed(msg)) = p.check(present("bbbbbbbbbbbbbbbb")) else {
            panic!("expected a precondition failure");
        };
        assert!(msg.contains("aaaaaaaaaaaa"), "{msg}");
        assert!(msg.contains("bbbbbbbbbbbb"), "{msg}");
    }

    #[test]
    fn revision_check_fails_on_a_missing_page() {
        let p = Precondition::Revision("abc".into());
        assert!(matches!(
            p.check(CurrentState::Absent),
            Err(AppError::PreconditionFailed(_))
        ));
    }

    /// A row written outside this service has no revision to compare against.
    /// Guessing either way would be worse than refusing.
    #[test]
    fn revision_check_fails_when_the_page_has_no_recorded_revision() {
        let p = Precondition::Revision("abc".into());
        assert!(matches!(
            p.check(CurrentState::Present(None)),
            Err(AppError::PreconditionFailed(_))
        ));
    }

    #[test]
    fn exists_and_absent_are_mirror_images() {
        assert!(Precondition::Exists.check(present("x")).is_ok());
        assert!(Precondition::Exists.check(CurrentState::Present(None)).is_ok());
        assert!(matches!(
            Precondition::Exists.check(CurrentState::Absent),
            Err(AppError::PreconditionFailed(_))
        ));

        assert!(Precondition::Absent.check(CurrentState::Absent).is_ok());
        assert!(matches!(
            Precondition::Absent.check(present("x")),
            Err(AppError::PreconditionFailed(_))
        ));
    }

    #[test]
    fn default_precondition_preserves_the_historical_behaviour() {
        assert_eq!(Precondition::default(), Precondition::None);
    }

    #[test]
    fn short_rev_does_not_panic_on_short_input() {
        assert_eq!(short_rev("abc"), "abc");
        assert_eq!(short_rev(""), "");
        assert_eq!(short_rev("0123456789abcdef"), "0123456789ab");
    }

    #[test]
    fn fts_query_sanitization() {
        assert_eq!(fts_match_query("hello world"), "\"hello\"* \"world\"*");
        assert_eq!(fts_match_query("  drop;table--"), "\"droptable--\"*");
        assert_eq!(fts_match_query(""), "");
    }
}

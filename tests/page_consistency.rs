//! Cross-store guarantees for page writes, using real SQLite, files and Git.
//!
//! These exercise the full page service rather than only the Git wrapper:
//! a correct commit can still be followed by an out-of-order DB projection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::sync::Barrier;

use wikikiki::db::{connect_and_migrate, queries};
use wikikiki::git::Repo;
use wikikiki::pages;

const AUTHOR_TEMPLATE: &str = "{actor_type}:{actor_handle} <{actor_handle}@wikikiki.local>";

struct Fixture {
    pool: SqlitePool,
    repo: Repo,
    content_root: PathBuf,
    _tmp: TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let tmp = TempDir::new().expect("temporary directory");
        let content_root = tmp.path().join("content");
        let pool = connect_and_migrate(&tmp.path().join("wiki.db"))
            .await
            .expect("database migrations");
        let repo = Repo::open_or_init(&content_root, AUTHOR_TEMPLATE).expect("content repository");
        Self {
            pool,
            repo,
            content_root,
            _tmp: tmp,
        }
    }

    async fn actor(&self, handle: &str) -> i64 {
        queries::insert_actor(&self.pool, "agent", handle, handle, false)
            .await
            .expect("insert test actor")
    }

    async fn seed_page(&self, path: &str, actor_id: i64, handle: &str) {
        pages::write(
            &self.pool,
            &self.repo,
            &self.content_root,
            path,
            "semantic",
            "# Original\n\noriginalcontenttoken\n",
            actor_id,
            "agent",
            handle,
            Some("original page"),
        )
        .await
        .expect("seed page");
    }

    fn head(&self) -> String {
        let repo = gix::open(self.repo.root()).expect("open content repository");
        repo.head_id().expect("HEAD").detach().to_string()
    }

    async fn snapshot(&self, path: &str) -> Snapshot {
        let disk = match std::fs::read(self.content_root.join(format!("{path}.md"))) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read page snapshot: {error}"),
        };
        Snapshot {
            head: self.head(),
            disk,
            pages: sqlx::query_as::<_, PageRow>(
                "SELECT id, path, layer, title, content_hash, created_at, updated_at,
                        last_edit_at, last_edit_by FROM pages ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await
            .expect("page snapshot"),
            edits: sqlx::query_as::<_, EditRow>(
                "SELECT id, page_id, actor_id, git_commit, summary, created_at
                 FROM page_edits ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await
            .expect("edit snapshot"),
            fts: sqlx::query_as::<_, FtsRow>(
                "SELECT rowid, title, content, path FROM pages_fts ORDER BY rowid",
            )
            .fetch_all(&self.pool)
            .await
            .expect("search snapshot"),
        }
    }
}

type PageRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
);
type EditRow = (i64, i64, i64, Option<String>, Option<String>, i64);
type FtsRow = (i64, String, String, String);

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    head: String,
    disk: Option<Vec<u8>>,
    pages: Vec<PageRow>,
    edits: Vec<EditRow>,
    fts: Vec<FtsRow>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_page_writes_keep_every_store_and_actor_in_agreement() {
    const WRITERS: usize = 16;
    const PAGE: &str = "notes/shared";
    let fixture = Fixture::new().await;
    let mut submitted = Vec::new();
    for i in 0..WRITERS {
        let handle = format!("writer{i}");
        let actor_id = fixture.actor(&handle).await;
        let content = format!("# Version {i}\n\nwriter{i}contenttoken\n");
        submitted.push((actor_id, handle, content));
    }

    // Release real writers together. The timeout is only a deadlock guard;
    // correctness is checked against actual persisted bytes and attribution.
    let barrier = Arc::new(Barrier::new(WRITERS + 1));
    let mut tasks = Vec::new();
    for (actor_id, handle, content) in &submitted {
        let pool = fixture.pool.clone();
        let repo = fixture.repo.clone();
        let content_root = fixture.content_root.clone();
        let barrier = barrier.clone();
        let (actor_id, handle, content) = (*actor_id, handle.clone(), content.clone());
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            pages::write(
                &pool,
                &repo,
                &content_root,
                PAGE,
                "semantic",
                &content,
                actor_id,
                "agent",
                &handle,
                Some(&format!("write by {handle}")),
            )
            .await
        }));
    }
    barrier.wait().await;
    let results = tokio::time::timeout(Duration::from_secs(30), async {
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.expect("writer task").expect("page write"));
        }
        results
    })
    .await
    .expect("concurrent writes must finish");

    let page = pages::read(&fixture.pool, &fixture.content_root, PAGE)
        .await
        .expect("read completed page");
    assert!(results.iter().all(|result| result.id == page.meta.id));
    let relpath = Path::new("notes/shared.md");
    let committed = fixture
        .repo
        .file_at_commit(&fixture.head(), relpath)
        .await
        .expect("read HEAD blob")
        .expect("page in HEAD");
    let final_writer = submitted
        .iter()
        .find(|(_, _, content)| content == &committed)
        .expect("HEAD must contain one submitted payload");
    assert_eq!(page.content, committed, "disk must agree with Git HEAD");
    assert_eq!(
        page.meta.last_edit_by,
        Some(final_writer.0),
        "latest metadata actor"
    );
    assert_eq!(
        page.meta.last_edit_handle.as_deref(),
        Some(final_writer.1.as_str())
    );
    assert_eq!(page.meta.last_edit_type.as_deref(), Some("agent"));
    assert_eq!(page.meta.title, pages::derive_title(&committed, PAGE));

    let stored_hash: Option<String> =
        sqlx::query_scalar("SELECT content_hash FROM pages WHERE id = ?")
            .bind(page.meta.id)
            .fetch_one(&fixture.pool)
            .await
            .expect("stored hash");
    assert_eq!(
        stored_hash,
        Some(hex::encode(Sha256::digest(committed.as_bytes())))
    );
    let fts: FtsRow =
        sqlx::query_as("SELECT rowid, title, content, path FROM pages_fts WHERE rowid = ?")
            .bind(page.meta.id)
            .fetch_one(&fixture.pool)
            .await
            .expect("indexed content");
    assert_eq!(
        fts,
        (
            page.meta.id,
            page.meta.title.clone(),
            committed.clone(),
            PAGE.into()
        )
    );

    // Old versions must not survive as current search results.
    for (i, (actor_id, _, _)) in submitted.iter().enumerate() {
        let hits = pages::search(&fixture.pool, &format!("writer{i}contenttoken"), 20)
            .await
            .expect("search after concurrent writes");
        assert_eq!(
            hits.iter().any(|hit| hit.path == PAGE),
            *actor_id == final_writer.0
        );
    }

    let history = fixture
        .repo
        .file_history(relpath, WRITERS + 1)
        .await
        .expect("page Git history");
    let edits: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT actor_id, git_commit, summary FROM page_edits WHERE page_id = ? ORDER BY id",
    )
    .bind(page.meta.id)
    .fetch_all(&fixture.pool)
    .await
    .expect("page edit records");
    assert_eq!(
        history.len(),
        WRITERS,
        "one Git commit per acknowledged write"
    );
    assert_eq!(
        edits.len(),
        WRITERS,
        "one edit record per acknowledged write"
    );
    for (actor_id, handle, content) in &submitted {
        let own_edits: Vec<_> = edits.iter().filter(|edit| edit.0 == *actor_id).collect();
        assert_eq!(own_edits.len(), 1, "exactly one edit for {handle}");
        let edit = own_edits[0];
        let commit = history
            .iter()
            .find(|commit| commit.oid == edit.1)
            .expect("every edit record must identify a real page commit");
        assert_eq!(commit.author_name, format!("agent:{handle}"));
        assert_eq!(edit.2, format!("write by {handle}"));
        let blob = fixture
            .repo
            .file_at_commit(&edit.1, relpath)
            .await
            .expect("historical blob")
            .expect("page at its edit revision");
        assert_eq!(
            &blob, content,
            "{handle}'s edit must contain their own bytes"
        );
    }
    fixture.pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_request_after_git_does_not_abandon_its_database_projection() {
    const PAGE: &str = "shared";
    const CONTENT: &str = "# Survives cancellation\n\ncancellationcontenttoken\n";
    let fixture = Fixture::new().await;
    let original_id = fixture.actor("original").await;
    let writer_id = fixture.actor("writer").await;
    fixture.seed_page(PAGE, original_id, "original").await;
    let original_head = fixture.head();

    // A real SQLite writer blocks projection commits while WAL still permits
    // the operation's actor/layer preflight reads. This lets us observe the
    // exact Git-to-database boundary without guessing how long a save takes.
    let mut blocker = fixture.pool.begin().await.expect("blocking transaction");
    sqlx::query("UPDATE actors SET name = name WHERE id = ?")
        .bind(original_id)
        .execute(&mut *blocker)
        .await
        .expect("hold SQLite writer");

    let pool = fixture.pool.clone();
    let repo = fixture.repo.clone();
    let content_root = fixture.content_root.clone();
    let request = tokio::spawn(async move {
        pages::write(
            &pool,
            &repo,
            &content_root,
            PAGE,
            "semantic",
            CONTENT,
            writer_id,
            "agent",
            "writer",
            Some("survive request cancellation"),
        )
        .await
    });

    let committed_oid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let history = fixture
                .repo
                .file_history(Path::new("shared.md"), 1)
                .await
                .expect("observe Git progress");
            if let Some(commit) = history.first() {
                if commit.oid != original_head {
                    break commit.oid.clone();
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("write must reach Git while the database writer is blocked");
    assert!(
        !request.is_finished(),
        "the operation must wait for the database writer, not fail after committing Git",
    );
    request.abort();
    assert!(request
        .await
        .expect_err("request task must be cancelled")
        .is_cancelled());
    blocker.rollback().await.expect("release database writer");

    // Cancellation of the caller must not cancel the already-started write.
    // Observe the persisted result, not a sleep or an implementation signal.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let meta = pages::read_meta(&fixture.pool, PAGE)
                .await
                .expect("poll metadata");
            if meta.as_ref().and_then(|meta| meta.last_edit_by) == Some(writer_id) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached write must finish its database projection");

    let page = pages::read(&fixture.pool, &fixture.content_root, PAGE)
        .await
        .expect("read completed operation");
    assert_eq!(page.content, CONTENT);
    assert_eq!(page.meta.title, "Survives cancellation");
    assert_eq!(page.meta.last_edit_by, Some(writer_id));
    assert_eq!(
        fixture.head(),
        committed_oid,
        "recovery must not duplicate the commit"
    );
    let committed = fixture
        .repo
        .file_at_commit(&committed_oid, Path::new("shared.md"))
        .await
        .expect("read operation blob")
        .expect("committed page");
    assert_eq!(committed, CONTENT);
    let stored_hash: Option<String> =
        sqlx::query_scalar("SELECT content_hash FROM pages WHERE id = ?")
            .bind(page.meta.id)
            .fetch_one(&fixture.pool)
            .await
            .expect("projected hash");
    assert_eq!(
        stored_hash,
        Some(hex::encode(Sha256::digest(CONTENT.as_bytes())))
    );
    let indexed: String = sqlx::query_scalar("SELECT content FROM pages_fts WHERE rowid = ?")
        .bind(page.meta.id)
        .fetch_one(&fixture.pool)
        .await
        .expect("projected search content");
    assert_eq!(indexed, CONTENT);
    let edit: (i64, String) = sqlx::query_as(
        "SELECT actor_id, git_commit FROM page_edits WHERE page_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(page.meta.id)
    .fetch_one(&fixture.pool)
    .await
    .expect("projected edit attribution");
    assert_eq!(edit, (writer_id, committed_oid));
    let edit_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM page_edits WHERE page_id = ?")
        .bind(page.meta.id)
        .fetch_one(&fixture.pool)
        .await
        .expect("edit count");
    assert_eq!(
        edit_count, 2,
        "seed and detached write each appear exactly once"
    );
    fixture.pool.close().await;
}

async fn rejected_layer_preserves_all_stores(existing: bool, inactive: bool) {
    let fixture = Fixture::new().await;
    let actor_id = fixture.actor("writer").await;
    let path = if existing {
        "existing"
    } else {
        "notes/new-page"
    };
    if existing {
        fixture.seed_page(path, actor_id, "writer").await;
    }
    let layer = if inactive {
        sqlx::query("UPDATE layers SET is_active = 0 WHERE code = 'semantic'")
            .execute(&fixture.pool)
            .await
            .expect("disable layer");
        "semantic"
    } else {
        "no-such-layer"
    };
    let before = fixture.snapshot(path).await;
    let result = pages::write(
        &fixture.pool,
        &fixture.repo,
        &fixture.content_root,
        path,
        layer,
        "# Rejected\n\nrejectedcontenttoken\n",
        actor_id,
        "agent",
        "writer",
        Some("must not be committed"),
    )
    .await;
    assert!(
        result.is_err(),
        "invalid/inactive layer must reject the write"
    );
    assert_eq!(
        fixture.snapshot(path).await,
        before,
        "rejection changed persisted state"
    );
    fixture.pool.close().await;
}

#[tokio::test]
async fn unknown_layer_does_not_modify_an_existing_page() {
    rejected_layer_preserves_all_stores(true, false).await;
}

#[tokio::test]
async fn unknown_layer_does_not_create_a_page_or_commit() {
    rejected_layer_preserves_all_stores(false, false).await;
}

#[tokio::test]
async fn inactive_layer_does_not_modify_an_existing_page() {
    rejected_layer_preserves_all_stores(true, true).await;
}

#[tokio::test]
async fn inactive_layer_does_not_create_a_page_or_commit() {
    rejected_layer_preserves_all_stores(false, true).await;
}

#[tokio::test]
async fn invalid_actor_identity_cannot_change_any_store() {
    let fixture = Fixture::new().await;
    let actor_id = fixture.actor("writer").await;
    let inactive_id = fixture.actor("inactive").await;
    fixture.seed_page("existing", actor_id, "writer").await;
    sqlx::query("UPDATE actors SET is_active = 0 WHERE id = ?")
        .bind(inactive_id)
        .execute(&fixture.pool)
        .await
        .expect("disable actor");

    let attempts = [
        (actor_id, "agent", "someone-else"),
        (actor_id, "human", "writer"),
        (inactive_id, "agent", "inactive"),
        (i64::MAX, "agent", "missing"),
    ];
    for path in ["existing", "notes/new-page"] {
        let before = fixture.snapshot(path).await;
        for (id, actor_type, handle) in attempts {
            let result = pages::write(
                &fixture.pool,
                &fixture.repo,
                &fixture.content_root,
                path,
                "semantic",
                "# Unattributable\n\ninvalidactorcontenttoken\n",
                id,
                actor_type,
                handle,
                Some("must not be committed"),
            )
            .await;
            assert!(
                result.is_err(),
                "invalid actor {id}/{actor_type}/{handle} was accepted"
            );
            assert_eq!(
                fixture.snapshot(path).await,
                before,
                "rejected actor changed persisted state"
            );
        }
    }
    fixture.pool.close().await;
}

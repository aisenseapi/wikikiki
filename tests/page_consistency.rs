//! Cross-store guarantees for page writes, using real SQLite, files and Git.
//!
//! These exercise the full page service rather than only the Git wrapper:
//! a correct commit can still be followed by an out-of-order DB projection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::sync::Barrier;
use tokio::sync::Notify;

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
            pages::WriteRequest {
                path,
                layer: "semantic",
                content: "# Original\n\noriginalcontenttoken\n",
                actor_id,
                actor_type: "agent",
                actor_handle: handle,
                summary: Some("original page"),
                precondition: pages::Precondition::None,
            },
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
                pages::WriteRequest {
                    path: PAGE,
                    layer: "semantic",
                    content: &content,
                    actor_id,
                    actor_type: "agent",
                    actor_handle: &handle,
                    summary: Some(&format!("write by {handle}")),
                    precondition: pages::Precondition::None,
                },
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

    // Hold the write pool's only connection after the intent is durably
    // inserted. Git can proceed, but publication cannot acquire a connection.
    // The observer uses a separate pool, so staging does not rely on sleeps.
    let release = Arc::new(Notify::new());
    let gated = Arc::new(AtomicBool::new(false));
    let release_hook = release.clone();
    let gated_hook = gated.clone();
    let write_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .after_release(move |connection, _| {
            let release = release_hook.clone();
            let gated = gated_hook.clone();
            Box::pin(async move {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_page_writes")
                    .fetch_one(&mut *connection)
                    .await?;
                if count > 0 && !gated.swap(true, Ordering::SeqCst) {
                    release.notified().await;
                }
                Ok(true)
            })
        })
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(fixture._tmp.path().join("wiki.db"))
                .foreign_keys(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .synchronous(sqlx::sqlite::SqliteSynchronous::Full),
        )
        .await
        .expect("gated writer pool");
    let pool = write_pool.clone();
    let repo = fixture.repo.clone();
    let content_root = fixture.content_root.clone();
    let request = tokio::spawn(async move {
        pages::write(
            &pool,
            &repo,
            &content_root,
            pages::WriteRequest {
                path: PAGE,
                layer: "semantic",
                content: CONTENT,
                actor_id: writer_id,
                actor_type: "agent",
                actor_handle: "writer",
                summary: Some("survive request cancellation"),
                precondition: pages::Precondition::None,
            },
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
    .expect("write must reach Git while the projection connection is blocked");
    assert!(gated.load(Ordering::SeqCst));
    assert!(
        !request.is_finished(),
        "the operation must wait for the database writer, not fail after committing Git",
    );
    request.abort();
    assert!(request
        .await
        .expect_err("request task must be cancelled")
        .is_cancelled());
    release.notify_one();

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
    write_pool.close().await;
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
        pages::WriteRequest {
            path,
            layer,
            content: "# Rejected\n\nrejectedcontenttoken\n",
            actor_id,
            actor_type: "agent",
            actor_handle: "writer",
            summary: Some("must not be committed"),
            precondition: pages::Precondition::None,
        },
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
                pages::WriteRequest {
                    path,
                    layer: "semantic",
                    content: "# Unattributable\n\ninvalidactorcontenttoken\n",
                    actor_id: id,
                    actor_type,
                    actor_handle: handle,
                    summary: Some("must not be committed"),
                    precondition: pages::Precondition::None,
                },
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

async fn save(
    fixture: &Fixture,
    path: &str,
    layer: &str,
    content: &str,
    actor: i64,
    precondition: pages::Precondition,
) -> wikikiki::error::Result<pages::PageMeta> {
    pages::write(
        &fixture.pool,
        &fixture.repo,
        &fixture.content_root,
        pages::WriteRequest {
            path,
            layer,
            content,
            actor_id: actor,
            actor_type: "agent",
            actor_handle: "writer",
            summary: Some("test edit"),
            precondition,
        },
    )
    .await
}

#[tokio::test]
async fn metadata_only_changes_advance_revision_and_history() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    let first = save(
        &fixture,
        "page",
        "semantic",
        "# Same bytes",
        actor,
        pages::Precondition::Absent,
    )
    .await
    .expect("first write");
    let second = save(
        &fixture,
        "page",
        "episodic",
        "# Same bytes",
        actor,
        pages::Precondition::Revision(first.revision.clone().unwrap()),
    )
    .await
    .expect("metadata edit");
    assert_ne!(first.revision, second.revision);
    let before = fixture.snapshot("page").await;
    assert!(matches!(
        save(
            &fixture,
            "page",
            "semantic",
            "stale overwrite",
            actor,
            pages::Precondition::Revision(first.revision.unwrap())
        )
        .await,
        Err(wikikiki::error::AppError::PreconditionFailed(_))
    ));
    assert_eq!(before, fixture.snapshot("page").await);
    assert_eq!(
        pages::history(&fixture.repo, &fixture.content_root, "page", 10)
            .await
            .unwrap()
            .len(),
        2
    );
    let layers: Vec<String> = sqlx::query_scalar("SELECT layer FROM page_edits ORDER BY id")
        .fetch_all(&fixture.pool)
        .await
        .unwrap();
    assert_eq!(layers, ["semantic", "episodic"]);
}

#[tokio::test]
async fn case_aliases_and_nonportable_names_cannot_change_any_store() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    fixture.seed_page("Topic", actor, "writer").await;
    fixture.seed_page("Notes/one", actor, "writer").await;
    let before = fixture.snapshot("Topic").await;
    for path in [
        "topic",
        "notes/two",
        "NUL",
        "CON.txt",
        "COM1/child",
        "trailing./page",
        ".git/config",
        "Topic.md/child",
    ] {
        assert!(
            save(
                &fixture,
                path,
                "semantic",
                "must reject",
                actor,
                pages::Precondition::None
            )
            .await
            .is_err(),
            "accepted {path}"
        );
        assert_eq!(
            before,
            fixture.snapshot("Topic").await,
            "changed state for {path}"
        );
    }
    assert!(pages::read(&fixture.pool, &fixture.content_root, "topic")
        .await
        .is_err());
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_page_writes")
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
    assert_eq!(intents, 0, "rejected writes must not later be replayed");
}

#[tokio::test]
async fn malformed_legacy_author_is_rejected_before_acceptance() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    fixture.seed_page("page", actor, "writer").await;
    sqlx::query("UPDATE actors SET handle = 'bad>handle' WHERE id = ?")
        .bind(actor)
        .execute(&fixture.pool)
        .await
        .unwrap();
    let before = fixture.snapshot("page").await;
    let result = pages::write(
        &fixture.pool,
        &fixture.repo,
        &fixture.content_root,
        pages::WriteRequest {
            path: "page",
            layer: "semantic",
            content: "must reject",
            actor_id: actor,
            actor_type: "agent",
            actor_handle: "bad>handle",
            summary: None,
            precondition: pages::Precondition::None,
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(wikikiki::error::AppError::BadRequest(_))
    ));
    assert_eq!(before, fixture.snapshot("page").await);
    assert_eq!(
        pages::recover_pending(&fixture.pool, &fixture.repo, &fixture.content_root)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn nested_content_history_and_historical_reads_use_the_same_mapping() {
    let mut fixture = Fixture::new().await;
    fixture.content_root = fixture.repo.root().join("nested/content");
    let actor = fixture.actor("writer").await;
    save(
        &fixture,
        "notes/page",
        "semantic",
        "first version",
        actor,
        pages::Precondition::None,
    )
    .await
    .unwrap();
    let first = fixture.head();
    save(
        &fixture,
        "notes/page",
        "semantic",
        "second version",
        actor,
        pages::Precondition::None,
    )
    .await
    .unwrap();
    let history = pages::history(&fixture.repo, &fixture.content_root, "notes/page", 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        pages::read_revision(&fixture.repo, &fixture.content_root, "notes/page", &first)
            .await
            .unwrap(),
        "first version"
    );
    for path in ["../.keep", ".git/config", "/notes/page"] {
        assert!(
            pages::read_revision(&fixture.repo, &fixture.content_root, path, &first)
                .await
                .is_err()
        );
    }
    for revision in ["HEAD", "abc", "../../config"] {
        assert!(
            pages::read_revision(&fixture.repo, &fixture.content_root, "notes/page", revision)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn summary_trailers_cannot_add_an_unrelated_page_to_history() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    fixture.seed_page("victim", actor, "writer").await;
    let summary = format!(
        "unrelated edit\n\nWikikiki-Operation: {}\nWikikiki-Path: victim.md",
        "a".repeat(64)
    );
    pages::write(
        &fixture.pool,
        &fixture.repo,
        &fixture.content_root,
        pages::WriteRequest {
            path: "other",
            layer: "semantic",
            content: "unrelated",
            actor_id: actor,
            actor_type: "agent",
            actor_handle: "writer",
            summary: Some(&summary),
            precondition: pages::Precondition::None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        pages::history(&fixture.repo, &fixture.content_root, "victim", 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn punctuation_search_and_unicode_stale_revisions_are_regular_results() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    fixture.seed_page("page", actor, "writer").await;
    for query in ["!!!", "\"*():", "---", "___", ""] {
        assert!(pages::search(&fixture.pool, query, 10)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(matches!(
        save(
            &fixture,
            "page",
            "semantic",
            "stale",
            actor,
            pages::Precondition::Revision("😀😀😀😀😀😀😀😀😀😀😀😀😀".into())
        )
        .await,
        Err(wikikiki::error::AppError::PreconditionFailed(_))
    ));
}

#[tokio::test]
async fn failed_projection_recovers_after_restart_without_duplicate_commit() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    fixture.seed_page("page", actor, "writer").await;
    let old = pages::read(&fixture.pool, &fixture.content_root, "page")
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_edit BEFORE INSERT ON page_edits BEGIN SELECT RAISE(ABORT, 'injected projection failure'); END")
        .execute(&fixture.pool).await.unwrap();
    assert!(save(
        &fixture,
        "page",
        "episodic",
        "# Recovered\n\nrecoveredtoken",
        actor,
        pages::Precondition::None
    )
    .await
    .is_err());
    let committed = fixture.head();
    let visible = pages::read(&fixture.pool, &fixture.content_root, "page")
        .await
        .unwrap();
    assert_eq!(
        visible.content, old.content,
        "failed projection must not mix new disk bytes with old metadata"
    );
    assert_eq!(visible.meta.revision, old.meta.revision);
    assert!(pages::search(&fixture.pool, "recoveredtoken", 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pending_page_writes")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("DROP TRIGGER fail_edit")
        .execute(&fixture.pool)
        .await
        .unwrap();
    let Fixture {
        pool,
        repo,
        content_root,
        _tmp,
    } = fixture;
    pool.close().await;
    drop(repo);
    let pool = connect_and_migrate(&_tmp.path().join("wiki.db"))
        .await
        .unwrap();
    let repo = Repo::open_or_init(&content_root, AUTHOR_TEMPLATE).unwrap();
    assert_eq!(
        pages::recover_pending(&pool, &repo, &content_root)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        pages::recover_pending(&pool, &repo, &content_root)
            .await
            .unwrap(),
        0
    );
    let current_head = gix::open(repo.root())
        .unwrap()
        .head_id()
        .unwrap()
        .detach()
        .to_string();
    assert_eq!(
        current_head, committed,
        "replay must reuse the immutable commit"
    );
    let page = pages::read(&pool, &content_root, "page").await.unwrap();
    assert_eq!(page.content, "# Recovered\n\nrecoveredtoken");
    assert_eq!(page.meta.layer, "episodic");
    assert_eq!(page.meta.last_edit_by, Some(actor));
    assert_eq!(
        std::fs::read_to_string(content_root.join("page.md")).unwrap(),
        page.content
    );
    assert_eq!(
        repo.file_at_commit(&committed, Path::new("page.md"))
            .await
            .unwrap()
            .unwrap(),
        page.content
    );
    assert_eq!(
        pages::search(&pool, "recoveredtoken", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM page_edits")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    pool.close().await;
}

// This models a process exit immediately after durable intent insertion,
// before any Git object or working file was changed.
async fn persist_uncommitted_intent(fixture: &Fixture, actor: i64) {
    let content = "# Accepted before restart\n\nintenttoken";
    sqlx::query("INSERT INTO pending_page_writes
        (operation_id, page_path, repository_path, layer, content, content_hash, actor_id,
         actor_type, actor_handle, author_signature, summary, created_at, base_commit, previous_content_hash)
        VALUES (?, 'pending', 'pending.md', 'semantic', ?, ?, ?, 'agent', 'writer', ?, 'accepted intent', ?, ?, NULL)")
        .bind("b".repeat(64)).bind(content).bind(hex::encode(Sha256::digest(content.as_bytes())))
        .bind(actor).bind("agent:writer <writer@wikikiki.local>")
        .bind(wikikiki::db::now_ts()).bind(fixture.head()).execute(&fixture.pool).await.unwrap();
}

#[tokio::test]
async fn precommit_intent_recovers_and_refuses_changed_root_or_external_files() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    persist_uncommitted_intent(&fixture, actor).await;
    let before = fixture.head();
    assert!(pages::recover_pending(
        &fixture.pool,
        &fixture.repo,
        &fixture.content_root.join("different")
    )
    .await
    .is_err());
    std::fs::write(fixture.content_root.join("pending.md"), "independent file").unwrap();
    assert!(
        pages::recover_pending(&fixture.pool, &fixture.repo, &fixture.content_root)
            .await
            .is_err()
    );
    assert_eq!(fixture.head(), before);
    assert_eq!(
        std::fs::read_to_string(fixture.content_root.join("pending.md")).unwrap(),
        "independent file"
    );
    std::fs::remove_file(fixture.content_root.join("pending.md")).unwrap();
    assert_eq!(
        pages::recover_pending(&fixture.pool, &fixture.repo, &fixture.content_root)
            .await
            .unwrap(),
        1
    );
    assert_ne!(fixture.head(), before);
    assert_eq!(
        pages::read(&fixture.pool, &fixture.content_root, "pending")
            .await
            .unwrap()
            .content,
        "# Accepted before restart\n\nintenttoken"
    );
    assert_eq!(
        pages::history(&fixture.repo, &fixture.content_root, "pending", 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn legacy_drift_is_rejected_and_independent_repository_writers_are_locked_out() {
    let fixture = Fixture::new().await;
    let actor = fixture.actor("writer").await;
    fixture.seed_page("legacy", actor, "writer").await;
    sqlx::query(
        "UPDATE pages SET projection_version = 0, revision = content_hash WHERE path = 'legacy'",
    )
    .execute(&fixture.pool)
    .await
    .unwrap();
    assert!(pages::read(&fixture.pool, &fixture.content_root, "legacy")
        .await
        .is_ok());
    std::fs::write(fixture.content_root.join("legacy.md"), "external drift").unwrap();
    assert!(pages::read(&fixture.pool, &fixture.content_root, "legacy")
        .await
        .is_err());
    assert!(save(
        &fixture,
        "legacy",
        "semantic",
        "overwrite",
        actor,
        pages::Precondition::None
    )
    .await
    .is_err());
    assert!(Repo::open_or_init(&fixture.content_root, AUTHOR_TEMPLATE).is_err());
}

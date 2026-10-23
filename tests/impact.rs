//! Recorded reads and the affected-work view, over real HTTP.
//!
//! The claim under test is narrow: the server records which actor was served
//! which revision, and can list what that actor wrote between reading an old
//! revision and reading the current one. Nothing here asserts that a listed
//! page actually depends on the inspected one — that is inference the view
//! labels as such.

use std::net::SocketAddr;

use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::net::TcpListener;

use wikikiki::actor::{bootstrap_admin, issue_actor_token};
use wikikiki::config::Config;
use wikikiki::db::connect_and_migrate;
use wikikiki::git::Repo;
use wikikiki::web::{router, AppState};

struct Harness {
    base: String,
    _server_task: tokio::task::JoinHandle<()>,
    _tmp: TempDir,
    pool: SqlitePool,
}

async fn spawn() -> Harness {
    let tmp = TempDir::new().expect("tempdir");
    let content = tmp.path().join("content");
    let db = tmp.path().join("wikikiki.db");

    let mut cfg = Config::default();
    cfg.paths.content_root = content.clone();
    cfg.paths.db_path = db.clone();
    cfg.paths.git_repo = content.clone();
    cfg.server.bind = "127.0.0.1:0".into();
    cfg.ui.allow_anonymous_read = true;
    cfg.ui.require_auth_for_write = true;

    let pool = connect_and_migrate(&db).await.expect("migrate");
    let repo = Repo::open_or_init(&content, &cfg.git.author_template).expect("git init");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    cfg.server.public_url = format!("http://{addr}");
    let state = AppState::new(pool.clone(), repo, cfg);
    let app = router(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    Harness {
        base: format!("http://{addr}"),
        _server_task: task,
        _tmp: tmp,
        pool,
    }
}

struct Actors {
    human: String,
    agent: String,
}

async fn actors(h: &Harness) -> Actors {
    let admin_id = bootstrap_admin(&h.pool, "alice", Some("Alice"), "hunter2-hunter2")
        .await
        .expect("bootstrap");
    let (_, human) = issue_actor_token(&h.pool, "alice", "human", None, None, None, admin_id)
        .await
        .expect("human token");
    let (_, agent) = issue_actor_token(
        &h.pool,
        "planner",
        "agent",
        Some("Planner"),
        None,
        None,
        admin_id,
    )
    .await
    .expect("agent token");
    Actors { human, agent }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().expect("client")
}

async fn put(h: &Harness, token: &str, path: &str, body: &str) -> String {
    let resp = client()
        .put(format!("{}/api/pages/{path}", h.base))
        .bearer_auth(token)
        .header("Content-Type", "text/markdown")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "PUT {path}");
    let v: serde_json::Value = resp.json().await.unwrap();
    v["revision"].as_str().unwrap().to_string()
}

async fn get(h: &Harness, token: Option<&str>, path: &str) {
    let mut req = client().get(format!("{}/api/pages/{path}", h.base));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200, "GET {path}");
}

async fn impact(h: &Harness, token: &str, path: &str) -> serde_json::Value {
    let resp = client()
        .get(format!("{}/api/impact/{path}", h.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "impact {path}");
    resp.json().await.unwrap()
}

/// Timestamps are whole seconds and the window is `>= first_old_read`, so a
/// write in the same second as the read is inside it; the read must precede
/// the write by a full tick for the ordering to be the one under test.
async fn tick() {
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
}

#[tokio::test]
async fn stale_reader_and_later_writes_are_listed_then_cleared_by_a_current_read() {
    let h = spawn().await;
    let a = actors(&h).await;

    let rev1 = put(
        &h,
        &a.human,
        "premise",
        "# Premise\n\nCustomers run it themselves.\n",
    )
    .await;
    get(&h, Some(&a.agent), "premise").await;
    tick().await;
    put(
        &h,
        &a.agent,
        "plans/arch",
        "# Arch\n\nUses a hosted service.\n",
    )
    .await;
    tick().await;
    let rev2 = put(
        &h,
        &a.human,
        "premise",
        "# Premise\n\nNo publisher-operated service.\n",
    )
    .await;
    assert_ne!(rev1, rev2);

    let v = impact(&h, &a.human, "premise").await;
    assert_eq!(v["revision"], rev2);
    let readers = v["readers"].as_array().unwrap();
    assert_eq!(readers.len(), 1, "{v}");
    let r = &readers[0];
    assert_eq!(r["handle"], "planner");
    assert_eq!(r["actor_type"], "agent");
    assert_eq!(r["last_old_revision"], rev1);
    assert!(r["caught_up_at"].is_null(), "{r}");
    let writes = r["later_writes"].as_array().unwrap();
    assert_eq!(writes.len(), 1, "{r}");
    assert_eq!(writes[0]["path"], "plans/arch");
    assert_eq!(r["later_writes_total"], 1);

    // The agent reads the new revision. It is now caught up, and the plan it
    // wrote in between stays listed — that write still predates the read.
    tick().await;
    get(&h, Some(&a.agent), "premise").await;
    tick().await;
    put(&h, &a.agent, "plans/after", "# After\n\nrevised\n").await;

    let v = impact(&h, &a.human, "premise").await;
    let r = &v["readers"][0];
    assert!(r["caught_up_at"].is_number(), "{r}");
    let paths: Vec<_> = r["later_writes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(paths, vec!["plans/arch"], "{r}");
}

#[tokio::test]
async fn an_actor_that_only_read_the_current_revision_is_not_a_stale_reader() {
    let h = spawn().await;
    let a = actors(&h).await;
    put(&h, &a.human, "premise", "# Premise\n\nv1\n").await;
    get(&h, Some(&a.agent), "premise").await;

    let v = impact(&h, &a.human, "premise").await;
    assert!(v["readers"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["current_only"][0]["handle"], "planner");
}

#[tokio::test]
async fn anonymous_reads_leave_no_row() {
    let h = spawn().await;
    let a = actors(&h).await;
    put(&h, &a.human, "premise", "# Premise\n\nv1\n").await;
    get(&h, None, "premise").await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM page_reads")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn repeated_reads_of_one_revision_coalesce() {
    let h = spawn().await;
    let a = actors(&h).await;
    put(&h, &a.human, "premise", "# Premise\n\nv1\n").await;
    for _ in 0..5 {
        get(&h, Some(&a.agent), "premise").await;
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM page_reads")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // A new revision is a new row, so "last read an older version" is exact.
    put(&h, &a.human, "premise", "# Premise\n\nv2\n").await;
    get(&h, Some(&a.agent), "premise").await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM page_reads")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
}

/// The editor of the current revision has not "read" it through GET, but it
/// produced the text. It must not be reported as behind its own edit.
#[tokio::test]
async fn authoring_the_current_revision_counts_as_caught_up() {
    let h = spawn().await;
    let a = actors(&h).await;
    put(&h, &a.human, "premise", "# Premise\n\nv1\n").await;
    get(&h, Some(&a.agent), "premise").await;
    tick().await;
    put(&h, &a.agent, "premise", "# Premise\n\nv2 by agent\n").await;

    let v = impact(&h, &a.human, "premise").await;
    let r = &v["readers"][0];
    assert_eq!(r["handle"], "planner");
    assert!(r["caught_up_at"].is_number(), "{r}");
    // Its own write of this page is not "other work".
    assert_eq!(r["later_writes_total"], 0, "{r}");
}

#[tokio::test]
async fn the_browser_view_renders_readers_and_states_its_limits() {
    let h = spawn().await;
    let a = actors(&h).await;
    put(&h, &a.human, "premise", "# Premise\n\nv1\n").await;
    get(&h, Some(&a.agent), "premise").await;
    tick().await;
    put(&h, &a.agent, "plans/arch", "# Arch plan\n\nx\n").await;
    tick().await;
    put(&h, &a.human, "premise", "# Premise\n\nv2\n").await;

    let resp = client()
        .get(format!("{}/wiki-impact/premise", h.base))
        .bearer_auth(&a.human)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("planner"), "{body}");
    assert!(body.contains("not read current"), "{body}");
    assert!(body.contains("Arch plan"), "{body}");
    assert!(body.contains("inferred from timing"), "{body}");

    // Inspecting the list is not a read of the page.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM page_reads WHERE actor_id = (SELECT id FROM actors WHERE handle = 'alice')")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    let missing = client()
        .get(format!("{}/wiki-impact/nope", h.base))
        .bearer_auth(&a.human)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);
}

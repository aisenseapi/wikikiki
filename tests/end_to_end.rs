//! End-to-end integration tests for Stage 1.
//!
//! These exercise the real router against a real SQLite + git repo in a tempdir.
//! No mocking — Stage 1's correctness *is* the interaction between disk, git,
//! DB and FTS, so mocks would test something that isn't the product.

use std::net::SocketAddr;
use std::path::PathBuf;

use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::net::TcpListener;

use wikikiki::actor::{bootstrap_admin, issue_actor_token};
use wikikiki::config::Config;
use wikikiki::db::{connect_and_migrate, queries};
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

    let state = AppState::new(pool.clone(), repo, cfg);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = router(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    // Tiny grace period for the server task to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    Harness {
        base: format!("http://{addr}"),
        _server_task: task,
        _tmp: tmp,
        pool,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .expect("client")
}

#[tokio::test]
async fn anonymous_read_when_allowed() {
    let h = spawn().await;
    let resp = client().get(format!("{}/", h.base)).send().await.unwrap();
    assert!(resp.status().is_success(), "home should 200 for anon read");
    let body = resp.text().await.unwrap();
    assert!(body.contains("wikikiki"));
}

#[tokio::test]
async fn anonymous_write_denied() {
    let h = spawn().await;
    let resp = client()
        .put(format!("{}/api/pages/welcome", h.base))
        .header("Content-Type", "text/markdown")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401, "anon PUT must 401");
}

#[tokio::test]
async fn agent_token_write_then_read() {
    let h = spawn().await;

    // Bootstrap admin + issue an agent token via the library API.
    let admin_id = bootstrap_admin(&h.pool, "alice", Some("Alice"), "hunter2-hunter2")
        .await
        .expect("bootstrap");
    let (_, token) = issue_actor_token(
        &h.pool,
        "researcher",
        "agent",
        Some("Researcher"),
        Some("test"),
        None,
        admin_id,
    )
    .await
    .expect("issue");

    let url = format!("{}/api/pages/notes/from-agent", h.base);
    let put = client()
        .put(&url)
        .bearer_auth(&token)
        .header("Content-Type", "text/markdown")
        .body("# from agent\n\nhello world\n")
        .send()
        .await
        .unwrap();
    assert_eq!(put.status().as_u16(), 200, "agent write should 200");
    let j: serde_json::Value = put.json().await.unwrap();
    assert_eq!(j["ok"], true);
    assert_eq!(j["path"], "notes/from-agent");

    // Read it back — anon read allowed by default.
    let get = client().get(&url).send().await.unwrap();
    assert!(get.status().is_success());
    let body = get.text().await.unwrap();
    assert!(body.contains("hello world"));

    // And as JSON.
    let get_json = client()
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = get_json.json().await.unwrap();
    assert_eq!(v["path"], "notes/from-agent");
    assert_eq!(v["title"], "from agent");
    assert!(v["content"].as_str().unwrap().contains("hello world"));
}

#[tokio::test]
async fn human_login_then_write_via_ui() {
    let h = spawn().await;
    bootstrap_admin(&h.pool, "bob", Some("Bob"), "correct-horse-battery-staple")
        .await
        .expect("bootstrap");

    let cli = client();

    // Log in.
    let form = [("handle", "bob"), ("password", "correct-horse-battery-staple")];
    let login = cli
        .post(format!("{}/login", h.base))
        .form(&form)
        .send()
        .await
        .unwrap();
    assert!(
        login.status().is_success() || login.status().is_redirection(),
        "login should land on home: got {}",
        login.status()
    );

    // Save a page via the UI form.
    let save = cli
        .post(format!("{}/wiki/welcome", h.base))
        .form(&[
            ("content", "# Welcome\n\nThis wiki is the memory.\n"),
            ("summary", "first page"),
            ("layer", "semantic"),
        ])
        .send()
        .await
        .unwrap();
    assert!(
        save.status().is_success() || save.status().is_redirection(),
        "save should redirect or 200: got {}",
        save.status()
    );

    // Read it.
    let view = cli
        .get(format!("{}/wiki/welcome", h.base))
        .send()
        .await
        .unwrap();
    assert!(view.status().is_success());
    let body = view.text().await.unwrap();
    assert!(body.contains("This wiki is the memory"));

    // FTS search picks it up.
    let search = cli
        .get(format!("{}/api/search?q=memory", h.base))
        .send()
        .await
        .unwrap();
    assert!(search.status().is_success());
    let v: serde_json::Value = search.json().await.unwrap();
    let hits = v["hits"].as_array().expect("hits array");
    assert!(
        hits.iter().any(|h| h["path"] == "welcome"),
        "should find welcome page in search"
    );

    // Attribution: page_edits should have a row tied to the agent.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM page_edits")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(count.0 >= 1, "page_edits should have at least one row");

    // Audit: bootstrap should have written an admin_audit entry.
    let admin_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admin_audit WHERE operation = 'bootstrap'")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(admin_count.0 >= 1, "admin_audit should record the bootstrap");

    // Access log: login_success should be recorded.
    let access_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM access_log WHERE outcome = 'login_success'")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(access_count.0 >= 1, "access_log should record the login");
}

#[tokio::test]
async fn invalid_paths_rejected() {
    let h = spawn().await;
    let admin_id = bootstrap_admin(&h.pool, "carol", None, "passwordpassword")
        .await
        .unwrap();
    let (_, token) = issue_actor_token(&h.pool, "writer", "agent", None, None, None, admin_id)
        .await
        .unwrap();

    // `a%2F..%2Fb` instead of `a/../b`: percent-encoded slashes survive
    // reqwest's RFC 3986 URL normalisation, so the bad path actually reaches
    // the validator instead of being collapsed to `b` client-side.
    let bad_paths = ["../escape", "a%2F..%2Fb", "with spaces", "/leading"];
    for p in bad_paths {
        // URL-encode minimally — the router still receives our path.
        let resp = client()
            .put(format!("{}/api/pages/{}", h.base, p))
            .bearer_auth(&token)
            .body("hi")
            .send()
            .await
            .unwrap();
        assert!(
            resp.status().is_client_error(),
            "{p:?} should be rejected, got {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn revoked_actor_blocked() {
    let h = spawn().await;
    let admin_id = bootstrap_admin(&h.pool, "dave", None, "passwordpassword")
        .await
        .unwrap();
    let (actor_id, token) = issue_actor_token(&h.pool, "bot", "agent", None, None, None, admin_id)
        .await
        .unwrap();

    // Revoke the actor.
    sqlx::query("UPDATE actors SET is_active = 0 WHERE id = ?")
        .bind(actor_id)
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = client()
        .put(format!("{}/api/pages/from-revoked", h.base))
        .bearer_auth(&token)
        .body("nope")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401, "revoked actor must be denied");

    // Sanity: actor row is gone-but-not-gone.
    let a = queries::actor_by_id(&h.pool, actor_id).await.unwrap().unwrap();
    assert!(!a.is_active);
}

// Keeps the unused-import lint quiet on platforms without certain reqwest features.
#[allow(dead_code)]
fn _path_witness(_: PathBuf) {}

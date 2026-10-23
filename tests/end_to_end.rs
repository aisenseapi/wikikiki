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
    spawn_with_anonymous(true).await
}

async fn spawn_with_anonymous(allow_anonymous_read: bool) -> Harness {
    let tmp = TempDir::new().expect("tempdir");
    let content = tmp.path().join("content");
    let db = tmp.path().join("wikikiki.db");

    let mut cfg = Config::default();
    cfg.paths.content_root = content.clone();
    cfg.paths.db_path = db.clone();
    cfg.paths.git_repo = content.clone();
    cfg.server.bind = "127.0.0.1:0".into();
    cfg.ui.allow_anonymous_read = allow_anonymous_read;
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
    let form = [
        ("handle", "bob"),
        ("password", "correct-horse-battery-staple"),
    ];
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
    let admin_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM admin_audit WHERE operation = 'bootstrap'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert!(
        admin_count.0 >= 1,
        "admin_audit should record the bootstrap"
    );

    // Access log: login_success should be recorded.
    let access_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM access_log WHERE outcome = 'login_success'")
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
    let a = queries::actor_by_id(&h.pool, actor_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!a.is_active);
}

// Keeps the unused-import lint quiet on platforms without certain reqwest features.
#[allow(dead_code)]
fn _path_witness(_: PathBuf) {}

// ===========================================================================
// Conditional writes
//
// The property under test is that an actor cannot erase a change it never
// saw. Everything else here — ETag plumbing, 412 codes — exists to serve it.
// ===========================================================================

/// Bootstrap an admin and mint an agent token in one step.
async fn agent_token(h: &Harness, admin: &str, agent: &str) -> String {
    let admin_id = bootstrap_admin(&h.pool, admin, None, "passwordpassword")
        .await
        .expect("bootstrap");
    let (_, token) = issue_actor_token(&h.pool, agent, "agent", None, None, None, admin_id)
        .await
        .expect("issue");
    token
}

fn etag_of(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::ETAG)
        .expect("response carries an ETag")
        .to_str()
        .expect("ascii etag")
        .to_string()
}

#[tokio::test]
async fn get_exposes_a_revision_that_a_conditional_write_accepts() {
    let h = spawn().await;
    let token = agent_token(&h, "admin1", "writer1").await;
    let url = format!("{}/api/pages/conditional", h.base);
    let cli = client();

    cli.put(&url)
        .bearer_auth(&token)
        .body("# One\n\nfirst\n")
        .send()
        .await
        .unwrap();

    let get = cli.get(&url).send().await.unwrap();
    let etag = etag_of(&get);
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");

    // The revision just read is the one a write may name.
    let ok = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-Match", &etag)
        .body("# Two\n\nsecond\n")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status().as_u16(), 200, "matching If-Match must succeed");

    // ...and it is stale immediately afterwards.
    let stale = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-Match", &etag)
        .body("# Three\n\nthird\n")
        .send()
        .await
        .unwrap();
    assert_eq!(
        stale.status().as_u16(),
        412,
        "stale If-Match must be refused"
    );

    // The refused write changed nothing.
    let body = cli.get(&url).send().await.unwrap().text().await.unwrap();
    assert!(
        body.contains("second"),
        "refused write must not land: {body}"
    );
    assert!(!body.contains("third"));
}

#[tokio::test]
async fn if_none_match_star_creates_without_overwriting() {
    let h = spawn().await;
    let token = agent_token(&h, "admin2", "writer2").await;
    let url = format!("{}/api/pages/create-once", h.base);
    let cli = client();

    let created = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-None-Match", "*")
        .body("# Created\n\noriginal\n")
        .send()
        .await
        .unwrap();
    assert_eq!(created.status().as_u16(), 200);

    let second = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-None-Match", "*")
        .body("# Clobbered\n\nreplacement\n")
        .send()
        .await
        .unwrap();
    assert_eq!(second.status().as_u16(), 412, "must not overwrite");

    let body = cli.get(&url).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("original"), "{body}");
}

/// Legacy clients without conditional headers keep their explicit overwrite behavior.
#[tokio::test]
async fn writes_without_conditions_are_unaffected() {
    let h = spawn().await;
    let token = agent_token(&h, "admin3", "writer3").await;
    let url = format!("{}/api/pages/unconditional", h.base);
    let cli = client();

    for body in ["# A\n\nfirst\n", "# B\n\nsecond\n", "# C\n\nthird\n"] {
        let resp = cli
            .put(&url)
            .bearer_auth(&token)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200, "unconditional write must land");
    }

    let body = cli.get(&url).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("third"), "{body}");
}

#[tokio::test]
async fn malformed_conditional_headers_are_rejected_rather_than_ignored() {
    let h = spawn().await;
    let token = agent_token(&h, "admin4", "writer4").await;
    let url = format!("{}/api/pages/malformed", h.base);
    let cli = client();

    // Silently ignoring these would drop a guarantee the caller asked for.
    let cases: [(&str, &str); 3] = [
        ("If-Match", "not-a-quoted-tag"),
        ("If-Match", "\"aaa\", \"bbb\""),
        ("If-None-Match", "\"some-tag\""),
    ];
    for (name, value) in cases {
        let resp = cli
            .put(&url)
            .bearer_auth(&token)
            .header(name, value)
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            400,
            "{name}: {value} should be a bad request"
        );
    }

    let invalid = reqwest::header::HeaderValue::from_bytes(b"\"\xff\"").unwrap();
    let response = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-Match", invalid)
        .body("must not create")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let mut duplicates = reqwest::header::HeaderMap::new();
    duplicates.append("If-Match", "*".parse().unwrap());
    duplicates.append("If-Match", "\"another\"".parse().unwrap());
    let response = cli
        .put(&url)
        .bearer_auth(&token)
        .headers(duplicates)
        .body("must not create")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(cli.get(&url).send().await.unwrap().status().as_u16(), 404);
}

#[tokio::test]
async fn metadata_only_edit_invalidates_previous_revision() {
    let h = spawn().await;
    let token = agent_token(&h, "metadata-admin", "metadata-writer").await;
    let cli = client();
    let url = format!("{}/api/pages/metadata", h.base);
    let created = cli
        .put(&url)
        .bearer_auth(&token)
        .body("# Same body\n")
        .send()
        .await
        .unwrap();
    let original = etag_of(&created);
    let changed = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-Match", &original)
        .header("X-Layer", "episodic")
        .body("# Same body\n")
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status().as_u16(), 200);
    assert_ne!(etag_of(&changed), original);
    let stale = cli
        .put(&url)
        .bearer_auth(&token)
        .header("If-Match", &original)
        .body("stale overwrite")
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status().as_u16(), 412);
    let json: serde_json::Value = cli
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json["layer"], "episodic");
    assert_eq!(json["content"], "# Same body\n");
}

#[tokio::test]
async fn private_workspace_exposes_health_but_not_content() {
    let h = spawn_with_anonymous(false).await;
    let cli = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let home = cli.get(&h.base).send().await.unwrap();
    assert_eq!(home.status().as_u16(), 303);
    assert_eq!(home.headers()["location"], "/login");
    for path in [
        "/api/pages/private",
        "/api/search?q=secret",
        "/actors",
        "/guide",
        "/new",
    ] {
        assert_eq!(
            cli.get(format!("{}{path}", h.base))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16(),
            401,
            "{path}"
        );
    }
    let health = cli.get(format!("{}/healthz", h.base)).send().await.unwrap();
    assert_eq!(health.status().as_u16(), 200);
    assert_eq!(health.headers()["cache-control"], "no-store");
    assert!(health.text().await.unwrap().is_empty());
}

#[tokio::test]
async fn new_page_template_stays_unpublished_until_conditional_save() {
    let h = spawn().await;
    let token = agent_token(&h, "workspace-admin", "workspace-writer").await;
    let cli = client();
    let editor = cli
        .get(format!("{}/new?path=tasks/next&template=handoff", h.base))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(editor.status().as_u16(), 200);
    let editor = editor.text().await.unwrap();
    assert!(editor.contains("## Current state and evidence"));
    assert!(editor.contains("name=\"revision\" value=\"\""));
    let url = format!("{}/api/pages/tasks/next", h.base);
    assert_eq!(cli.get(&url).send().await.unwrap().status().as_u16(), 404);
    cli.put(&url)
        .bearer_auth(&token)
        .body("# Someone else created it\n")
        .send()
        .await
        .unwrap();
    let conflict = cli
        .post(format!("{}/wiki/tasks/next", h.base))
        .bearer_auth(&token)
        .form(&[("content", "My unsaved handoff"), ("revision", "")])
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status().as_u16(), 412);
    assert!(conflict
        .text()
        .await
        .unwrap()
        .contains("My unsaved handoff"));
    let unicode = cli
        .post(format!("{}/wiki/tasks/next", h.base))
        .bearer_auth(&token)
        .form(&[
            ("content", "Unicode revision draft"),
            ("revision", "aaaaaaaaaaaé"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(unicode.status().as_u16(), 412);
    assert!(unicode
        .text()
        .await
        .unwrap()
        .contains("Unicode revision draft"));
    assert!(cli
        .get(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
        .contains("Someone else"));
}

#[tokio::test]
async fn punctuation_search_is_empty_and_login_attempts_are_limited() {
    let h = spawn().await;
    let cli = client();
    let search = cli
        .get(format!("{}/api/search?q=!!!", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status().as_u16(), 200);
    assert_eq!(
        search.json::<serde_json::Value>().await.unwrap()["hits"],
        serde_json::json!([])
    );
    for _ in 0..5 {
        let login = cli
            .post(format!("{}/login", h.base))
            .form(&[("handle", "unknown"), ("password", "invalid")])
            .send()
            .await
            .unwrap();
        assert_eq!(login.status().as_u16(), 401);
    }
    let limited = cli
        .post(format!("{}/login", h.base))
        .form(&[("handle", "unknown"), ("password", "invalid")])
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status().as_u16(), 429);
    assert_eq!(limited.headers()["retry-after"], "60");
}

#[tokio::test]
async fn browser_cross_origin_writes_are_rejected() {
    let h = spawn().await;
    let token = agent_token(&h, "origin-admin", "origin-writer").await;
    let url = format!("{}/api/pages/origin", h.base);
    let cli = client();
    let refused = cli
        .put(&url)
        .bearer_auth(&token)
        .header("Origin", "https://another.example")
        .body("foreign origin")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status().as_u16(), 403);
    assert_eq!(cli.get(&url).send().await.unwrap().status().as_u16(), 404);
    let accepted = cli
        .put(&url)
        .bearer_auth(&token)
        .header("Origin", &h.base)
        .header("If-None-Match", "*")
        .body("same origin")
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status().as_u16(), 200);
}

#[tokio::test]
async fn history_links_read_immutable_versions_without_changing_current_page() {
    let h = spawn().await;
    let token = agent_token(&h, "history-admin", "history-writer").await;
    let cli = client();
    let url = format!("{}/api/pages/history-example", h.base);
    for content in [
        "# Original\n\nfirst version\n",
        "# Current\n\nsecond version\n",
    ] {
        assert_eq!(
            cli.put(&url)
                .bearer_auth(&token)
                .body(content)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16(),
            200
        );
    }
    let first: String = sqlx::query_scalar("SELECT git_commit FROM page_edits ORDER BY id LIMIT 1")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    let history = cli
        .get(format!("{}/wiki-history/history-example", h.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(history.contains(&format!("/wiki-revision/history-example?commit={first}")));
    let old = cli
        .get(format!(
            "{}/wiki-revision/history-example?commit={first}&source=1",
            h.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(old.status().as_u16(), 200);
    assert_eq!(old.text().await.unwrap(), "# Original\n\nfirst version\n");
    let json: serde_json::Value = cli
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json["content"], "# Current\n\nsecond version\n");
    assert_eq!(json["last_edit_handle"], "history-writer");
    assert_eq!(json["last_edit_type"], "agent");
    let invalid = cli
        .get(format!(
            "{}/wiki-revision/history-example?commit=HEAD",
            h.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status().as_u16(), 400);
}

#[tokio::test]
async fn an_invalid_explicit_bearer_token_never_falls_back_to_the_browser_identity() {
    let h = spawn().await;
    bootstrap_admin(&h.pool, "token-admin", None, "valid-password")
        .await
        .unwrap();
    let cli = client();
    let login = cli
        .post(format!("{}/login", h.base))
        .form(&[("handle", "token-admin"), ("password", "valid-password")])
        .send()
        .await
        .unwrap();
    assert_eq!(login.status().as_u16(), 200);
    let url = format!("{}/api/pages/identity", h.base);
    let refused = cli
        .put(&url)
        .bearer_auth("invalid-token")
        .body("wrong attribution")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status().as_u16(), 401);
    assert_eq!(cli.get(&url).send().await.unwrap().status().as_u16(), 404);
}

/// The point of the whole feature: several actors read the same revision,
/// then all write back. Exactly one may win, and the rest must be told.
#[tokio::test]
async fn concurrent_writers_sharing_a_base_revision_produce_exactly_one_winner() {
    let h = spawn().await;
    let token = agent_token(&h, "admin5", "writer5").await;
    let url = format!("{}/api/pages/contested", h.base);
    let cli = client();

    cli.put(&url)
        .bearer_auth(&token)
        .body("# Base\n\nbase\n")
        .send()
        .await
        .unwrap();
    let base = etag_of(&cli.get(&url).send().await.unwrap());

    let mut tasks = Vec::new();
    for i in 0..8 {
        let (cli, url, token, base) = (cli.clone(), url.clone(), token.clone(), base.clone());
        tasks.push(tokio::spawn(async move {
            cli.put(&url)
                .bearer_auth(&token)
                .header("If-Match", &base)
                .body(format!("# Writer {i}\n\nbody from writer {i}\n"))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }

    let mut statuses = Vec::new();
    for t in tasks {
        statuses.push(t.await.expect("join"));
    }

    let winners = statuses.iter().filter(|s| **s == 200).count();
    let refused = statuses.iter().filter(|s| **s == 412).count();
    assert_eq!(winners, 1, "exactly one writer may win: {statuses:?}");
    assert_eq!(refused, 7, "every loser must be told: {statuses:?}");

    // And the surviving page is one writer's work, not a blend.
    let body = cli.get(&url).send().await.unwrap().text().await.unwrap();
    let mentions = (0..8)
        .filter(|i| body.contains(&format!("body from writer {i}")))
        .count();
    assert_eq!(
        mentions, 1,
        "page must hold exactly one writer's text: {body}"
    );
}

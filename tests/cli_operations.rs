//! Exercise the operator's real binary against isolated configuration and
//! storage. No server, external account or production token is involved.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;
use wikikiki::actor::list_credentials;
use wikikiki::auth::{session, token};
use wikikiki::config::Config;
use wikikiki::db::{connect_and_migrate, queries};
use wikikiki::throttle::Throttle;

struct Instance {
    dir: TempDir,
    config_path: PathBuf,
    database_path: PathBuf,
}

impl Instance {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.paths.content_root = dir.path().join("content");
        cfg.paths.git_repo = cfg.paths.content_root.clone();
        cfg.paths.db_path = dir.path().join("instance.db");
        let config_path = dir.path().join("instance.toml");
        std::fs::write(&config_path, toml::to_string(&cfg).unwrap()).unwrap();
        Self {
            dir,
            config_path,
            database_path: cfg.paths.db_path,
        }
    }

    fn run(&self, args: &[&str], input: Option<&[u8]>) -> Output {
        run(self.dir.path(), &self.config_path, args, input)
    }

    fn ok(&self, args: &[&str], input: Option<&[u8]>) -> String {
        let output = self.run(args, input);
        assert!(
            output.status.success(),
            "command {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn bootstrap(&self, handle: &str) {
        self.ok(
            &["bootstrap", "--handle", handle, "--password-stdin"],
            Some(b"test-password\r\n"),
        );
    }
}

fn run(cwd: &Path, config: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wikikiki"));
    command
        .current_dir(cwd)
        .arg("--config")
        .arg(config)
        .args(args)
        .env("RUST_LOG", "error")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "WIKIKIKI_BIND",
        "WIKIKIKI_PUBLIC_URL",
        "WIKIKIKI_CONTENT_ROOT",
        "WIKIKIKI_DB_PATH",
        "WIKIKIKI_GIT_REPO",
        "WIKIKIKI_ALLOW_ANON_READ",
        "WIKIKIKI_REQUIRE_AUTH_WRITE",
    ] {
        command.env_remove(key);
    }
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn issued_token(output: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix("token:"))
        .unwrap()
        .trim()
        .to_string()
}

#[test]
fn missing_explicit_config_fails_without_creating_default_storage() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(
        dir.path(),
        &dir.path().join("missing.toml"),
        &["list-actors"],
        None,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing.toml"));
    assert!(!dir.path().join("wikikiki.db").exists());
    assert!(!dir.path().join("content").exists());
}

#[tokio::test]
async fn cli_bootstrap_issue_list_and_revoke_use_the_same_store() {
    let instance = Instance::new();
    instance.bootstrap("alice");
    let pool = connect_and_migrate(&instance.database_path).await.unwrap();
    let (_, old_session) = session::login(&pool, "alice", "test-password", 3600)
        .await
        .unwrap();
    instance.ok(
        &["bootstrap", "--handle", "alice", "--password-stdin"],
        Some(b"new-password\n"),
    );
    assert!(session::lookup(&pool, &old_session)
        .await
        .unwrap()
        .is_none());
    assert!(session::login(&pool, "alice", "new-password", 3600)
        .await
        .is_ok());
    assert!(session::login(&pool, "alice", "test-password", 3600)
        .await
        .is_err());

    let plaintext = issued_token(&instance.ok(
        &[
            "issue-token",
            "--handle",
            "worker",
            "--label",
            "CLI integration",
        ],
        None,
    ));
    assert!(token::verify(&pool, &plaintext, &Throttle::new(60))
        .await
        .unwrap()
        .is_some());
    let credentials = list_credentials(&pool, Some("worker")).await.unwrap();
    let credential_id = credentials[0].id;
    let stored_hash: String =
        sqlx::query_scalar("SELECT secret_hash FROM credentials WHERE id = ?")
            .bind(credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let listing = instance.ok(&["list-credentials", "--handle", "worker"], None);
    assert!(listing.contains("worker"));
    assert!(listing.contains("CLI integration"));
    assert!(!listing.contains(&plaintext));
    assert!(!listing.contains(&stored_hash));
    instance.ok(
        &["revoke-credential", "--id", &credential_id.to_string()],
        None,
    );
    assert!(token::verify(&pool, &plaintext, &Throttle::new(60))
        .await
        .unwrap()
        .is_none());
    assert!(list_credentials(&pool, Some("worker")).await.unwrap()[0]
        .revoked_at
        .is_some());
    pool.close().await;
}

#[tokio::test]
async fn cli_session_revocation_and_deactivation_preserve_a_recovery_admin() {
    let instance = Instance::new();
    instance.bootstrap("alice");
    instance.bootstrap("bob");
    let pool = connect_and_migrate(&instance.database_path).await.unwrap();
    let (_, alice_session) = session::login(&pool, "alice", "test-password", 3600)
        .await
        .unwrap();
    let (_, bob_session) = session::login(&pool, "bob", "test-password", 3600)
        .await
        .unwrap();
    instance.ok(&["revoke-sessions", "--handle", "bob"], None);
    assert!(session::lookup(&pool, &bob_session)
        .await
        .unwrap()
        .is_none());
    assert!(session::lookup(&pool, &alice_session)
        .await
        .unwrap()
        .is_some());
    let (_, bob_session) = session::login(&pool, "bob", "test-password", 3600)
        .await
        .unwrap();
    instance.ok(&["deactivate-actor", "--handle", "bob"], None);
    assert!(session::lookup(&pool, &bob_session)
        .await
        .unwrap()
        .is_none());
    assert!(
        !queries::actor_by_handle(&pool, "bob")
            .await
            .unwrap()
            .unwrap()
            .is_active
    );
    let output = instance.run(&["deactivate-actor", "--handle", "alice"], None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("last active human admin"));
    assert!(session::lookup(&pool, &alice_session)
        .await
        .unwrap()
        .is_some());
    pool.close().await;
}

#[tokio::test]
async fn invalid_token_lifetime_and_password_input_leave_storage_unchanged() {
    let instance = Instance::new();
    instance.bootstrap("alice");
    let pool = connect_and_migrate(&instance.database_path).await.unwrap();
    let initial_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM actors), (SELECT COUNT(*) FROM credentials), (SELECT COUNT(*) FROM admin_audit)",
    ).fetch_one(&pool).await.unwrap();
    for lifetime in [
        "18446744073709551615y",
        "9223372036854775807s",
        "0s",
        "nonsense",
    ] {
        let output = instance.run(
            &["issue-token", "--handle", "worker", "--lifetime", lifetime],
            None,
        );
        assert!(!output.status.success(), "accepted lifetime {lifetime}");
    }
    for password in [Vec::new(), vec![b'x'; 4097]] {
        let output = instance.run(
            &["bootstrap", "--handle", "alice", "--password-stdin"],
            Some(&password),
        );
        assert!(!output.status.success());
    }
    let after_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM actors), (SELECT COUNT(*) FROM credentials), (SELECT COUNT(*) FROM admin_audit)",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(initial_counts, after_counts);
    assert!(session::login(&pool, "alice", "test-password", 3600)
        .await
        .is_ok());
    pool.close().await;
}

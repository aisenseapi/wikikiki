//! Operator actions must either change identity and audit together or leave
//! both untouched. These tests inject real SQLite failures at the audit step.

use sqlx::SqlitePool;
use tempfile::TempDir;
use wikikiki::actor::{
    bootstrap_admin, deactivate_actor, issue_actor_token, list_credentials, revoke_credential,
    revoke_sessions, validate_handle,
};
use wikikiki::auth::{hash, session, token};
use wikikiki::db::{connect_and_migrate, queries};
use wikikiki::error::AppError;
use wikikiki::throttle::Throttle;

async fn database() -> (TempDir, SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = connect_and_migrate(&dir.path().join("auth.db"))
        .await
        .unwrap();
    (dir, pool)
}

async fn deny_audit(pool: &SqlitePool) {
    sqlx::query(
        "CREATE TRIGGER deny_audit BEFORE INSERT ON admin_audit
         BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn allow_audit(pool: &SqlitePool) {
    sqlx::query("DROP TRIGGER deny_audit")
        .execute(pool)
        .await
        .unwrap();
}

async fn token_works(pool: &SqlitePool, plaintext: &str) -> bool {
    token::verify(pool, plaintext, &Throttle::new(60))
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn password_reset_revokes_old_sessions_and_credentials() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "old-password")
        .await
        .unwrap();
    let (_, first_session) = session::login(&pool, "alice", "old-password", 3600)
        .await
        .unwrap();
    let second_session = session::create(&pool, alice, 3600).await.unwrap();
    assert_eq!(
        bootstrap_admin(&pool, "alice", None, "new-password")
            .await
            .unwrap(),
        alice
    );
    assert!(session::lookup(&pool, &first_session)
        .await
        .unwrap()
        .is_none());
    assert!(session::lookup(&pool, &second_session)
        .await
        .unwrap()
        .is_none());
    assert!(session::login(&pool, "alice", "old-password", 3600)
        .await
        .is_err());
    let (_, new_session) = session::login(&pool, "alice", "new-password", 3600)
        .await
        .unwrap();
    assert!(session::lookup(&pool, &new_session)
        .await
        .unwrap()
        .is_some());
    let credentials = list_credentials(&pool, Some("alice")).await.unwrap();
    assert_eq!(credentials.len(), 2);
    assert!(credentials[0].revoked_at.is_some());
    assert!(credentials[1].revoked_at.is_none());
}

#[tokio::test]
async fn failed_reset_preserves_password_session_and_actor_state() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "old-password")
        .await
        .unwrap();
    let sid = session::create(&pool, alice, 3600).await.unwrap();
    deny_audit(&pool).await;
    assert!(bootstrap_admin(&pool, "alice", None, "new-password")
        .await
        .is_err());
    assert!(session::lookup(&pool, &sid).await.unwrap().is_some());
    assert!(
        session::verify_password_login(&pool, "alice", "old-password")
            .await
            .is_ok()
    );
    assert!(
        session::verify_password_login(&pool, "alice", "new-password")
            .await
            .is_err()
    );
    assert_eq!(
        list_credentials(&pool, Some("alice")).await.unwrap().len(),
        1
    );
    let audit_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit_count, 1);

    // Failed first bootstrap must not leave an actor without a password.
    assert!(bootstrap_admin(&pool, "new-admin", None, "password")
        .await
        .is_err());
    assert!(queries::actor_by_handle(&pool, "new-admin")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn failed_token_audit_rolls_back_actor_and_token() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "password")
        .await
        .unwrap();
    deny_audit(&pool).await;
    assert!(
        issue_actor_token(&pool, "worker", "agent", None, None, None, alice)
            .await
            .is_err()
    );
    assert!(queries::actor_by_handle(&pool, "worker")
        .await
        .unwrap()
        .is_none());
    assert_eq!(list_credentials(&pool, None).await.unwrap().len(), 1);
    allow_audit(&pool).await;
    let (_, token) = issue_actor_token(
        &pool,
        "worker",
        "agent",
        None,
        Some("integration"),
        None,
        alice,
    )
    .await
    .unwrap();
    assert!(token_works(&pool, &token).await);
    let metadata =
        serde_json::to_value(list_credentials(&pool, Some("worker")).await.unwrap()).unwrap();
    assert_eq!(metadata[0]["label"], "integration");
    assert!(metadata[0].get("secret_hash").is_none());
    assert!(metadata[0].get("lookup_key").is_none());
    assert!(!metadata.to_string().contains(&token));
}

#[tokio::test]
async fn credential_revocation_is_audited_and_rolls_back_on_failure() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "password")
        .await
        .unwrap();
    let (_, plaintext) = issue_actor_token(&pool, "worker", "agent", None, None, None, alice)
        .await
        .unwrap();
    let credential = list_credentials(&pool, Some("worker"))
        .await
        .unwrap()
        .remove(0);
    deny_audit(&pool).await;
    assert!(revoke_credential(&pool, credential.id, alice)
        .await
        .is_err());
    assert!(token_works(&pool, &plaintext).await);
    allow_audit(&pool).await;
    revoke_credential(&pool, credential.id, alice)
        .await
        .unwrap();
    assert!(!token_works(&pool, &plaintext).await);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM admin_audit WHERE operation = 'revoke_credential'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}

#[tokio::test]
async fn session_revocation_is_scoped_and_atomic() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "password")
        .await
        .unwrap();
    let bob = bootstrap_admin(&pool, "bob", None, "password")
        .await
        .unwrap();
    let alice_session = session::create(&pool, alice, 3600).await.unwrap();
    let bob_session = session::create(&pool, bob, 3600).await.unwrap();
    deny_audit(&pool).await;
    assert!(revoke_sessions(&pool, "alice", alice).await.is_err());
    assert!(session::lookup(&pool, &alice_session)
        .await
        .unwrap()
        .is_some());
    allow_audit(&pool).await;
    assert_eq!(revoke_sessions(&pool, "alice", alice).await.unwrap(), 1);
    assert!(session::lookup(&pool, &alice_session)
        .await
        .unwrap()
        .is_none());
    assert!(session::lookup(&pool, &bob_session)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn deactivation_revokes_every_credential_and_keeps_a_recovery_admin() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "password")
        .await
        .unwrap();
    let bob = bootstrap_admin(&pool, "bob", None, "password")
        .await
        .unwrap();
    let (_, plaintext) = issue_actor_token(&pool, "bob", "human", None, None, None, alice)
        .await
        .unwrap();
    let sid = session::create(&pool, bob, 3600).await.unwrap();
    deny_audit(&pool).await;
    assert!(deactivate_actor(&pool, "bob", alice).await.is_err());
    assert!(
        queries::actor_by_id(&pool, bob)
            .await
            .unwrap()
            .unwrap()
            .is_active
    );
    assert!(token_works(&pool, &plaintext).await);
    assert!(session::lookup(&pool, &sid).await.unwrap().is_some());
    allow_audit(&pool).await;
    deactivate_actor(&pool, "bob", alice).await.unwrap();
    assert!(!token_works(&pool, &plaintext).await);
    assert!(session::lookup(&pool, &sid).await.unwrap().is_none());
    assert!(session::create(&pool, bob, 3600).await.is_err());
    assert!(
        issue_actor_token(&pool, "bob", "human", None, None, None, alice)
            .await
            .is_err()
    );
    assert!(matches!(
        deactivate_actor(&pool, "alice", alice).await,
        Err(AppError::Conflict(_))
    ));
    assert!(
        queries::actor_by_id(&pool, alice)
            .await
            .unwrap()
            .unwrap()
            .is_active
    );
    bootstrap_admin(&pool, "bob", None, "reset-password")
        .await
        .unwrap();
    assert!(
        !token_works(&pool, &plaintext).await,
        "reactivation must not revive old tokens"
    );
    assert!(session::lookup(&pool, &sid).await.unwrap().is_none());
}

#[tokio::test]
async fn non_operator_cannot_change_credentials_or_deactivate_actors() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "password")
        .await
        .unwrap();
    let (worker, _) = issue_actor_token(&pool, "worker", "agent", None, None, None, alice)
        .await
        .unwrap();
    assert!(matches!(
        issue_actor_token(&pool, "other", "agent", None, None, None, worker).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        revoke_sessions(&pool, "alice", worker).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        deactivate_actor(&pool, "alice", worker).await,
        Err(AppError::Forbidden)
    ));
    assert!(queries::actor_by_handle(&pool, "other")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn unsafe_handles_are_rejected_before_storage() {
    let (_dir, pool) = database().await;
    for handle in [
        "",
        "../escape",
        "alice\ncommitter forged",
        "a<b>",
        "a@host",
        "alice smith",
        "a:b",
        ".",
        "..",
        "ø",
        "a\0b",
    ] {
        assert!(validate_handle(handle).is_err(), "accepted {handle:?}");
        assert!(queries::insert_actor(&pool, "agent", "name", handle, false)
            .await
            .is_err());
    }
    assert!(validate_handle(&"a".repeat(65)).is_err());
    for handle in ["alice", "agent-1", "research_bot", "alice.test"] {
        queries::insert_actor(&pool, "agent", "name", handle, false)
            .await
            .unwrap();
    }
    assert_eq!(queries::list_actors(&pool).await.unwrap().len(), 4);
}

#[tokio::test]
async fn legacy_token_upgrade_keeps_revocation_effective() {
    let (_dir, pool) = database().await;
    let alice = bootstrap_admin(&pool, "alice", None, "password")
        .await
        .unwrap();
    let (_, plaintext) = issue_actor_token(&pool, "worker", "agent", None, None, None, alice)
        .await
        .unwrap();
    let credential = list_credentials(&pool, Some("worker"))
        .await
        .unwrap()
        .remove(0);
    let secret = plaintext.rsplit('_').next().unwrap();
    let legacy = hash::hash_async(secret).await.unwrap();
    sqlx::query("UPDATE credentials SET secret_hash = ? WHERE id = ?")
        .bind(legacy)
        .bind(credential.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(token_works(&pool, &plaintext).await);
    let stored: String = sqlx::query_scalar("SELECT secret_hash FROM credentials WHERE id = ?")
        .bind(credential.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 64);
    assert!(!stored.starts_with('$'));
    revoke_credential(&pool, credential.id, alice)
        .await
        .unwrap();
    assert!(!token_works(&pool, &plaintext).await);
}

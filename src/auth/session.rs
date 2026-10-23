//! Cookie sessions for `human` actors.
//!
//! The cookie carries a random 32-byte session id (hex-encoded). The
//! server looks it up in `sessions`, joins to `actors`, and attaches the
//! resolved Actor to the request. There is no client-readable claim.

use rand::RngCore;
use sqlx::{Executor, Sqlite, SqlitePool};

use crate::db::queries::{actor_by_id, Actor};
use crate::error::{AppError, Result};

pub const COOKIE_NAME: &str = "wk_session";

pub fn new_session_id() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Create a session for a trusted caller that already holds the required
/// authentication context. Password handlers must use `login` so verification
/// and creation cannot straddle an operator's password reset.
pub async fn create<'e, E>(executor: E, actor_id: i64, lifetime_secs: i64) -> Result<String>
where
    E: Executor<'e, Database = Sqlite>,
{
    let id = new_session_id();
    let now = crate::db::now_ts();
    let expires = now
        .checked_add(lifetime_secs)
        .filter(|expiry| *expiry > now)
        .ok_or_else(|| AppError::BadRequest("invalid session lifetime".into()))?;
    let inserted = sqlx::query(
        "INSERT INTO sessions(id, actor_id, created_at, expires_at)
         SELECT ?, id, ?, ? FROM actors WHERE id = ? AND is_active = 1",
    )
    .bind(&id)
    .bind(now)
    .bind(expires)
    .bind(actor_id)
    .execute(executor)
    .await?
    .rows_affected();
    if inserted != 1 {
        return Err(AppError::Unauthorized);
    }
    Ok(id)
}

pub async fn lookup(pool: &SqlitePool, session_id: &str) -> Result<Option<Actor>> {
    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT actor_id FROM sessions
         WHERE id = ? AND revoked_at IS NULL AND expires_at > ?",
    )
    .bind(session_id)
    .bind(crate::db::now_ts())
    .fetch_optional(pool)
    .await?;
    let Some((actor_id,)) = row else {
        return Ok(None);
    };
    let actor = actor_by_id(pool, actor_id).await?;
    Ok(actor.filter(|a| a.is_active))
}

pub async fn revoke(pool: &SqlitePool, session_id: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ?")
        .bind(crate::db::now_ts())
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Verify a handle+password against the credentials table, returning the
/// active actor on success. The error variant is collapsed to `Unauthorized`
/// to avoid leaking which side of the credential was wrong.
/// Use `login` when the next action is to create a browser session.
pub async fn verify_password_login(
    pool: &SqlitePool,
    handle: &str,
    password: &str,
) -> Result<Actor> {
    let (credential_id, stored) = verify_password_credential(pool, handle, password).await?;
    current_credential_actor(pool, credential_id, &stored).await
}

/// Verify outside the SQLite lock, then create a session atomically with a
/// final credential check. A simultaneous reset either revokes this session
/// or makes the check fail. It cannot leave an old-password login alive.
pub async fn login(
    pool: &SqlitePool,
    handle: &str,
    password: &str,
    lifetime_secs: i64,
) -> Result<(Actor, String)> {
    let (credential_id, stored) = verify_password_credential(pool, handle, password).await?;
    finish_login(pool, credential_id, &stored, lifetime_secs).await
}

async fn finish_login(
    pool: &SqlitePool,
    credential_id: i64,
    stored: &str,
    lifetime_secs: i64,
) -> Result<(Actor, String)> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let actor = current_credential_actor(&mut *tx, credential_id, stored).await?;
    let session_id = create(&mut *tx, actor.id, lifetime_secs).await?;
    tx.commit().await?;
    Ok((actor, session_id))
}

async fn verify_password_credential(
    pool: &SqlitePool,
    handle: &str,
    password: &str,
) -> Result<(i64, String)> {
    let row = sqlx::query_as::<_, (i64, String)>(
        "SELECT c.id, c.secret_hash
         FROM actors a
         JOIN credentials c ON c.actor_id = a.id
         WHERE a.handle = ?
           AND a.is_active = 1
           AND c.kind = 'password'
           AND c.revoked_at IS NULL
           AND (c.expires_at IS NULL OR c.expires_at > ?)
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT 1",
    )
    .bind(handle)
    .bind(crate::db::now_ts())
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Err(AppError::Unauthorized);
    };
    if !super::hash::verify_async(password, &row.1).await? {
        return Err(AppError::Unauthorized);
    }
    Ok(row)
}

async fn current_credential_actor<'e, E>(
    executor: E,
    credential_id: i64,
    stored: &str,
) -> Result<Actor>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        "SELECT a.id, a.type, a.name, a.handle, a.is_admin, a.is_active
         FROM actors a JOIN credentials c ON c.actor_id = a.id
         WHERE c.id = ? AND c.secret_hash = ? AND c.kind = 'password'
           AND c.revoked_at IS NULL AND a.is_active = 1
           AND (c.expires_at IS NULL OR c.expires_at > ?)",
    )
    .bind(credential_id)
    .bind(stored)
    .bind(crate::db::now_ts())
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::Unauthorized)?;
    Ok(Actor {
        id: row.0,
        actor_type: row.1,
        name: row.2,
        handle: row.3,
        is_admin: row.4 != 0,
        is_active: row.5 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interleave the reset exactly between successful password verification
    /// and session creation, without timing guesses or artificial sleeps.
    #[tokio::test]
    async fn password_reset_between_verification_and_session_creation_blocks_login() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_and_migrate(&dir.path().join("login.db"))
            .await
            .unwrap();
        crate::actor::bootstrap_admin(&pool, "alice", None, "old-password")
            .await
            .unwrap();
        let (credential_id, stored) = verify_password_credential(&pool, "alice", "old-password")
            .await
            .unwrap();
        crate::actor::bootstrap_admin(&pool, "alice", None, "new-password")
            .await
            .unwrap();
        assert!(matches!(
            finish_login(&pool, credential_id, &stored, 3600).await,
            Err(AppError::Unauthorized)
        ));
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0);
    }
}

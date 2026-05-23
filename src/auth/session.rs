//! Cookie sessions for `human` actors.
//!
//! The cookie carries a random 32-byte session id (hex-encoded). The
//! server looks it up in `sessions`, joins to `actors`, and attaches the
//! resolved Actor to the request. There is no client-readable claim.

use rand::RngCore;
use sqlx::SqlitePool;

use crate::db::queries::{actor_by_id, Actor};
use crate::error::{AppError, Result};

pub const COOKIE_NAME: &str = "wk_session";

pub fn new_session_id() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

pub async fn create(
    pool: &SqlitePool,
    actor_id: i64,
    lifetime_secs: i64,
) -> Result<String> {
    let id = new_session_id();
    let now = crate::db::now_ts();
    let expires = now + lifetime_secs;
    sqlx::query(
        "INSERT INTO sessions(id, actor_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(actor_id)
    .bind(now)
    .bind(expires)
    .execute(pool)
    .await?;
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
pub async fn verify_password_login(
    pool: &SqlitePool,
    handle: &str,
    password: &str,
) -> Result<Actor> {
    let row = sqlx::query_as::<_, (i64, String, String, String, i64, i64, String)>(
        "SELECT a.id, a.type, a.name, a.handle, a.is_admin, a.is_active, c.secret_hash
         FROM actors a
         JOIN credentials c ON c.actor_id = a.id
         WHERE a.handle = ?
           AND a.is_active = 1
           AND c.kind = 'password'
           AND c.revoked_at IS NULL
         ORDER BY c.created_at DESC
         LIMIT 1",
    )
    .bind(handle)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Err(AppError::Unauthorized);
    };
    if !super::hash::verify(password, &row.6)? {
        return Err(AppError::Unauthorized);
    }
    Ok(Actor {
        id: row.0,
        actor_type: row.1,
        name: row.2,
        handle: row.3,
        is_admin: row.4 != 0,
        is_active: row.5 != 0,
    })
}

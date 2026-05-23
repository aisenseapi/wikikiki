//! Programmatic actor operations used by the CLI (bootstrap, issue-token).
//!
//! These are not HTTP handlers — they are the things `wikikiki bootstrap`
//! and `wikikiki issue-token` call directly against the DB.

use sqlx::SqlitePool;

use crate::auth::{hash, log_admin, token};
use crate::db::{now_ts, queries};
use crate::error::{AppError, Result};

/// Create the first human admin, attaching a password credential.
/// Idempotent on handle: if it exists, only the password is replaced.
pub async fn bootstrap_admin(
    pool: &SqlitePool,
    handle: &str,
    name: Option<&str>,
    password: &str,
) -> Result<i64> {
    let existing = queries::actor_by_handle(pool, handle).await?;
    let actor_id = if let Some(a) = existing {
        if a.actor_type != "human" {
            return Err(AppError::Conflict(format!(
                "actor {handle} exists with type {} (not 'human')",
                a.actor_type
            )));
        }
        // Promote to admin if not already.
        sqlx::query("UPDATE actors SET is_admin = 1, is_active = 1 WHERE id = ?")
            .bind(a.id)
            .execute(pool)
            .await?;
        a.id
    } else {
        queries::insert_actor(pool, "human", name.unwrap_or(handle), handle, true).await?
    };

    // Replace any prior password credential.
    sqlx::query(
        "UPDATE credentials SET revoked_at = ?
         WHERE actor_id = ? AND kind = 'password' AND revoked_at IS NULL",
    )
    .bind(now_ts())
    .bind(actor_id)
    .execute(pool)
    .await?;

    let pw_hash = hash::hash(password)?;
    sqlx::query(
        "INSERT INTO credentials(actor_id, kind, secret_hash, label, created_at)
         VALUES (?, 'password', ?, 'primary password', ?)",
    )
    .bind(actor_id)
    .bind(&pw_hash)
    .bind(now_ts())
    .execute(pool)
    .await?;

    log_admin(
        pool,
        actor_id,
        &format!("actors:{actor_id}"),
        "bootstrap",
        None,
        Some(&format!("{{\"handle\":\"{handle}\",\"is_admin\":true}}")),
        Some("bootstrap admin via CLI"),
    )
    .await?;
    Ok(actor_id)
}

/// Create an actor + issue a token. Returns (actor_id, plaintext_token).
/// `actor_type` must already exist in `actor_types` (e.g. 'agent', 'external').
pub async fn issue_actor_token(
    pool: &SqlitePool,
    handle: &str,
    actor_type: &str,
    name: Option<&str>,
    label: Option<&str>,
    expires_at: Option<i64>,
    issuer_actor_id: i64,
) -> Result<(i64, String)> {
    let type_exists: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM actor_types WHERE code = ? AND is_active = 1")
            .bind(actor_type)
            .fetch_optional(pool)
            .await?;
    if type_exists.is_none() {
        return Err(AppError::BadRequest(format!(
            "unknown actor_type: {actor_type}"
        )));
    }

    let actor_id = if let Some(a) = queries::actor_by_handle(pool, handle).await? {
        if a.actor_type != actor_type {
            return Err(AppError::Conflict(format!(
                "actor {handle} already exists with type {} (requested {actor_type})",
                a.actor_type
            )));
        }
        a.id
    } else {
        queries::insert_actor(pool, actor_type, name.unwrap_or(handle), handle, false).await?
    };

    let issued = token::issue(pool, actor_id, label, expires_at).await?;

    log_admin(
        pool,
        issuer_actor_id,
        &format!("credentials:{}", issued.credential_id),
        "issue_token",
        None,
        Some(&format!(
            "{{\"actor_id\":{actor_id},\"label\":{}}}",
            serde_json::to_string(&label).unwrap_or_else(|_| "null".into())
        )),
        Some(&format!("issued to {handle}")),
    )
    .await?;

    Ok((actor_id, issued.plaintext))
}

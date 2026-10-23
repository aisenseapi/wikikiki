//! Operator operations used by the CLI. Each mutation and its audit entry
//! commit together. These functions do not expose an HTTP admin surface.

use sqlx::{SqliteConnection, SqlitePool};

use crate::auth::{hash, log_admin, token};
use crate::db::{now_ts, queries};
use crate::error::{AppError, Result};

/// Handles become both Git identities and page namespace components.
pub fn validate_handle(handle: &str) -> Result<()> {
    if handle.is_empty()
        || handle.len() > 64
        || !handle.as_bytes()[0].is_ascii_alphanumeric()
        || !handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(AppError::BadRequest(
            "handle must contain 1-64 ASCII letters, digits, dots, underscores or hyphens and start with a letter or digit".into(),
        ));
    }
    Ok(())
}

async fn require_operator(conn: &mut SqliteConnection, actor_id: i64) -> Result<()> {
    let actor = queries::actor_by_id(&mut *conn, actor_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("issuer actor {actor_id}")))?;
    if !actor.is_active || (!actor.is_admin && actor.actor_type != "system") {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Create a human admin or reset an existing human's password. Hash before
/// taking the write lock. A reset also invalidates all existing sessions.
pub async fn bootstrap_admin(
    pool: &SqlitePool,
    handle: &str,
    name: Option<&str>,
    password: &str,
) -> Result<i64> {
    validate_handle(handle)?;
    if password.is_empty() || password.len() > 4096 {
        return Err(AppError::BadRequest(
            "password must contain 1 to 4096 bytes".into(),
        ));
    }
    let pw_hash = hash::hash_async(password).await?;
    let now = now_ts();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing = queries::actor_by_handle(&mut *tx, handle).await?;
    let actor_id = if let Some(a) = existing {
        if a.actor_type != "human" {
            return Err(AppError::Conflict(format!(
                "actor {handle} exists with type {} (not 'human')",
                a.actor_type
            )));
        }
        sqlx::query("UPDATE actors SET is_admin = 1, is_active = 1 WHERE id = ?")
            .bind(a.id)
            .execute(&mut *tx)
            .await?;
        a.id
    } else {
        queries::insert_actor(&mut *tx, "human", name.unwrap_or(handle), handle, true).await?
    };
    sqlx::query(
        "UPDATE credentials SET revoked_at = ?
         WHERE actor_id = ? AND kind = 'password' AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE actor_id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO credentials(actor_id, kind, secret_hash, label, created_at)
         VALUES (?, 'password', ?, 'primary password', ?)",
    )
    .bind(actor_id)
    .bind(&pw_hash)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    log_admin(
        &mut *tx,
        actor_id,
        &format!("actors:{actor_id}"),
        "bootstrap",
        None,
        Some(&serde_json::json!({"handle": handle, "is_admin": true}).to_string()),
        Some("bootstrap admin via CLI; existing sessions revoked"),
    )
    .await?;
    tx.commit().await?;
    Ok(actor_id)
}

/// Create or reuse an active actor and issue one token, atomically with its
/// audit entry. The plaintext is returned only after the transaction commits.
pub async fn issue_actor_token(
    pool: &SqlitePool,
    handle: &str,
    actor_type: &str,
    name: Option<&str>,
    label: Option<&str>,
    expires_at: Option<i64>,
    issuer_actor_id: i64,
) -> Result<(i64, String)> {
    validate_handle(handle)?;
    validate_handle(actor_type)
        .map_err(|_| AppError::BadRequest("invalid actor type code".into()))?;
    if expires_at.is_some_and(|expiry| expiry <= now_ts()) {
        return Err(AppError::BadRequest(
            "token expiry must be in the future".into(),
        ));
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    require_operator(&mut tx, issuer_actor_id).await?;
    let type_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM actor_types WHERE code = ? AND is_active = 1)",
    )
    .bind(actor_type)
    .fetch_one(&mut *tx)
    .await?;
    if !type_exists {
        return Err(AppError::BadRequest(format!(
            "unknown actor_type: {actor_type}"
        )));
    }
    let actor_id = if let Some(a) = queries::actor_by_handle(&mut *tx, handle).await? {
        if a.actor_type != actor_type {
            return Err(AppError::Conflict(format!(
                "actor {handle} already exists with type {} (requested {actor_type})",
                a.actor_type
            )));
        }
        if !a.is_active {
            return Err(AppError::Conflict(format!("actor {handle} is inactive")));
        }
        a.id
    } else {
        queries::insert_actor(&mut *tx, actor_type, name.unwrap_or(handle), handle, false).await?
    };
    let issued = token::issue(&mut *tx, actor_id, label, expires_at).await?;
    log_admin(
        &mut *tx,
        issuer_actor_id,
        &format!("credentials:{}", issued.credential_id),
        "issue_token",
        None,
        Some(&serde_json::json!({"actor_id": actor_id, "label": label}).to_string()),
        Some(&format!("issued to {handle}")),
    )
    .await?;
    tx.commit().await?;
    Ok((actor_id, issued.plaintext))
}

/// Credential metadata without lookup keys or secret hashes.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct CredentialInfo {
    pub id: i64,
    pub actor_id: i64,
    pub actor_handle: String,
    pub kind: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

pub async fn list_credentials(
    pool: &SqlitePool,
    handle: Option<&str>,
) -> Result<Vec<CredentialInfo>> {
    if let Some(handle) = handle {
        if queries::actor_by_handle(pool, handle).await?.is_none() {
            return Err(AppError::NotFound(format!("actor {handle}")));
        }
    }
    Ok(sqlx::query_as::<_, CredentialInfo>(
        "SELECT c.id, c.actor_id, a.handle AS actor_handle, c.kind, c.label,
                c.created_at, c.last_used_at, c.expires_at, c.revoked_at
         FROM credentials c JOIN actors a ON a.id = c.actor_id
         WHERE (? IS NULL OR a.handle = ?) ORDER BY c.id",
    )
    .bind(handle)
    .bind(handle)
    .fetch_all(pool)
    .await?)
}

pub async fn revoke_credential(
    pool: &SqlitePool,
    credential_id: i64,
    issuer_actor_id: i64,
) -> Result<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    require_operator(&mut tx, issuer_actor_id).await?;
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT actor_id, kind FROM credentials WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(credential_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (actor_id, kind) =
        row.ok_or_else(|| AppError::NotFound(format!("active credential {credential_id}")))?;
    let now = now_ts();
    sqlx::query("UPDATE credentials SET revoked_at = ? WHERE id = ?")
        .bind(now)
        .bind(credential_id)
        .execute(&mut *tx)
        .await?;
    // Revoking a password must also end sessions derived from that password.
    if kind == "password" {
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE actor_id = ? AND revoked_at IS NULL")
            .bind(now)
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;
    }
    log_admin(
        &mut *tx,
        issuer_actor_id,
        &format!("credentials:{credential_id}"),
        "revoke_credential",
        None,
        Some(&serde_json::json!({"revoked_at": now}).to_string()),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn revoke_sessions(pool: &SqlitePool, handle: &str, issuer_actor_id: i64) -> Result<u64> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    require_operator(&mut tx, issuer_actor_id).await?;
    let actor = queries::actor_by_handle(&mut *tx, handle)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("actor {handle}")))?;
    let count =
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE actor_id = ? AND revoked_at IS NULL")
            .bind(now_ts())
            .bind(actor.id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    log_admin(
        &mut *tx,
        issuer_actor_id,
        &format!("actors:{}", actor.id),
        "revoke_sessions",
        None,
        Some(&serde_json::json!({"sessions_revoked": count}).to_string()),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(count)
}

/// Disable an actor and permanently revoke its credentials and sessions.
/// Re-enabling an actor must not resurrect old credentials. Keep at least
/// one active human admin for operator recovery.
pub async fn deactivate_actor(pool: &SqlitePool, handle: &str, issuer_actor_id: i64) -> Result<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    require_operator(&mut tx, issuer_actor_id).await?;
    let actor = queries::actor_by_handle(&mut *tx, handle)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("actor {handle}")))?;
    if actor.is_active && actor.is_admin && actor.actor_type == "human" {
        let others: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM actors WHERE type = 'human' AND is_admin = 1 AND is_active = 1 AND id != ?",
        ).bind(actor.id).fetch_one(&mut *tx).await?;
        if others == 0 {
            return Err(AppError::Conflict(
                "cannot deactivate the last active human admin".into(),
            ));
        }
    }
    let now = now_ts();
    sqlx::query("UPDATE actors SET is_active = 0 WHERE id = ?")
        .bind(actor.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE credentials SET revoked_at = ? WHERE actor_id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(actor.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE actor_id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(actor.id)
        .execute(&mut *tx)
        .await?;
    log_admin(
        &mut *tx,
        issuer_actor_id,
        &format!("actors:{}", actor.id),
        "deactivate_actor",
        Some(&serde_json::json!({"is_active": actor.is_active}).to_string()),
        Some("{\"is_active\":false}"),
        Some("all credentials and sessions revoked"),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

//! Bearer-token credentials for `agent` and `external` actors.
//!
//! Token wire format: `wk_<lookup>_<secret>`
//! - `lookup`: 16 hex chars (8 random bytes). Stored verbatim on the row
//!   so the verifier can do an indexed lookup instead of scanning.
//! - `secret`: 48 hex chars (24 random bytes). Argon2-hashed in
//!   `credentials.secret_hash`. Plaintext is shown to the user once.
//!
//! Revocation is immediate: setting `revoked_at` and the lookup misses
//! immediately filter the row out.

use rand::RngCore;
use sqlx::SqlitePool;

use crate::db::queries::Actor;
use crate::error::{AppError, Result};

pub const TOKEN_PREFIX: &str = "wk_";

#[derive(Debug)]
pub struct IssuedToken {
    pub plaintext: String,
    pub credential_id: i64,
}

/// Mint a new token for `actor_id` and store its hash. The plaintext is
/// returned to the caller and must be shown to the user *exactly once*.
pub async fn issue(
    pool: &SqlitePool,
    actor_id: i64,
    label: Option<&str>,
    expires_at: Option<i64>,
) -> Result<IssuedToken> {
    let mut lookup_bytes = [0u8; 8];
    let mut secret_bytes = [0u8; 24];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut lookup_bytes);
    rng.fill_bytes(&mut secret_bytes);
    let lookup = hex::encode(lookup_bytes);
    let secret = hex::encode(secret_bytes);
    let plaintext = format!("{TOKEN_PREFIX}{lookup}_{secret}");

    let secret_hash = super::hash::hash(&secret)?;
    let now = crate::db::now_ts();

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO credentials(actor_id, kind, lookup_key, secret_hash, label, expires_at, created_at)
         VALUES (?, 'token', ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(actor_id)
    .bind(&lookup)
    .bind(&secret_hash)
    .bind(label)
    .bind(expires_at)
    .bind(now)
    .fetch_one(pool)
    .await?;

    Ok(IssuedToken {
        plaintext,
        credential_id: id,
    })
}

/// Parse a bearer token into (lookup, secret) parts. Wrong shape → None.
fn parse_token(t: &str) -> Option<(&str, &str)> {
    let rest = t.strip_prefix(TOKEN_PREFIX)?;
    let (lookup, secret) = rest.split_once('_')?;
    if lookup.len() == 16 && secret.len() == 48 {
        Some((lookup, secret))
    } else {
        None
    }
}

/// Resolve a bearer token to its active actor. Touches `last_used_at` on hit.
pub async fn verify(pool: &SqlitePool, token: &str) -> Result<Option<Actor>> {
    let Some((lookup, secret)) = parse_token(token) else {
        return Ok(None);
    };

    let row = sqlx::query_as::<_, (i64, String, i64, String, String, String, i64, i64, Option<i64>)>(
        "SELECT c.id, c.secret_hash, a.id, a.type, a.name, a.handle, a.is_admin, a.is_active, c.expires_at
         FROM credentials c
         JOIN actors a ON a.id = c.actor_id
         WHERE c.lookup_key = ?
           AND c.kind = 'token'
           AND c.revoked_at IS NULL",
    )
    .bind(lookup)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    // Expiry check.
    if let Some(exp) = row.8 {
        if exp <= crate::db::now_ts() {
            return Ok(None);
        }
    }
    if row.7 == 0 {
        // Actor revoked.
        return Ok(None);
    }
    if !super::hash::verify(secret, &row.1)? {
        return Ok(None);
    }

    let _ = sqlx::query("UPDATE credentials SET last_used_at = ? WHERE id = ?")
        .bind(crate::db::now_ts())
        .bind(row.0)
        .execute(pool)
        .await;

    Ok(Some(Actor {
        id: row.2,
        actor_type: row.3,
        name: row.4,
        handle: row.5,
        is_admin: row.6 != 0,
        is_active: row.7 != 0,
    }))
}

/// Revoke a single credential row.
pub async fn revoke(pool: &SqlitePool, credential_id: i64) -> Result<()> {
    let updated = sqlx::query("UPDATE credentials SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(crate::db::now_ts())
        .bind(credential_id)
        .execute(pool)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound(format!("credential {credential_id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shape() {
        let good = format!("{TOKEN_PREFIX}{}_{}", "a".repeat(16), "b".repeat(48));
        assert!(parse_token(&good).is_some());
        assert!(parse_token("nope").is_none());
        assert!(parse_token(&format!("{TOKEN_PREFIX}aaaa_bbbb")).is_none());
    }
}

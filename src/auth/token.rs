//! Bearer-token credentials for `agent` and `external` actors.
//!
//! Token wire format: `wk_<lookup>_<secret>`
//! - `lookup`: 16 hex chars (8 random bytes). Stored verbatim on the row
//!   so the verifier can do an indexed lookup instead of scanning.
//! - `secret`: 48 hex chars (24 random bytes), stored as a SHA-256 digest in
//!   `credentials.secret_hash`. Plaintext is shown to the user once.
//!
//! ## Why SHA-256 here and Argon2 for passwords
//!
//! Argon2's cost is the whole point for a password: it buys time against
//! offline guessing of a low-entropy secret a human chose. A token is 24
//! bytes from the OS CSPRNG — 192 bits. There is no guessing attack to slow
//! down, so the deliberate cost buys nothing and is paid on *every single API
//! request*. At Argon2 defaults that is tens of milliseconds per call, which
//! puts DESIGN.md §11's target ("well into hundreds of writes/sec") out of
//! reach for reasons that have nothing to do with the database.
//!
//! A plain digest is the correct primitive for a high-entropy secret: it
//! still means a leaked database yields no usable tokens, at roughly a
//! microsecond. The comparison is constant-time so the digest cannot be
//! recovered by timing.
//!
//! Credentials minted before this change carry an Argon2 PHC string. They are
//! still accepted, and the row is transparently rewritten to a digest on
//! first successful use — no reissue, no flag day.
//!
//! Revocation is immediate: setting `revoked_at` and the lookup misses
//! immediately filter the row out.

use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

use crate::db::queries::Actor;
use crate::error::{AppError, Result};

pub const TOKEN_PREFIX: &str = "wk_";

/// SHA-256 of the token secret, hex-encoded.
fn secret_digest(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    hex::encode(h.finalize())
}

/// A stored Argon2 hash is a PHC string and always starts with `$`; a digest
/// is bare hex and never does. That is enough to tell the two apart without
/// a schema column to keep in sync.
fn is_legacy_argon2(stored: &str) -> bool {
    stored.starts_with('$')
}

/// Constant-time equality against the stored digest.
fn digest_matches(secret: &str, stored: &str) -> bool {
    secret_digest(secret)
        .as_bytes()
        .ct_eq(stored.as_bytes())
        .into()
}

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

    let secret_hash = secret_digest(&secret);
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

/// Resolve a bearer token to its active actor. Touches `last_used_at` on hit,
/// subject to `touch` (see [`crate::throttle`]).
pub async fn verify(
    pool: &SqlitePool,
    token: &str,
    touch: &crate::throttle::Throttle,
) -> Result<Option<Actor>> {
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

    let credential_id = row.0;
    let stored = &row.1;
    if is_legacy_argon2(stored) {
        if !super::hash::verify(secret, stored)? {
            return Ok(None);
        }
        // Upgrade in place, once, on first successful use. Cheap here
        // because we have just paid the Argon2 cost anyway, and it means the
        // expensive path drains away on its own rather than needing a
        // migration that cannot recompute one-way hashes.
        let digest = secret_digest(secret);
        if let Err(e) = sqlx::query("UPDATE credentials SET secret_hash = ? WHERE id = ?")
            .bind(&digest)
            .bind(credential_id)
            .execute(pool)
            .await
        {
            // A failed upgrade is not a failed authentication; the row stays
            // legacy and we try again next time.
            tracing::warn!(error = %e, credential_id, "token digest upgrade failed");
        }
    } else if !digest_matches(secret, stored) {
        return Ok(None);
    }

    // `last_used_at` has minute-resolution usefulness, so it does not justify
    // a SQLite write lock on every authenticated request (DESIGN.md §11).
    if touch.claim(credential_id, crate::db::now_ts()) {
        let _ = sqlx::query("UPDATE credentials SET last_used_at = ? WHERE id = ?")
            .bind(crate::db::now_ts())
            .bind(credential_id)
            .execute(pool)
            .await;
    }

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

    #[test]
    fn digest_accepts_the_secret_and_rejects_everything_else() {
        let stored = secret_digest("s3cret");
        assert!(digest_matches("s3cret", &stored));
        assert!(!digest_matches("s3cres", &stored));
        assert!(!digest_matches("", &stored));
        assert!(!digest_matches("s3cret ", &stored));
    }

    #[test]
    fn digest_is_bare_hex_so_it_never_looks_legacy() {
        let d = secret_digest("whatever");
        assert_eq!(d.len(), 64, "sha256 hex");
        assert!(d.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!is_legacy_argon2(&d));
    }

    /// The upgrade path hinges on telling the two encodings apart, so pin it
    /// against a hash produced by the real hasher rather than a literal.
    #[test]
    fn argon2_phc_strings_are_recognised_as_legacy() {
        let phc = crate::auth::hash::hash("s3cret").unwrap();
        assert!(is_legacy_argon2(&phc), "got {phc}");
        assert!(crate::auth::hash::verify("s3cret", &phc).unwrap());
        // ...and must not be mistaken for a digest of the same secret.
        assert!(!digest_matches("s3cret", &phc));
    }

    #[test]
    fn mismatched_lengths_do_not_panic() {
        assert!(!digest_matches("x", "short"));
        assert!(!digest_matches("x", &"f".repeat(128)));
    }
}

//! Argon2id password/token hashing.

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;

use crate::error::{AppError, Result};

// Argon2 is deliberately expensive. Bound its memory/CPU use and keep it
// away from the async workers that serve pages and drain accepted writes.
static HASH_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

pub async fn hash_async(secret: &str) -> Result<String> {
    let secret = secret.to_owned();
    let permit = HASH_WORKERS
        .acquire()
        .await
        .map_err(|_| AppError::Internal("password hashing unavailable".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash(&secret)
    })
    .await
    .map_err(|e| AppError::Internal(format!("password hashing task: {e}")))?
}

pub async fn verify_async(secret: &str, stored: &str) -> Result<bool> {
    let secret = secret.to_owned();
    let stored = stored.to_owned();
    let permit = HASH_WORKERS
        .acquire()
        .await
        .map_err(|_| AppError::Internal("password verification unavailable".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        verify(&secret, &stored)
    })
    .await
    .map_err(|e| AppError::Internal(format!("password verification task: {e}")))?
}

pub fn hash(secret: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let phc = Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map_err(|e| AppError::Internal(format!("hash: {e}")))?;
    Ok(phc.to_string())
}

pub fn verify(secret: &str, stored: &str) -> Result<bool> {
    let parsed = PasswordHash::new(stored)
        .map_err(|_| AppError::BadRequest("invalid stored hash".into()))?;
    Ok(Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let h = hash("hunter2").unwrap();
        assert!(verify("hunter2", &h).unwrap());
        assert!(!verify("hunter3", &h).unwrap());
    }
}

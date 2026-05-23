//! Argon2id password/token hashing.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::error::{AppError, Result};

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

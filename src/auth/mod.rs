//! Identity & authentication (DESIGN.md §14).
//!
//! One access model across surfaces (web UI, REST API, MCP-later). Every
//! request resolves to exactly one actor — or anonymous — before any handler
//! runs. Tokens carry no claims beyond actor identity; permissions are
//! looked up server-side so revocation is immediate.

pub mod hash;
pub mod session;
pub mod token;

use sqlx::SqlitePool;

use crate::db::queries::Actor;
use crate::error::Result;

/// The actor a request is acting as, plus the surface they reached us through.
#[derive(Debug, Clone)]
pub struct Auth {
    pub actor: Actor,
    pub surface: Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Ui,
    Api,
    Mcp,
}

impl Surface {
    pub fn as_str(&self) -> &'static str {
        match self {
            Surface::Ui => "ui",
            Surface::Api => "api",
            Surface::Mcp => "mcp",
        }
    }
}

/// Log an access-log entry. Failures here are tracing-logged but never
/// bubble up — auditing a denied write must not itself cause a 500.
pub async fn log_access(
    pool: &SqlitePool,
    actor_id: Option<i64>,
    attempted_handle: Option<&str>,
    surface: Surface,
    outcome: &str,
    target: Option<&str>,
    remote_addr: Option<&str>,
) {
    let res = sqlx::query(
        "INSERT INTO access_log(actor_id, attempted_handle, surface, outcome, target, remote_addr, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(actor_id)
    .bind(attempted_handle)
    .bind(surface.as_str())
    .bind(outcome)
    .bind(target)
    .bind(remote_addr)
    .bind(crate::db::now_ts())
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "access_log insert failed");
    }
}

/// Log an admin_audit entry. Same fail-soft policy as `log_access`.
pub async fn log_admin(
    pool: &SqlitePool,
    actor_id: i64,
    target: &str,
    operation: &str,
    before_value: Option<&str>,
    after_value: Option<&str>,
    note: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO admin_audit(actor_id, target, operation, before_value, after_value, note, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(actor_id)
    .bind(target)
    .bind(operation)
    .bind(before_value)
    .bind(after_value)
    .bind(note)
    .bind(crate::db::now_ts())
    .execute(pool)
    .await?;
    Ok(())
}

/// Hash an IP/UA for logging when `auth.log_remote_addr = "hashed"`.
pub fn hash_remote_addr(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    // First 16 hex chars is plenty for de-duplication without identifying.
    hex::encode(&out[..8])
}

pub fn format_remote_addr(raw: &str, mode: &str) -> Option<String> {
    match mode {
        "plain" => Some(raw.to_string()),
        "hashed" => Some(hash_remote_addr(raw)),
        _ => None,
    }
}

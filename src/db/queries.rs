//! Hand-rolled DB queries. Kept un-`query!()`-macro'd so the project compiles
//! without a live database at build time.

use sqlx::SqlitePool;

use crate::error::Result;

/// One row of the `actors` table.
#[derive(Debug, Clone)]
pub struct Actor {
    pub id: i64,
    pub actor_type: String,
    pub name: String,
    pub handle: String,
    pub is_admin: bool,
    pub is_active: bool,
}

pub async fn actor_by_id(pool: &SqlitePool, id: i64) -> Result<Option<Actor>> {
    let row = sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        "SELECT id, type, name, handle, is_admin, is_active FROM actors WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| Actor {
        id: r.0,
        actor_type: r.1,
        name: r.2,
        handle: r.3,
        is_admin: r.4 != 0,
        is_active: r.5 != 0,
    }))
}

pub async fn actor_by_handle(pool: &SqlitePool, handle: &str) -> Result<Option<Actor>> {
    let row = sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        "SELECT id, type, name, handle, is_admin, is_active FROM actors WHERE handle = ?",
    )
    .bind(handle)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| Actor {
        id: r.0,
        actor_type: r.1,
        name: r.2,
        handle: r.3,
        is_admin: r.4 != 0,
        is_active: r.5 != 0,
    }))
}

/// Insert an actor; returns the new id. Caller is responsible for
/// writing the credential row (or rows) and any admin_audit entry.
pub async fn insert_actor(
    pool: &SqlitePool,
    actor_type: &str,
    name: &str,
    handle: &str,
    is_admin: bool,
) -> Result<i64> {
    let now = super::now_ts();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO actors(type, name, handle, is_admin, is_active, created_at)
         VALUES (?, ?, ?, ?, 1, ?) RETURNING id",
    )
    .bind(actor_type)
    .bind(name)
    .bind(handle)
    .bind(is_admin as i64)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn list_actors(pool: &SqlitePool) -> Result<Vec<Actor>> {
    let rows = sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        "SELECT id, type, name, handle, is_admin, is_active
         FROM actors ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Actor {
            id: r.0,
            actor_type: r.1,
            name: r.2,
            handle: r.3,
            is_admin: r.4 != 0,
            is_active: r.5 != 0,
        })
        .collect())
}

pub async fn touch_actor_last_seen(pool: &SqlitePool, actor_id: i64) -> Result<()> {
    sqlx::query("UPDATE actors SET last_seen_at = ? WHERE id = ?")
        .bind(super::now_ts())
        .bind(actor_id)
        .execute(pool)
        .await?;
    Ok(())
}

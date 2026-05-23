//! Database pool + migrations.
//!
//! Per DESIGN.md §11 SQLite concurrency notes: WAL mode is mandatory,
//! short transactions, busy_timeout to absorb transient contention.

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::SqlitePool;

use crate::error::{AppError, Result};

pub mod queries;

/// Connect to the SQLite database at `path`, ensuring WAL + busy_timeout,
/// then run pending migrations.
pub async fn connect_and_migrate(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let url = format!("sqlite://{}", path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| AppError::Config(format!("sqlite url {url}: {e}")))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(16)
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| AppError::Config(format!("migrate: {e}")))?;

    Ok(pool)
}

/// Current unix timestamp in seconds.
pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

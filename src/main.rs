//! wikikiki CLI entrypoint.
//!
//! Subcommands:
//!   serve         start the HTTP server (default).
//!   bootstrap     create the first human admin.
//!   issue-token   create or reuse an actor and mint a bearer token.
//!   list-actors   show all actors and their status.

use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wikikiki::actor::{bootstrap_admin, issue_actor_token};
use wikikiki::config::Config;
use wikikiki::db::{connect_and_migrate, now_ts, queries};
use wikikiki::error::Result;
use wikikiki::git::Repo;
use wikikiki::web::{router, AppState};

#[derive(Parser, Debug)]
#[command(
    name = "wikikiki",
    version,
    about = "A self-hosted workspace for people and agents."
)]
struct Cli {
    /// Path to an existing TOML file. Otherwise ./wikikiki.toml or defaults.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Start the HTTP server (default if no subcommand is given).
    Serve,

    /// Create the first human admin (or reset their password).
    Bootstrap {
        #[arg(long)]
        handle: String,
        #[arg(long)]
        name: Option<String>,
        /// Prefer --password-stdin to keep passwords out of shell history.
        #[arg(
            long,
            conflicts_with = "password_stdin",
            required_unless_present = "password_stdin"
        )]
        password: Option<String>,
        /// Read the password from stdin, removing its final line ending.
        #[arg(long)]
        password_stdin: bool,
    },

    /// Issue a bearer token. Prints plaintext to stdout once.
    IssueToken {
        #[arg(long)]
        handle: String,
        /// Actor type: 'agent', 'external', etc. Must exist in actor_types.
        #[arg(long, default_value = "agent")]
        r#type: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        label: Option<String>,
        /// Override lifetime; e.g. "30d", "1y". Empty/absent → config default.
        #[arg(long)]
        lifetime: Option<String>,
        /// Issuer handle for audit. Must exist; default tries 'system'.
        #[arg(long, default_value = "system")]
        issuer: String,
    },

    /// List all actors with their type and status.
    ListActors,

    /// List credential IDs and expiry/revocation status. Never prints secrets.
    ListCredentials {
        #[arg(long)]
        handle: Option<String>,
    },
    /// Revoke a password or token by credential ID.
    RevokeCredential {
        #[arg(long)]
        id: i64,
        #[arg(long, default_value = "system")]
        issuer: String,
    },
    /// Sign an actor out of all browser sessions.
    RevokeSessions {
        #[arg(long)]
        handle: String,
        #[arg(long, default_value = "system")]
        issuer: String,
    },
    /// Disable an actor and revoke access, retaining attributed history.
    DeactivateActor {
        #[arg(long)]
        handle: String,
        #[arg(long, default_value = "system")]
        issuer: String,
    },
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;

    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Serve => serve(cfg).await,
        Cmd::Bootstrap {
            handle,
            name,
            password,
            password_stdin,
        } => {
            let password = if password_stdin {
                let mut value = String::new();
                std::io::stdin()
                    .lock()
                    .take(4099)
                    .read_to_string(&mut value)?;
                if value.len() > 4098 {
                    return Err(wikikiki::error::AppError::BadRequest(
                        "password input is too long".into(),
                    ));
                }
                if value.ends_with('\n') {
                    value.pop();
                    if value.ends_with('\r') {
                        value.pop();
                    }
                }
                if value.is_empty() || value.len() > 4096 {
                    return Err(wikikiki::error::AppError::BadRequest(
                        "password must contain 1 to 4096 bytes".into(),
                    ));
                }
                value
            } else {
                password.ok_or_else(|| {
                    wikikiki::error::AppError::BadRequest("password is required".into())
                })?
            };
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let id = bootstrap_admin(&pool, &handle, name.as_deref(), &password).await?;
            println!("bootstrapped admin: handle={handle} id={id}");
            Ok(())
        }
        Cmd::IssueToken {
            handle,
            r#type,
            name,
            label,
            lifetime,
            issuer,
        } => {
            let exp = parse_lifetime(&cfg, lifetime.as_deref())?;
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let issuer_id = operator_id(&pool, &issuer).await?;
            let (actor_id, plaintext) = issue_actor_token(
                &pool,
                &handle,
                &r#type,
                name.as_deref(),
                label.as_deref(),
                exp,
                issuer_id,
            )
            .await?;
            println!("actor_id: {actor_id}");
            println!("token:    {plaintext}");
            println!();
            println!("Store this token now. It will not be shown again.");
            Ok(())
        }
        Cmd::ListActors => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let actors = queries::list_actors(&pool).await?;
            if actors.is_empty() {
                println!("(no actors yet — run `wikikiki bootstrap` first)");
            }
            for a in actors {
                println!(
                    "{:>4}  {:<8}  {:<24}  admin={}  active={}",
                    a.id, a.actor_type, a.handle, a.is_admin, a.is_active
                );
            }
            Ok(())
        }
        Cmd::ListCredentials { handle } => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let items = wikikiki::actor::list_credentials(&pool, handle.as_deref()).await?;
            println!("ID\tACTOR\tKIND\tLABEL\tEXPIRES (unix)\tREVOKED (unix)");
            for item in items {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    item.id,
                    item.actor_handle,
                    item.kind,
                    item.label.as_deref().unwrap_or("-"),
                    item.expires_at
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "never".into()),
                    item.revoked_at
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into())
                );
            }
            Ok(())
        }
        Cmd::RevokeCredential { id, issuer } => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let issuer_id = operator_id(&pool, &issuer).await?;
            wikikiki::actor::revoke_credential(&pool, id, issuer_id).await?;
            println!("revoked credential: {id}");
            Ok(())
        }
        Cmd::RevokeSessions { handle, issuer } => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let issuer_id = operator_id(&pool, &issuer).await?;
            let count = wikikiki::actor::revoke_sessions(&pool, &handle, issuer_id).await?;
            println!("revoked {count} sessions for {handle}");
            Ok(())
        }
        Cmd::DeactivateActor { handle, issuer } => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let issuer_id = operator_id(&pool, &issuer).await?;
            wikikiki::actor::deactivate_actor(&pool, &handle, issuer_id).await?;
            println!("deactivated actor: {handle}");
            Ok(())
        }
    }
}

async fn serve(cfg: Config) -> Result<()> {
    let repo = Repo::open_or_init(&cfg.paths.git_repo, &cfg.git.author_template)?;
    let pool = connect_and_migrate(&cfg.paths.db_path).await?;
    wikikiki::pages::recover_pending(&pool, &repo, &cfg.paths.content_root).await?;
    let bind: SocketAddr = cfg.server.bind.parse().map_err(|e| {
        wikikiki::error::AppError::Config(format!("server.bind '{}': {e}", cfg.server.bind))
    })?;

    let state = AppState::new(pool, repo, cfg);
    let write_drain = state.repo.clone();

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| wikikiki::error::AppError::Config(format!("bind {bind}: {e}")))?;
    tracing::info!("wikikiki listening on http://{}", bind);

    let result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| wikikiki::error::AppError::Internal(format!("serve: {e}")));
    // A disconnected caller can leave an accepted write finishing in its
    // own task. Drain that task before the runtime is allowed to stop.
    write_drain.wait_for_writes().await;
    result
}

/// Stop accepting new connections on Ctrl-C (or SIGTERM under a supervisor)
/// and let in-flight requests finish.
///
/// A page write spans a git commit and a SQLite transaction; being killed
/// between them is exactly the gap that leaves the search index describing
/// content no page has. Draining does not make that boundary atomic, but it
/// removes the most common way of landing in it.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler unavailable");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received — draining");
}

fn parse_lifetime(cfg: &Config, raw: Option<&str>) -> Result<Option<i64>> {
    let raw = match raw {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(cfg
                .auth
                .token_default_lifetime
                .map(|d| now_ts() + d.as_secs()));
        }
    };
    let duration =
        wikikiki::config::parse_duration(raw).map_err(wikikiki::error::AppError::BadRequest)?;
    Ok(Some(now_ts() + duration.as_secs() as i64))
}

async fn operator_id(pool: &sqlx::SqlitePool, handle: &str) -> Result<i64> {
    if let Some(actor) = queries::actor_by_handle(pool, handle).await? {
        return Ok(actor.id);
    }
    if handle == "system" {
        return queries::insert_actor(pool, "system", "system", "system", false).await;
    }
    Err(wikikiki::error::AppError::NotFound(format!(
        "issuer handle '{handle}' not found"
    )))
}

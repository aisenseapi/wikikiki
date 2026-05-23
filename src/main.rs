//! wikikiki CLI entrypoint.
//!
//! Subcommands:
//!   serve         start the HTTP server (default).
//!   bootstrap     create the first human admin.
//!   issue-token   create or reuse an actor and mint a bearer token.
//!   list-actors   show all actors and their status.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wikikiki::actor::{bootstrap_admin, issue_actor_token};
use wikikiki::config::Config;
use wikikiki::db::{connect_and_migrate, now_ts, queries};
use wikikiki::error::Result;
use wikikiki::git::Repo;
use wikikiki::web::{router, AppState};

#[derive(Parser, Debug)]
#[command(name = "wikikiki", version, about = "A wiki that is the live memory of a system of actors.")]
struct Cli {
    /// Path to wikikiki.toml. Defaults to ./wikikiki.toml; missing → defaults.
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
        #[arg(long)]
        password: String,
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
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;

    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Serve => serve(cfg).await,
        Cmd::Bootstrap { handle, name, password } => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let id = bootstrap_admin(&pool, &handle, name.as_deref(), &password).await?;
            println!("bootstrapped admin: handle={handle} id={id}");
            Ok(())
        }
        Cmd::IssueToken { handle, r#type, name, label, lifetime, issuer } => {
            let pool = connect_and_migrate(&cfg.paths.db_path).await?;
            let issuer_actor = queries::actor_by_handle(&pool, &issuer).await?;
            let issuer_id = match issuer_actor {
                Some(a) => a.id,
                None => {
                    // For first-run convenience, auto-create a 'system' actor for audit if missing.
                    if issuer == "system" {
                        queries::insert_actor(&pool, "system", "system", "system", false).await?
                    } else {
                        return Err(wikikiki::error::AppError::NotFound(format!(
                            "issuer handle '{issuer}' not found"
                        )));
                    }
                }
            };
            let exp = parse_lifetime(&cfg, lifetime.as_deref())?;
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
            println!("Store this token now — it will not be shown again.");
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
    }
}

async fn serve(cfg: Config) -> Result<()> {
    let pool = connect_and_migrate(&cfg.paths.db_path).await?;
    let repo = Repo::open_or_init(&cfg.paths.git_repo, &cfg.git.author_template)?;
    let bind: SocketAddr = cfg
        .server
        .bind
        .parse()
        .map_err(|e| wikikiki::error::AppError::Config(format!("server.bind '{}': {e}", cfg.server.bind)))?;

    let state = AppState {
        pool,
        repo,
        config: Arc::new(cfg),
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(bind).await
        .map_err(|e| wikikiki::error::AppError::Config(format!("bind {bind}: {e}")))?;
    tracing::info!("wikikiki listening on http://{}", bind);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| wikikiki::error::AppError::Internal(format!("serve: {e}")))?;
    Ok(())
}

fn parse_lifetime(cfg: &Config, raw: Option<&str>) -> Result<Option<i64>> {
    let raw = match raw {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(cfg.auth.token_default_lifetime.map(|d| now_ts() + d.as_secs()));
        }
    };
    // Quick parser: digits + unit suffix (s/m/h/d/w/y). Reuses the config grammar.
    let (n, unit) = raw.split_at(raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len()));
    let n: i64 = n.parse().map_err(|_| {
        wikikiki::error::AppError::BadRequest(format!("invalid lifetime: {raw}"))
    })?;
    let mult: i64 = match unit {
        "s" | "" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        "y" => 365 * 86_400,
        other => {
            return Err(wikikiki::error::AppError::BadRequest(format!(
                "unknown duration unit: '{other}'"
            )))
        }
    };
    Ok(Some(now_ts() + n * mult))
}

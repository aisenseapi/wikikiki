//! Web UI + REST API surface.
//!
//! Every request goes through `auth_middleware`, which resolves it to one
//! actor (or anonymous) before any handler runs. Handlers then enforce
//! per-route access rules.

pub mod templates;

use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, Request, State};
use std::net::SocketAddr;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{from_fn, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::{self, Auth, Surface};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::git::Repo;
use crate::pages;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub repo: Repo,
    pub config: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/pages/*path", get(api_get_page).put(api_put_page))
        .route("/api/search", get(api_search));

    let ui = Router::new()
        .route("/", get(home))
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", post(logout))
        .route("/wiki/*path", get(view_page).post(save_page))
        .route("/wiki-history/*path", get(history_page))
        .route("/search", get(search_page))
        .route("/me", get(me_page))
        .route("/actors", get(list_actors_page));

    // Capture state in the middleware via closure rather than
    // `from_fn_with_state`. The latter has finicky extractor-tuple
    // inference for single-extractor middlewares; the closure form is
    // unambiguous and lets the middleware fn keep a simple signature.
    let mw_state = state.clone();
    Router::new()
        .merge(ui)
        .merge(api)
        .layer(from_fn(move |req, next| {
            let s = mw_state.clone();
            async move { auth_middleware(s, req, next).await }
        }))
        .with_state(state)
}

// =========================================================================
// Middleware
// =========================================================================

#[derive(Clone, Debug)]
struct Anonymous;

async fn auth_middleware(state: AppState, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let surface = if path.starts_with("/api/") {
        Surface::Api
    } else {
        Surface::Ui
    };

    // Extract identifying headers into owned values BEFORE any await — the
    // request type isn't Sync and holding `&req` across an await yields a
    // !Send future that can't be used in axum's Send-bounded middleware.
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let auth_opt = resolve_actor(&state, surface, auth_header, cookie_header).await;

    // Stash auth in extensions for handlers to read.
    match &auth_opt {
        Some(a) => {
            req.extensions_mut().insert(a.clone());
            let _ = crate::db::queries::touch_actor_last_seen(&state.pool, a.actor.id).await;
        }
        None => {
            req.extensions_mut().insert(Anonymous);
        }
    }

    // ConnectInfo is placed into request extensions by
    // `into_make_service_with_connect_info`. Reading it here avoids the
    // extractor-tuple inference headache `from_fn_with_state` has with
    // multiple extractors.
    let remote_addr = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string());
    if let Some(s) = remote_addr {
        if let Some(r) = auth::format_remote_addr(&s, &state.config.auth.log_remote_addr) {
            req.extensions_mut().insert(RemoteAddr(r));
        }
    }

    next.run(req).await
}

#[derive(Clone, Debug)]
struct RemoteAddr(pub String);

async fn resolve_actor(
    state: &AppState,
    surface: Surface,
    auth_header: Option<String>,
    cookie_header: Option<String>,
) -> Option<Auth> {
    // 1. Bearer token (api surface, but also accepted on ui — useful for curl/dev).
    if let Some(s) = auth_header.as_deref() {
        if let Some(tok) = s.strip_prefix("Bearer ") {
            if let Ok(Some(actor)) = auth::token::verify(&state.pool, tok).await {
                return Some(Auth { actor, surface });
            }
        }
    }

    // 2. Session cookie.
    if let Some(s) = cookie_header.as_deref() {
        if let Some(sid) = parse_cookie(s, auth::session::COOKIE_NAME) {
            if let Ok(Some(actor)) = auth::session::lookup(&state.pool, &sid).await {
                return Some(Auth {
                    actor,
                    surface: Surface::Ui,
                });
            }
        }
    }

    None
}

fn parse_cookie<'a>(header_value: &'a str, name: &str) -> Option<String> {
    for piece in header_value.split(';') {
        let piece = piece.trim();
        if let Some((k, v)) = piece.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn current_auth(req_exts: &axum::http::Extensions) -> Option<&Auth> {
    req_exts.get::<Auth>()
}

fn current_remote(req_exts: &axum::http::Extensions) -> Option<&str> {
    req_exts.get::<RemoteAddr>().map(|r| r.0.as_str())
}

// =========================================================================
// Access policy
// =========================================================================

fn can_read(state: &AppState, auth: Option<&Auth>) -> bool {
    auth.is_some() || state.config.ui.allow_anonymous_read
}

fn require_read(state: &AppState, auth: Option<&Auth>) -> Result<()> {
    if can_read(state, auth) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

/// Stage 1 enforces "every write is attributed" by requiring an actor on every
/// write regardless of `require_auth_for_write`. Toggling that config off only
/// affects *reads*; we do not synthesize a guest actor.
fn require_writer<'a>(_state: &AppState, auth: Option<&'a Auth>) -> Result<&'a Auth> {
    auth.ok_or(AppError::Unauthorized)
}

// =========================================================================
// Handlers — UI
// =========================================================================

async fn home(State(state): State<AppState>, req: Request) -> Result<Html<String>> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;
    let recent = pages::recent(&state.pool, 25).await?;
    Ok(Html(templates::home(auth, &recent).into_string()))
}

async fn login_form(State(_state): State<AppState>, req: Request) -> Html<String> {
    let auth = current_auth(req.extensions());
    Html(templates::login(auth, None).into_string())
}

#[derive(Deserialize)]
struct LoginInput {
    handle: String,
    password: String,
}

async fn login_submit(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response> {
    let remote = current_remote(req.extensions()).map(|s| s.to_string());
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(format!("body: {e}")))?;
    let input: LoginInput = serde_urlencoded::from_bytes(&bytes)
        .map_err(|e| AppError::BadRequest(format!("form: {e}")))?;

    match auth::session::verify_password_login(&state.pool, &input.handle, &input.password).await {
        Ok(actor) => {
            let sid = auth::session::create(
                &state.pool,
                actor.id,
                state.config.auth.session_lifetime.as_secs(),
            )
            .await?;
            auth::log_access(
                &state.pool,
                Some(actor.id),
                Some(&input.handle),
                Surface::Ui,
                "login_success",
                None,
                remote.as_deref(),
            )
            .await;
            let cookie = build_session_cookie(&sid, state.config.auth.session_lifetime.as_secs());
            let mut resp = Redirect::to("/").into_response();
            resp.headers_mut().insert(header::SET_COOKIE, cookie);
            Ok(resp)
        }
        Err(_) => {
            auth::log_access(
                &state.pool,
                None,
                Some(&input.handle),
                Surface::Ui,
                "login_failure",
                None,
                remote.as_deref(),
            )
            .await;
            let body = templates::login(None, Some("invalid handle or password")).into_string();
            Ok((StatusCode::UNAUTHORIZED, Html(body)).into_response())
        }
    }
}

async fn logout(State(state): State<AppState>, req: Request) -> Response {
    if let Some(hv) = req.headers().get(header::COOKIE) {
        if let Ok(s) = hv.to_str() {
            if let Some(sid) = parse_cookie(s, auth::session::COOKIE_NAME) {
                let _ = auth::session::revoke(&state.pool, &sid).await;
            }
        }
    }
    let mut resp = Redirect::to("/").into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "wk_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        ),
    );
    resp
}

#[derive(Deserialize)]
struct ViewQuery {
    edit: Option<u8>,
    source: Option<u8>,
}

async fn view_page(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(q): Query<ViewQuery>,
    req: Request,
) -> Result<Response> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;

    let page = pages::read(&state.pool, &state.config.paths.content_root, &path).await;

    if q.edit == Some(1) {
        // Edit view — require write.
        require_writer(&state, auth)?;
        let (content, meta) = match page {
            Ok(p) => (p.content, Some(p.meta)),
            Err(AppError::NotFound(_)) => (seed_page_content(&path), None),
            Err(e) => return Err(e),
        };
        return Ok(Html(
            templates::edit_page(auth, &path, &content, meta.as_ref()).into_string(),
        )
        .into_response());
    }

    if let Err(AppError::NotFound(_)) = page {
        // Friendly "create this page" landing.
        return Ok(Html(templates::missing_page(auth, &path).into_string()).into_response());
    }
    let page = page?;
    if q.source == Some(1) {
        return Ok((
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            page.content,
        )
            .into_response());
    }
    Ok(Html(templates::view_page(auth, &page).into_string()).into_response())
}

#[derive(Deserialize)]
struct SaveInput {
    content: String,
    summary: Option<String>,
    layer: Option<String>,
}

async fn save_page(
    State(state): State<AppState>,
    Path(path): Path<String>,
    req: Request,
) -> Result<Response> {
    let auth = current_auth(req.extensions()).cloned();
    let auth = require_writer(&state, auth.as_ref())?.clone();
    let remote = current_remote(req.extensions()).map(|s| s.to_string());

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(format!("body: {e}")))?;
    let input: SaveInput = serde_urlencoded::from_bytes(&bytes)
        .map_err(|e| AppError::BadRequest(format!("form: {e}")))?;
    let layer = input.layer.as_deref().unwrap_or("semantic");

    let result = pages::write(
        &state.pool,
        &state.repo,
        &state.config.paths.content_root,
        &path,
        layer,
        &input.content,
        auth.actor.id,
        &auth.actor.actor_type,
        &auth.actor.handle,
        input.summary.as_deref(),
    )
    .await;

    match result {
        Ok(_meta) => Ok(Redirect::to(&format!("/wiki/{path}")).into_response()),
        Err(e) => {
            auth::log_access(
                &state.pool,
                Some(auth.actor.id),
                Some(&auth.actor.handle),
                Surface::Ui,
                "write_error",
                Some(&format!("page:{path}")),
                remote.as_deref(),
            )
            .await;
            Err(e)
        }
    }
}

async fn history_page(
    State(state): State<AppState>,
    Path(path): Path<String>,
    req: Request,
) -> Result<Html<String>> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;
    pages::validate_path(&path)?;
    let meta = pages::read_meta(&state.pool, &path).await?;
    let commits = pages::history(&state.repo, &path, 50).await.unwrap_or_default();
    Ok(Html(templates::history(auth, &path, meta.as_ref(), &commits).into_string()))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

async fn search_page(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
    req: Request,
) -> Result<Html<String>> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;
    let query = q.q.unwrap_or_default();
    let hits = if query.trim().is_empty() {
        Vec::new()
    } else {
        pages::search(&state.pool, &query, 50).await?
    };
    Ok(Html(templates::search(auth, &query, &hits).into_string()))
}

async fn me_page(State(_state): State<AppState>, req: Request) -> Result<Html<String>> {
    let auth = current_auth(req.extensions()).ok_or(AppError::Unauthorized)?;
    Ok(Html(templates::me(auth).into_string()))
}

async fn list_actors_page(State(state): State<AppState>, req: Request) -> Result<Html<String>> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;
    let actors = crate::db::queries::list_actors(&state.pool).await?;
    Ok(Html(templates::actors_list(auth, &actors).into_string()))
}

// =========================================================================
// Handlers — REST API
// =========================================================================

async fn api_get_page(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    req: Request,
) -> Result<Response> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;
    let page = pages::read(&state.pool, &state.config.paths.content_root, &path).await?;
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("application/json"))
        .unwrap_or(false);
    if wants_json {
        let body = serde_json::json!({
            "path": page.meta.path,
            "title": page.meta.title,
            "layer": page.meta.layer,
            "content": page.content,
            "last_edit_at": page.meta.last_edit_at,
        });
        Ok(axum::Json(body).into_response())
    } else {
        Ok((
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            page.content,
        )
            .into_response())
    }
}

async fn api_put_page(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    req: Request,
) -> Result<Response> {
    let auth = current_auth(req.extensions()).cloned();
    let auth = require_writer(&state, auth.as_ref())?.clone();

    let summary = headers
        .get("X-Summary")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let layer = headers
        .get("X-Layer")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("semantic")
        .to_string();

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(format!("body: {e}")))?;
    let content = String::from_utf8(bytes.to_vec())
        .map_err(|_| AppError::BadRequest("body must be UTF-8".into()))?;

    let meta = pages::write(
        &state.pool,
        &state.repo,
        &state.config.paths.content_root,
        &path,
        &layer,
        &content,
        auth.actor.id,
        &auth.actor.actor_type,
        &auth.actor.handle,
        summary.as_deref(),
    )
    .await?;

    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "path": meta.path,
        "title": meta.title,
        "layer": meta.layer,
    }))
    .into_response())
}

#[derive(Deserialize)]
struct ApiSearch {
    q: String,
    limit: Option<i64>,
}

async fn api_search(
    State(state): State<AppState>,
    Query(q): Query<ApiSearch>,
    req: Request,
) -> Result<Response> {
    let auth = current_auth(req.extensions());
    require_read(&state, auth)?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let hits = pages::search(&state.pool, &q.q, limit).await?;
    let items: Vec<_> = hits
        .into_iter()
        .map(|h| {
            serde_json::json!({
                "path": h.path,
                "title": h.title,
                "snippet": h.snippet,
            })
        })
        .collect();
    Ok(axum::Json(serde_json::json!({ "hits": items })).into_response())
}

// =========================================================================
// Helpers
// =========================================================================

fn build_session_cookie(sid: &str, max_age_secs: i64) -> HeaderValue {
    let v = format!(
        "{name}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age={ma}",
        name = auth::session::COOKIE_NAME,
        ma = max_age_secs,
    );
    HeaderValue::from_str(&v).expect("session cookie ascii")
}

fn seed_page_content(path: &str) -> String {
    let title = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .replace(['-', '_'], " ");
    format!("# {title}\n\n")
}

